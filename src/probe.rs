use gstreamer as gst;

use crate::source::Source;
use gstreamer_pbutils as pbutils;

use pbutils::prelude::*;

#[derive(Clone)]
pub struct AudioTrack {
    /// Position among the file's audio streams (0-based, in container
    /// order), not any global stream index. The pipeline selects tracks by
    /// the same counting, over decodebin3's stream collection.
    pub index: u32,
    pub codec: String,
    pub channels: u32,
    pub language: String,
    pub title: String,
    /// Whether the container marked this a description for blind viewers -
    /// Matroska's `FlagVisualImpaired`. `None` where the file did not say,
    /// which is not the same as saying no. See [`AudioTrack::is_described`].
    pub described: Option<bool>,
    /// Marked a director's commentary. Shown on the row and never acted on:
    /// a commentary is a thing somebody chooses, not a thing to prefer.
    pub commentary: Option<bool>,
}

impl AudioTrack {
    /// Whether this track is an audio description.
    ///
    /// The container first, its title second. The tools that add a described
    /// soundtrack set the flag - describealign writes
    /// `disposition:a:0 default+visual_impaired` - so on a file made by one of
    /// them this is the file's own answer rather than a guess at its wording.
    /// Only where the file says nothing does the title get read, which is
    /// every MP4 and every rip made by something that did not bother.
    pub fn is_described(&self) -> bool {
        self.described
            .unwrap_or_else(|| is_audio_description(&self.title))
    }

    /// What this track is for, on the same ladder everything else uses: the
    /// container's flag, then the title.
    ///
    /// Description is asked first because it is the one a preference acts on,
    /// and because the two overlap in the wild - a described track is titled
    /// "Commentary For Visually Impaired" often enough that answering
    /// "commentary" would be true and useless.
    pub fn kind(&self) -> Option<crate::label::Kind> {
        if self.is_described() {
            return Some(crate::label::Kind::Described);
        }
        let commentary = self
            .commentary
            .unwrap_or_else(|| self.title.to_lowercase().contains("commentary"));
        commentary.then_some(crate::label::Kind::Commentary)
    }
}

/// Whether a track's title marks it as an audio description: a narrated
/// account of what is happening on screen, for a viewer who is blind or has
/// low vision.
///
/// Read only where the container said nothing. Matroska's `FlagVisualImpaired`
/// is the better answer and is preferred - see [`AudioTrack::is_described`] -
/// but GStreamer discards it, so it is read back out of the file by
/// [`crate::matroska`] and is absent for every other container. This is what
/// answers the rest.
///
/// Naming is not standardized, and real files disagree wildly: Netflix labels
/// the track "Descriptive", while a Blu-ray rip called it "Commentary For
/// Visually Impaired". Those two share no words. The patterns below cover
/// what has actually been seen plus the conventions in common use, and are
/// deliberately loose: offering the wrong track is a menu row away from being
/// corrected, while missing the right one leaves someone without the feature.
///
/// "Commentary" alone is not one of them. A director's commentary is a
/// different thing entirely, and files carry both - the same Blu-ray rip had
/// three commentary-ish tracks of which exactly one was description.
pub fn is_audio_description(title: &str) -> bool {
    let title = title.to_lowercase();
    if title.contains("descri")
        || title.contains("visually impaired")
        || title.contains("visual impaired")
        || title.contains("impaired vision")
        || title.contains("narration")
    {
        return true;
    }
    // Only as a word of its own: "ad" is two letters and turns up inside
    // plenty of others.
    title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| word == "ad")
}

