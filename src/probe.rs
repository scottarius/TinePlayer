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

/// A subtitle stream carried inside the file.
#[derive(Clone)]
pub struct SubtitleTrack {
    /// Position among the file's subtitle streams, counted the same way
    /// audio tracks are.
    pub index: u32,
    pub language: String,
    pub title: String,
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

    Ok(Media {
        audio: tracks,
        subtitles,
        duration_ns: info.duration().map(|d| d.nseconds()).unwrap_or(0),
    })
}
