//! Choosing subtitles, from inside the file or from alongside it.
//!
//! Kept apart from the audio settings deliberately: the subtitle language is
//! an independent choice, and may well be a third language rather than a copy
//! of either soundtrack.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::probe::SubtitleTrack;

/// Formats GStreamer can parse from a plain file. Blu-ray `.sup` and the
/// VOBSUB `.sub`/`.idx` pair are deliberately absent: both are bitmap
/// formats with no decoder in the shipped GStreamer.
const EXTENSIONS: [&str; 4] = ["srt", "ass", "ssa", "vtt"];

/// One entry in the subtitle chooser.
#[derive(Clone, Debug, PartialEq)]
pub enum Subtitle {
    Embedded { index: u32, label: String },
    External { path: PathBuf, label: String },
}

impl Subtitle {
    pub fn label(&self) -> &str {
        match self {
            Subtitle::Embedded { label, .. } | Subtitle::External { label, .. } => label,
        }
    }

    pub fn choice(&self) -> SubtitleChoice {
        match self {
            Subtitle::Embedded { index, .. } => SubtitleChoice::Embedded(*index),
            Subtitle::External { path, .. } => SubtitleChoice::External(path.clone()),
        }
    }
}

/// The persisted form. Stored by index or by path rather than by menu
/// position, so it still resolves when the list changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SubtitleChoice {
    Embedded(u32),
    External(PathBuf),
}

/// Everything on offer for a video: what is inside it, then what sits beside
/// it on disk.
///
/// `video` is `None` for a remote source, which offers only what is embedded -
/// there is no folder to look in, and a media server hands its own subtitles
/// over inside the stream anyway.
pub fn options(video: Option<&Path>, embedded: &[SubtitleTrack]) -> Vec<Subtitle> {
    let mut options: Vec<Subtitle> = embedded
        .iter()
        .map(|track| Subtitle::Embedded {
            index: track.index,
            label: if track.title.is_empty() {
                track.language.clone()
            } else {
                format!("{} — {}", track.language, track.title)
            },
        })
        .collect();
    if let Some(video) = video {
        options.extend(external(video));
    }
    options
}

/// Subtitle files sitting next to the video and named after it.
///
/// The convention media libraries use is `<video name>.<language>.srt`, with
/// optional extra tags such as `.hi` for hearing-impaired versions. Whatever
/// sits between the video's name and the extension is shown as the label,
/// since that is where the language ended up.
fn external(video: &Path) -> Vec<Subtitle> {
    let Some(directory) = video.parent() else {
        return Vec::new();
    };
    let Some(stem) = video.file_stem().map(|s| s.to_string_lossy().to_string()) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };

    let mut found: Vec<Subtitle> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let extension = path.extension()?.to_string_lossy().to_lowercase();
            if !EXTENSIONS.contains(&extension.as_str()) {
                return None;
            }

            // Compared without the extension, so that an upper-case one
            // doesn't defeat the trimming.
            let name = path.file_stem()?.to_string_lossy().to_string();
            // Whatever sits between the video's name and the extension is
            // the label, since that is where the language ends up. A file
            // named exactly after the video leaves nothing.
            let label = name.strip_prefix(&stem)?.trim_matches('.').to_string();

            Some(Subtitle::External {
                path: path.clone(),
                label: if label.is_empty() {
                    "External".to_string()
                } else {
                    label
                },
            })
        })
        .collect();

    found.sort_by(|a, b| a.label().cmp(b.label()));
    found
}