/// Turns a `--primary` or `--secondary` value into a track index.
///
/// Accepts the same kinds of thing `--subtitle` does, so neither argument
/// needs its own vocabulary:
///
/// - `3` - the third entry `--list-tracks` prints
/// - `en` - the first track in that language
/// - `ad` - the first described track
/// - `en:ad` - the first described track in that language
/// - `0` or `none` - no audio on this output
///
/// A plain language code will not select a described track, matching what the
/// preference does: description is only ever chosen by asking for it.
pub fn resolve_audio(spec: &str, tracks: &[AudioTrack]) -> Result<Option<u32>, String> {
    let spec = spec.trim();
    if spec.eq_ignore_ascii_case("none") {
        return Ok(None);
    }

    if let Ok(number) = spec.parse::<usize>() {
        // Zero means no audio on this output, however it is spelled - "0" and
        // "00" are the same request. Checked after parsing rather than against
        // the text, because `number - 1` below underflows otherwise: harmless
        // in a release build, where it wraps and finds no track, and a panic
        // in a debug one.
        if number == 0 {
            return Ok(None);
        }
        return tracks
            .get(number - 1)
            .map(|track| Some(track.index))
            .ok_or_else(|| {
                format!(
                    "There is no audio track {number}. The file has {}.",
                    tracks.len()
                )
            });
    }

    let (code, described) = match spec.split_once(':') {
        Some((code, kind)) if kind.eq_ignore_ascii_case("ad") => (Some(code), true),
        Some((_, kind)) => {
            return Err(format!(
                "Don't know what \"{kind}\" means. Use \"ad\" after the colon, as in \"en:ad\"."
            ));
        }
        None if spec.eq_ignore_ascii_case("ad") => (None, true),
        None => (Some(spec), false),
    };

    let matching = |track: &&AudioTrack| match code {
        Some(code) => crate::languages::matches(&track.language, code),
        None => true,
    };
    let found = tracks
        .iter()
        .find(|track| track.is_described() == described && matching(track));

    found.map(|track| Some(track.index)).ok_or_else(|| {
        let what = if described {
            "described audio track"
        } else {
            "audio track"
        };
        match code {
            Some(code) => format!("No {what} in {code}."),
            None => format!("No {what} in this file."),
        }
    })
}

/// A subtitle stream carried inside the file.
#[derive(Clone)]
pub struct SubtitleTrack {
    /// Position among the file's subtitle streams, counted the same way
    /// audio tracks are.
    pub index: u32,
    pub language: String,
    pub title: String,
    /// What the stream is, for the technical half of a row: `SRT`, `ASS`,
    /// `PGS`. Empty where the container would not say.
    pub format: String,
    /// The container's forced flag, then the sidecar's where the container was
    /// silent. `None` is "nothing said so", which is not "not forced" - the
    /// title is still read after this, in
    /// [`crate::subtitles::Subtitle::is_forced`].
    pub forced: Option<bool>,
    /// Marked for the hard of hearing - Matroska's `FlagHearingImpaired`, the
    /// flag behind the "SDH" a track title usually spells out by hand.
    pub hearing_impaired: Option<bool>,
    /// Marked a commentary. Read for what a row says rather than for what gets
    /// chosen: nothing prefers or avoids a commentary automatically.
    pub commentary: Option<bool>,
}

/// The picture, as far as the container describes it.
///
/// Every field is optional in practice rather than in type: a stream that
/// cannot say its own size reports zero, and the page leaves the line out
/// rather than printing it. Kept separate from the audio and subtitle lists
/// because there is only ever one of it worth showing - a file with two video
/// streams is a rarity, and the first is the one that plays.
#[derive(Clone, Default)]
pub struct VideoDetails {
    pub width: u32,
    pub height: u32,
    /// As `pb_utils_get_codec_description` words it - "H.264", not "avc1".
    pub codec: String,
    /// Zero when the container states no frame rate, which is common for a
    /// variable-rate recording and for some MP4 muxers.
    pub fps: f64,
}

impl VideoDetails {
    /// The shorthand a viewer recognizes: 1080p rather than 1920x1080.
    ///
    /// Matched on height, and on the nearest standard below rather than an
    /// exact figure, because a widescreen film is letterboxed to fewer lines
    /// than the format names - a 2.39:1 transfer at "1080p" is 1920x800. An
    /// unusual size falls back to stating both numbers, which is honest
    /// rather than rounded into a name it does not deserve.
    pub fn resolution(&self) -> Option<String> {
        if self.width == 0 || self.height == 0 {
            return None;
        }
        let long = self.width.max(self.height);
        let name = match long {
            0..=800 => None,
            801..=1400 => Some("720p"),
            1401..=2000 => Some("1080p"),
            2001..=3000 => Some("1440p"),
            3001..=4200 => Some("4K"),
            _ => Some("8K"),
        };
        Some(match name {
            Some(name) => name.to_string(),
            None => format!("{}x{}", self.width, self.height),
        })
    }
}

