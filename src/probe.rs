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

/// What the file contains, from a single pass. Probing is not free, and the
/// menu needs both lists at once.
pub struct Media {
    pub audio: Vec<AudioTrack>,
    pub subtitles: Vec<SubtitleTrack>,
    /// Zero when the source could not say, which some live streams cannot.
    pub duration_ns: u64,
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

    let mut media = Media {
        audio: tracks,
        subtitles,
        duration_ns: info.duration().map(|d| d.nseconds()).unwrap_or(0),
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
