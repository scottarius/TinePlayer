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
}

/// Whether a track's title marks it as an audio description: a narrated
/// account of what is happening on screen, for a viewer who is blind or has
/// low vision.
///
/// Title text is the only signal there is. The container flags exist -
/// Matroska has `FlagVisualImpaired` - but GStreamer exposes none of them, so
/// there is nothing else to read.
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
        .find(|track| is_audio_description(&track.title) == described && matching(track));

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
    /// Set only from a sidecar, which is the one place a forced flag can be
    /// read from: GStreamer surfaces none. False means "nothing said so",
    /// not "not forced" - the title is still consulted, in
    /// [`crate::subtitles::Subtitle::is_forced`].
    pub forced: bool,
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
fn read_tags(tags: &gst::TagList) -> Tags {
    let text = |value: Option<String>| value.map(|v| v.trim().to_string()).unwrap_or_default();

    Tags {
        title: text(tags.get::<gst::tags::Title>().map(|t| t.get().to_string())),
        // Two spellings of the same fact, and files carry either. `Date` is a
        // plain calendar date and `DateTime` a timestamp; both reduce to the
        // year, which is all the facts line shows.
        year: tags
            .get::<gst::tags::Date>()
            .map(|d| d.get().year() as u32)
            .or_else(|| {
                tags.get::<gst::tags::DateTime>()
                    .map(|d| d.get().year() as u32)
            })
            // A container with a nonsense date is worse than one with none:
            // some muxers write the epoch when they were given nothing.
            .filter(|year| (1870..=2200).contains(year)),
        description: {
            let comment = text(
                tags.get::<gst::tags::Comment>()
                    .map(|t| t.get().to_string()),
            );
            if comment.is_empty() {
                text(
                    tags.get::<gst::tags::Description>()
                        .map(|t| t.get().to_string()),
                )
            } else {
                comment
            }
        },
        // `Image` is the cover proper and `PreviewImage` the thumbnail some
        // muxers write instead; either is better than none.
        image: tags
            .get::<gst::tags::Image>()
            .and_then(|tag| image_bytes(&tag.get()))
            .or_else(|| {
                tags.get::<gst::tags::PreviewImage>()
                    .and_then(|tag| image_bytes(&tag.get()))
            }),
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

    let mut subtitles = Vec::new();
    for (index, stream) in info.subtitle_streams().into_iter().enumerate() {
        // Blu-ray bitmap subtitles are listed by the container but no
        // decoder for them ships with GStreamer, so offering them would only
        // produce a picture with nothing drawn on it.
        let renderable = stream
            .caps()
            .map(|caps| {
                caps.structure(0)
                    .map(|s| s.name() != "subpicture/x-pgs")
                    .unwrap_or(true)
            })
            .unwrap_or(true);
        if !renderable {
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
            // Nothing in the pipeline can say. A sidecar can, and does so
            // below once the whole list is known.
            forced: false,
        });
    }

    let mut tracks = Vec::new();
    for (index, stream) in info.audio_streams().into_iter().enumerate() {
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

    let mut media = Media {
        audio: tracks,
        subtitles,
        duration_ns: info.duration().map(|d| d.nseconds()).unwrap_or(0),
        video,
        tags: info.tags().as_ref().map(read_tags).unwrap_or_default(),
    };

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