/// What the container says about itself, as opposed to about its streams.
///
/// This is the last resort the media page falls back to when there is no
/// sidecar beside the file. Most video files carry nothing here at all -
/// a muxer writes what it was given, and a download was given nothing - but
/// a recording from a camera or a purchased file often names itself, and a
/// title from the file beats a filename with a release tag in it.
#[derive(Clone, Default)]
pub struct Tags {
    pub title: String,
    /// Whatever `GST_TAG_DATE` or `GST_TAG_DATE_TIME` carried, reduced to the
    /// year, which is the only part the page shows.
    pub year: Option<u32>,
    /// From `GST_TAG_COMMENT` or `GST_TAG_DESCRIPTION`, in that order.
    pub description: String,
    /// Cover art carried inside the container, encoded as it was stored -
    /// which is a JPEG or a PNG in every case that matters, since those are
    /// the two an image tag is written in.
    ///
    /// Worth reading rather than skipping: the Matroska rips in the library
    /// this was written against each carry a 395x500 poster, so a file with
    /// no artwork beside it on disk is not necessarily a file with no
    /// artwork. It is the last thing tried, after everything on disk.
    pub image: Option<Vec<u8>>,
}

/// What the file contains, from a single pass. Probing is not free, and the
/// menu needs both lists at once.
pub struct Media {
    pub audio: Vec<AudioTrack>,
    pub subtitles: Vec<SubtitleTrack>,
    /// Zero when the source could not say, which some live streams cannot.
    pub duration_ns: u64,
    pub video: VideoDetails,
    pub tags: Tags,
}

/// Pulls the few container tags the media page can use out of the whole list.
///
/// Deliberately a short list. A container can carry dozens of tags and almost
/// none of them describe the film: the page shows a title, a year and a
/// summary, so those are what is read. Anything empty is left empty rather
/// than filled with a placeholder, because an absent tag and a blank one mean
/// the same thing to whatever is going to fall back past it.
/// The file's own tags, without the ones belonging to the tracks inside it.
///
/// `DiscovererInfo::tags` is the whole file merged - the container's tags and
/// every stream's, with repeated tags concatenated into one value. That is the
/// wrong list to describe a film by, and it goes wrong in a way that reads as
/// real data rather than as a mistake: a Matroska file whose audio tracks are
/// named "English" and "Romany" answers a global `Title` of "English, Romany",
/// and the page prints it in the largest type on the screen. Those same track
/// names are read deliberately elsewhere here, to label the tracks - the merged
/// list was picking them up a second time as the name of the film.
///
/// Asks for the container's own tags first. Measured 2026-08-11 on a Matroska
/// file: `container_streams` is empty and the top-level `stream_info` carries
/// no tags at all through these bindings, so this falls through to the merged
/// list in practice - which is why [`without_stream_titles`] exists to clean
/// up after it. Kept because it costs nothing and is right where it answers.
fn container_tags(info: &pbutils::DiscovererInfo) -> Option<gst::TagList> {
    info.container_streams()
        .first()
        .and_then(|container| container.tags())
        .or_else(|| info.stream_info().and_then(|stream| stream.tags()))
        .or_else(|| info.tags())
}

/// Whether a title is nothing but the names of the tracks inside the file.
///
/// The tag list this comes from is every tag in the file flattened into one,
/// and `title` may be set on any stream. A Matroska file whose subtitle tracks
/// are named "English" and "Romany" therefore answers a whole-file title of
/// "Romany, English" - the two track names, joined by the same comma that
/// separates any repeated tag. Printed as the name of the film, in the largest
/// type on the page, it reads as real information rather than as a mistake.
///
/// The track names are already known here, having been read from each stream
/// to label it, so this asks the plain question: is every part of this title
/// one of them? If it is, the file has said nothing about itself and the name
/// belongs to the tracks.
///
/// A file whose one track happens to be named after the film loses its title
/// to this, and falls back to the file name. That is the right way round: a
/// name evidenced only by a track label is not the film's name, and the file
/// name is something the viewer can read and judge.
fn without_stream_titles(title: &str, stream_titles: &[String]) -> bool {
    let title = title.trim();
    if title.is_empty() || stream_titles.is_empty() {
        return false;
    }
    let mut parts = title
        .split(',')
        .map(|part| part.trim().to_lowercase())
        .filter(|part| !part.is_empty())
        .peekable();
    parts.peek().is_some() && parts.all(|part| stream_titles.contains(&part))
}

