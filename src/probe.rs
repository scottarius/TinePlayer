use std::path::Path;

use gstreamer as gst;
use gstreamer_pbutils as pbutils;

use pbutils::prelude::*;

#[derive(Clone)]
pub struct AudioTrack {
    /// Matches matroskademux's audio_<N> pad numbering (0-based, in
    /// container order) — not any other global stream index.
    pub index: u32,
    pub codec: String,
    pub channels: u32,
    pub language: String,
    pub title: String,
}

pub fn probe_audio_tracks(path: &Path) -> Result<Vec<AudioTrack>, String> {
    let uri = glib::filename_to_uri(path, None).map_err(|e| e.to_string())?;

    let discoverer =
        pbutils::Discoverer::new(gst::ClockTime::from_seconds(10)).map_err(|e| e.to_string())?;
    let info = discoverer
        .discover_uri(&uri)
        .map_err(|e| format!("Failed to probe {}: {e}", path.display()))?;

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

    Ok(tracks)
}