fn read_tags(tags: &gst::TagList) -> Tags {
    let text = |value: Option<String>| value.map(|v| v.trim().to_string()).unwrap_or_default();

    Tags {
        title: text(tags.get::<gst::tags::Title>().map(|t| t.get().to_string())),
        // `Date` only, and deliberately not `DateTime`.
        //
        // They are not two spellings of one fact, which is what this used to
        // assume. `Date` comes from a release date a muxer was told; `DateTime`
        // is when the file itself was written, which Matroska carries as a
        // matter of course. Read together, a 2009 film muxed in 2010 announced
        // itself as 2010 in the largest facts line on the page - and there is
        // nothing about the number that says which of the two it was.
        year: tags
            .get::<gst::tags::Date>()
            .map(|d| d.get().year() as u32)
            // A container with a nonsense date is worse than one with none:
            // some muxers write the epoch when they were given nothing.
            .filter(|year| (1870..=2200).contains(year)),
        // `Description` only, and deliberately not `Comment`, which used to be
        // preferred over it.
        //
        // `Description` is the field Matroska and iTunes mean for a synopsis.
        // `Comment` is where the tools that made the file write about
        // themselves: an encoder banner, a settings dump, a release note, the
        // address of wherever it came from. None of that is about the film,
        // and the page gives a summary the largest block of text it has - so a
        // wrong one is three confident lines of somebody else's advertising.
        description: text(
            tags.get::<gst::tags::Description>()
                .map(|t| t.get().to_string()),
        ),
        // `Image` is the cover proper. `PreviewImage` used to stand in for it
        // and no longer does: it is a thumbnail, and what muxers put there is
        // usually a frame from somewhere in the film. Hung in a poster frame,
        // two by three, a random frame does not read as artwork the file
        // happened to lack - it reads as the wrong picture.
        image: tags
            .get::<gst::tags::Image>()
            .and_then(|tag| image_bytes(&tag.get())),
    }
}

/// Copies the encoded picture out of an image tag.
///
/// The sample's buffer is owned by the tag list, which does not outlive the
/// probe, so this has to be a copy rather than a borrow. Guarded on a size
/// that could plausibly be a picture: a tag holding a few bytes is a muxer
/// writing something that is not one, and a tag holding tens of megabytes is
/// not worth carrying about for a thumbnail.
fn image_bytes(sample: &gst::Sample) -> Option<Vec<u8>> {
    const PLAUSIBLE: std::ops::RangeInclusive<usize> = 64..=32 * 1024 * 1024;

    let buffer = sample.buffer()?;
    let map = buffer.map_readable().ok()?;
    PLAUSIBLE
        .contains(&map.size())
        .then(|| map.as_slice().to_vec())
}

pub fn probe_media(source: &Source) -> Result<Media, String> {
    // The discoverer has always worked in URIs, so a remote source needs
    // nothing special here: it opens an HTTP or SMB stream the same way it
    // opens a file, and reports the same track list either way.
    let uri = source.uri();

    let discoverer =
        pbutils::Discoverer::new(gst::ClockTime::from_seconds(10)).map_err(|e| e.to_string())?;
    let info = discoverer
        .discover_uri(&uri)
        .map_err(|e| format!("Failed to probe {uri}: {e}"))?;

    // Asking is not the same as being answered: a source that never replies
    // still comes back here as `Ok`, carrying an info that reports a timeout
    // and lists no streams at all. Taken at face value that looks like a
    // playable file with nothing in it, which is how an unreachable address
    // ended up in the menu instead of raising an error.
    match info.result() {
        pbutils::DiscovererResult::Ok => {}
        // The streams are known even when something to decode them is not.
        // The pipeline will say exactly what it cannot build, which is more
        // use than a refusal here.
        pbutils::DiscovererResult::MissingPlugins => {}
        pbutils::DiscovererResult::Timeout => {
            return Err(format!("Timed out reading {uri}. Nothing answered."));
        }
        other => return Err(format!("Couldn't read {uri}: {other:?}")),
    }

    // What the container itself says about each track, where it is a local
    // Matroska file. GStreamer reads these flags and hands none of them on -
    // see `crate::matroska` - so they are read again here rather than being
    // guessed from track titles.
    //
    // Joined by the Matroska track number, which `matroskademux` puts in the
    // `container-specific-track-id` tag. Never by the order the streams came
    // in: two orderings that agree today are two orderings free to disagree,
    // and the failure would be silent and per-file.
    let container = source
        .local()
        .map(crate::matroska::flags)
        .unwrap_or_default();
    let flags_for = |stream: &pbutils::DiscovererStreamInfo| {
        stream
            .tags()
            .and_then(|tags| {
                tags.index_generic("container-specific-track-id", 0)
                    .and_then(|value| value.get::<String>().ok())
            })
            .and_then(|id| id.parse::<u64>().ok())
            .and_then(|id| container.get(&id).copied())
            .unwrap_or_default()
    };

    let mut subtitles = Vec::new();
    for (index, stream) in info.subtitle_streams().into_iter().enumerate() {
        let container = flags_for(stream.upcast_ref());
        // Bitmap subtitles - Blu-ray PGS and the DVD subpictures in an
        // ordinary rip - are left out of the list.
        //
        // Not for the reason this said until 2026-08-19, which was that
        // GStreamer ships no decoder for them. It does: `dvdspu` takes
        // `subpicture/x-pgs` and `subpicture/x-dvd` alike and composites
        // either onto the picture, and `subtitleoverlay` plugs it unasked.
        // Two separate things were wrong underneath that.
        //
        // The first is fixed: the Windows and macOS packages shipped the
        // similarly named `dvdsub`, which parses, and not `dvdspu`, which
        // draws - so the renderer was absent and the pad would not link.
        //
        // The second is not, and is why this filter is still here. Choosing
        // one stops the film a few seconds in, on the frame before the first
        // line. `dvdspu` holds a subpicture from the moment it arrives until
        // the picture reaches it, which pulls the demuxer seconds ahead; the
        // demuxer produces every stream in the file, so that read-ahead fills
        // the multiqueue slots of the tracks nobody selected, and the demuxer
        // stops. Nothing errors, because nothing has failed. The plan's
        // backlog carries the measurements and what has already been ruled
        // out. Offering a track that freezes the film is worse than not
        // offering it, so until that is fixed, neither is listed.
        //
        // Only these two. Anything else GStreamer cannot draw is caught where
        // it becomes known rather than guessed at here - see `link_stream` in
        // pipeline.rs, which drops such a stream instead of stalling on it.
        let bitmap = stream
            .caps()
            .and_then(|caps| caps.structure(0).map(|s| s.name().to_string()))
            .is_some_and(|name| name.starts_with("subpicture/"));
        if bitmap {
            continue;
        }

        subtitles.push(SubtitleTrack {
            index: index as u32,
            language: stream
                .language()
                .map(|l| l.to_string())
                .unwrap_or_else(|| "und".to_string()),
            title: stream
                .tags()
                .and_then(|tags| tags.get::<gst::tags::Title>().map(|t| t.get().to_string()))
                .unwrap_or_default(),
            // What the container said, where it said anything. A `.nfo`
            // beside the file fills the gap below for the ones that did not,
            // and a track title is read after that - flags first, sidecar
            // second, names last, because that is the order of how much each
            // one actually knows.
            format: stream
                .caps()
                .and_then(|caps| {
                    caps.structure(0)
                        .map(|s| subtitle_format(s.name().as_str()))
                })
                .unwrap_or_default(),
            forced: container.forced,
            hearing_impaired: container.hearing_impaired,
            commentary: container.commentary,
        });
    }

    let mut tracks = Vec::new();
    for (index, stream) in info.audio_streams().into_iter().enumerate() {
        let container = flags_for(stream.upcast_ref());
        let codec = stream
            .caps()
            .map(|caps| pbutils::pb_utils_get_codec_description(&caps).to_string())
            .unwrap_or_else(|| "?".to_string());

        let title = stream
            .tags()
            .and_then(|tags| tags.get::<gst::tags::Title>().map(|t| t.get().to_string()))
            .unwrap_or_default();

        tracks.push(AudioTrack {
            index: index as u32,
            codec,
            channels: stream.channels(),
            language: stream
                .language()
                .map(|l| l.to_string())
                .unwrap_or_else(|| "und".to_string()),
            title,
            described: container.visual_impaired,
            commentary: container.commentary,
        });
    }

    // The first video stream, which is the one that plays. A file with two is
    // rare enough that picking between them would be inventing a problem.
    let video = info
        .video_streams()
        .into_iter()
        .next()
        .map(|stream| VideoDetails {
            width: stream.width(),
            height: stream.height(),
            codec: stream
                .caps()
                .map(|caps| pbutils::pb_utils_get_codec_description(&caps).to_string())
                .unwrap_or_default(),
            // Stated as a fraction, so 23.976 arrives as 24000/1001 and comes
            // out exact rather than as whatever a decimal field rounded to. A
            // zero denominator means the container said nothing.
            fps: match stream.framerate() {
                rate if rate.denom() != 0 => rate.numer() as f64 / rate.denom() as f64,
                _ => 0.0,
            },
        })
        .unwrap_or_default();

    let stream_titles: Vec<String> = tracks
        .iter()
        .map(|track| track.title.trim().to_lowercase())
        .chain(
            subtitles
                .iter()
                .map(|track| track.title.trim().to_lowercase()),
        )
        .filter(|title| !title.is_empty())
        .collect();

    let mut media = Media {
        audio: tracks,
        subtitles,
        duration_ns: info.duration().map(|d| d.nseconds()).unwrap_or(0),
        video,
        tags: container_tags(&info)
            .as_ref()
            .map(read_tags)
            .unwrap_or_default(),
    };
    // The whole-file tag list carries the tracks' own titles too, and this is
    // where they are recognisable: the track names have just been read.
    if without_stream_titles(&media.tags.title, &stream_titles) {
        media.tags.title.clear();
    }

    // What a media library already worked out about this file, where one has
    // been kept beside it. Only ever adds to what the pipeline found - a
    // forced flag it cannot see, and a language an MP4 often does not carry -
    // and only for a local file, since a sidecar is something on disk.
    if let Some(sidecar) = source.local().and_then(crate::nfo::read) {
        sidecar.apply(&mut media);
    }

    Ok(media)
}

#[cfg(test)]
mod video_details_tests {
    use super::VideoDetails;

    fn at(width: u32, height: u32) -> Option<String> {
        VideoDetails {
            width,
            height,
            ..Default::default()
        }
        .resolution()
    }

    /// The sizes a film actually arrives at, which are mostly not the sizes
    /// the format names. A 2.39:1 transfer has 800 lines and is still 1080p.
    #[test]
    fn names_the_format_a_viewer_would_recognize() {
        assert_eq!(at(1920, 1080).as_deref(), Some("1080p"));
        assert_eq!(at(1920, 800).as_deref(), Some("1080p"));
        assert_eq!(at(1920, 804).as_deref(), Some("1080p"));
        assert_eq!(at(1280, 720).as_deref(), Some("720p"));
        assert_eq!(at(1280, 534).as_deref(), Some("720p"));
        assert_eq!(at(3840, 2160).as_deref(), Some("4K"));
        assert_eq!(at(4096, 1716).as_deref(), Some("4K"));
        assert_eq!(at(2560, 1440).as_deref(), Some("1440p"));
    }

    /// Anything that is not one of the formats states both numbers rather
    /// than being rounded into a name it does not deserve.
    #[test]
    fn an_unusual_size_says_what_it_is() {
        assert_eq!(at(640, 480).as_deref(), Some("640x480"));
        assert_eq!(at(720, 576).as_deref(), Some("720x576"));
    }

    /// A stream that could not say reports zero, and a page that printed
    /// "0x0" would look broken rather than uninformed.
    #[test]
    fn nothing_known_is_nothing_shown() {
        assert_eq!(at(0, 0), None);
        assert_eq!(at(1920, 0), None);
        assert_eq!(at(0, 1080), None);
    }
}

#[cfg(test)]
mod audio_description_tests {
    use super::is_audio_description;

    #[test]
    fn recognizes_real_titles() {
        // Both seen in the wild, and sharing no vocabulary.
        assert!(is_audio_description("Descriptive"));
        assert!(is_audio_description(
            "2.0 Dolby Digital (Commentary For Visually Impaired)"
        ));
        // Conventional names, not yet seen but widely used.
        for title in [
            "Audio Description",
            "English (Audio Description)",
            "English AD",
            "Described Video",
            "AD",
        ] {
            assert!(is_audio_description(title), "missed {title}");
        }
    }

    #[test]
    fn leaves_ordinary_tracks_alone() {
        for title in [
            "English",
            "Commentary by director James Gunn",
            "3.0 Dolby Digital (1993 LD Audio Commentary - 2018)",
            "2.0 Dolby Digital (Isolated Score 2018)",
            "1.0 DTS-HD-MA (1977 35mm mono mix)",
            "Sub-commentary",
            // Contains "ad" only inside other words.
            "Deadpool Cast Track",
        ] {
            assert!(!is_audio_description(title), "wrongly matched {title}");
        }
    }
}

#[cfg(test)]
mod resolve_audio_tests {
    use super::*;

    fn tracks() -> Vec<AudioTrack> {
        [
            ("en", "English"),
            ("de", "German"),
            ("en", "Descriptive"),
            ("de", "Audio Description"),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (language, title))| AudioTrack {
            index: index as u32,
            codec: "AC-3".to_string(),
            channels: 2,
            language: language.to_string(),
            title: title.to_string(),
            described: None,
            commentary: None,
        })
        .collect()
    }

    #[test]
    fn takes_a_number_none_or_a_language() {
        let tracks = tracks();
        assert_eq!(resolve_audio("2", &tracks), Ok(Some(1)));
        assert_eq!(resolve_audio("0", &tracks), Ok(None));
        assert_eq!(resolve_audio("none", &tracks), Ok(None));
        assert_eq!(resolve_audio("de", &tracks), Ok(Some(1)));
    }

    /// Any spelling of zero, and any surrounding space, means the same thing.
    /// "00" used to reach `number - 1` and underflow.
    #[test]
    fn every_spelling_of_zero_means_none() {
        let tracks = tracks();
        for spec in ["0", "00", "000", " 0 ", "none", "NONE", "None"] {
            assert_eq!(resolve_audio(spec, &tracks), Ok(None), "for {spec:?}");
        }
    }

    #[test]
    fn a_language_alone_never_picks_a_described_track() {
        // German track 4 is described and track 2 is not, so the plain code
        // has to reach past the described one.
        assert_eq!(resolve_audio("de", &tracks()), Ok(Some(1)));
    }

    #[test]
    fn ad_picks_description() {
        let tracks = tracks();
        assert_eq!(resolve_audio("ad", &tracks), Ok(Some(2)));
        assert_eq!(resolve_audio("de:ad", &tracks), Ok(Some(3)));
        assert_eq!(resolve_audio("en:ad", &tracks), Ok(Some(2)));
    }

    #[test]
    fn reports_what_it_could_not_find() {
        let tracks = tracks();
        assert!(resolve_audio("9", &tracks).is_err());
        assert!(resolve_audio("fr", &tracks).is_err());
        assert!(resolve_audio("fr:ad", &tracks).is_err());
        assert!(resolve_audio("en:sdh", &tracks).is_err());
    }
}

/// Preferring what the container states over what a track is called.
#[cfg(test)]
mod described_tests {
    use super::AudioTrack;

    fn track(title: &str, described: Option<bool>) -> AudioTrack {
        AudioTrack {
            index: 0,
            codec: "AAC".into(),
            channels: 2,
            language: "en".into(),
            title: title.into(),
            described,
            commentary: None,
        }
    }

    /// The file's own answer wins, in both directions. The second half is the
    /// half that matters: a track titled "Commentary with the director" that
    /// the file marks as a description is a description, and the naming rules
    /// would never have found it.
    #[test]
    fn the_container_is_believed_over_the_title() {
        assert!(!track("English Audio Description", Some(false)).is_described());
        assert!(track("Commentary with the director", Some(true)).is_described());
    }

    /// Where the file says nothing - every MP4, and any rip made by something
    /// that did not set the flag - the title is still read.
    #[test]
    fn a_silent_container_falls_through_to_the_title() {
        assert!(track("English Audio Description", None).is_described());
        assert!(!track("English", None).is_described());
    }

    /// Absent is not false. A file that states the flag on one track and omits
    /// it on another has not said the second is ordinary, so the second is
    /// still judged on its name.
    #[test]
    fn absent_is_not_a_denial() {
        assert!(track("Descriptive Audio", None).is_described());
    }
}

/// A subtitle stream's format, where naming it tells a viewer something.
///
/// Only the bitmap formats are named, and the reason is that they behave
/// differently: they are pictures, so the subtitle font and size settings do
/// not touch them, and as of 1.5 they cannot be drawn at all. "PGS" on a row
/// is the difference between two subtitles that would otherwise look
/// interchangeable.
///
/// Text formats are deliberately not named, for two reasons. It says nothing
/// anybody would act on - "SRT" beside a subtitle does not help anyone choose
/// it - and we do not reliably know it anyway: SubRip, WebVTT, SAMI and an
/// MKV `S_TEXT/UTF8` track all reach us as decoded `text/x-raw`, the container
/// format already gone. This used to answer "SRT" for every one of them,
/// which was a guess wearing the clothes of a fact.
fn subtitle_format(media_type: &str) -> String {
    match media_type {
        "subpicture/x-pgs" => "PGS".to_string(),
        "subpicture/x-dvd" => "VOBSUB".to_string(),
        "subpicture/x-dvb" => "DVB".to_string(),
        _ => String::new(),
    }
}

impl SubtitleTrack {
    /// What this subtitle is for: the container's flags, then the sidecar's,
    /// then the words in its title.
    ///
    /// Forced is asked first because it is the one a preference acts on, and
    /// because a track is rarely both - forced subtitles carry signs and
    /// foreign lines, SDH carries everything including the sound.
    pub fn kind(&self) -> Option<crate::label::Kind> {
        let title = self.title.to_lowercase();
        if self.forced.unwrap_or_else(|| title.contains("forced")) {
            return Some(crate::label::Kind::Forced);
        }
        let sdh = self.hearing_impaired.unwrap_or_else(|| {
            title.contains("sdh")
                || title.contains("hearing impaired")
                || title.contains("hard of hearing")
                || title
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .any(|word| word == "hi" || word == "cc")
        });
        if sdh {
            return Some(crate::label::Kind::Sdh);
        }
        let commentary = self
            .commentary
            .unwrap_or_else(|| title.contains("commentary"));
        commentary.then_some(crate::label::Kind::Commentary)
    }
}

/// Working out what a subtitle is for, on the flags-then-title ladder.
#[cfg(test)]
mod subtitle_kind_tests {
    use super::SubtitleTrack;
    use crate::label::Kind;

    fn track(title: &str) -> SubtitleTrack {
        SubtitleTrack {
            index: 0,
            language: "en".into(),
            title: title.into(),
            format: "SRT".into(),
            forced: None,
            hearing_impaired: None,
            commentary: None,
        }
    }

    #[test]
    fn the_title_answers_where_the_container_did_not() {
        assert_eq!(track("English (Forced)").kind(), Some(Kind::Forced));
        assert_eq!(track("English SDH").kind(), Some(Kind::Sdh));
        assert_eq!(track("English CC").kind(), Some(Kind::Sdh));
        assert_eq!(
            track("Director's Commentary").kind(),
            Some(Kind::Commentary)
        );
        assert_eq!(track("English").kind(), None);
    }

    /// The flag wins both ways. The second half is the one that changed
    /// behaviour: a file stating "not forced" is believed over a title that
    /// says otherwise, where before the title always won.
    #[test]
    fn the_container_outranks_the_title() {
        let mut flagged = track("English");
        flagged.forced = Some(true);
        assert_eq!(flagged.kind(), Some(Kind::Forced));

        let mut denied = track("English (Forced)");
        denied.forced = Some(false);
        assert_ne!(denied.kind(), Some(Kind::Forced));
    }

    /// Forced is asked first, because it is the one a preference acts on and
    /// a track is rarely both.
    #[test]
    fn forced_is_asked_before_the_rest() {
        let mut both = track("English SDH Forced");
        both.hearing_impaired = Some(true);
        both.forced = Some(true);
        assert_eq!(both.kind(), Some(Kind::Forced));
    }

    /// "hi" is two letters that turn up inside other words, so it counts only
    /// as a word of its own - the same rule "ad" gets for description.
    #[test]
    fn short_marks_are_read_as_whole_words() {
        assert_eq!(track("Hindi").kind(), None);
        assert_eq!(track("English hi").kind(), Some(Kind::Sdh));
    }
}
