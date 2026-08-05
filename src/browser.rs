//! Reading directories for the built-in file browser.
//!
//! The system file dialog is the better tool at a desk, but it cannot be
//! driven from a controller and is not legible across a room. This provides
//! the listing; the screen that draws it lives with the other menus.

use std::path::{Path, PathBuf};

/// Extensions offered when browsing.
///
/// The pipeline typefinds rather than trusting the name, so this is about
/// keeping the listing free of clutter rather than about what will play: the
/// aim is to offer everything playable, not to guess what is inside. Anything
/// GStreamer has a demuxer for belongs here.
pub const VIDEO_EXTENSIONS: [&str; 30] = [
    // Matroska and WebM
    "mkv", "mk3d", "webm", // MPEG-4 and QuickTime
    "mp4", "m4v", "mov", "qt", "3gp", "3g2", "f4v", // AVI and its variants
    "avi", "divx", // MPEG program and transport streams
    "ts", "m2ts", "mts", "m2t", "mpg", "mpeg", "mpe", "m2v", "mpv", "vob",
    // Windows Media
    "wmv", "asf", // Flash
    "flv", // Ogg
    "ogv", "ogm", // Professional and miscellaneous
    "mxf", "dv", "nut",
];

pub struct Entry {
    pub path: PathBuf,
    pub label: String,
    pub is_dir: bool,
}

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.as_str()))
}

/// Folders first, then videos, each sorted the way a person reads them:
/// case-insensitively, so `avatar` and `Avatar` sit together rather than in
/// separate blocks.
pub fn read(directory: &Path) -> Vec<Entry> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };

    let mut folders = Vec::new();
    let mut videos = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        // Dotfiles are noise here, and on Linux the home directory is full
        // of them.
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };

        if kind.is_dir() {
            folders.push(Entry {
                path,
                label: name,
                is_dir: true,
            });
        } else if is_video(&path) {
            videos.push(Entry {
                path,
                label: name,
                is_dir: false,
            });
        }
    }

    let by_name = |a: &Entry, b: &Entry| a.label.to_lowercase().cmp(&b.label.to_lowercase());
    folders.sort_by(by_name);
    videos.sort_by(by_name);
    folders.extend(videos);
    folders
}

/// The user's own folder, which is where anything they are looking for almost
/// certainly lives.
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| rooted(Path::new(".")))
}

/// The places worth reaching in one press, for the column beside a browser's
/// listing.
///
/// Not drives: a drive letter is what this means on Windows and nothing at
/// all anywhere else, where the equivalent question is "where are my things
/// and what is plugged in". Home comes first on every platform because it is
/// where almost everything anyone browses for actually lives.
///
/// Everything is checked for existence, so a machine with nothing mounted
/// gets a short list rather than a list of places that are not there.
pub fn places() -> Vec<Entry> {
    let mut places = Vec::new();
    let mut add = |path: PathBuf, label: String| {
        if path.is_dir() && !places.iter().any(|e: &Entry| e.path == path) {
            places.push(Entry {
                path,
                label,
                is_dir: true,
            });
        }
    };

    add(home(), "Home".to_string());

    #[cfg(target_os = "windows")]
    for letter in 'A'..='Z' {
        let path = PathBuf::from(format!("{letter}:\\"));
        add(path.clone(), path.to_string_lossy().to_string());
    }

    #[cfg(not(target_os = "windows"))]
    {
        add(PathBuf::from("/"), "Filesystem".to_string());

        // Where removable and network volumes get mounted. Some systems put
        // them straight under the directory and others under one named for
        // the user, so both depths are looked at.
        let user = std::env::var("USER").unwrap_or_default();
        let mut bases: Vec<PathBuf> = vec![
            PathBuf::from("/mnt"),
            PathBuf::from("/media"),
            PathBuf::from("/Volumes"),
        ];
        if !user.is_empty() {
            bases.push(PathBuf::from("/media").join(&user));
            bases.push(PathBuf::from("/run/media").join(&user));
        }
        for base in bases {
            let Ok(entries) = std::fs::read_dir(&base) else {
                continue;
            };
            let mut found: Vec<PathBuf> = entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect();
            found.sort();
            for path in found {
                let label = path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string());
                // A mount point named for the user under /media is the
                // container for their volumes, not a volume itself.
                if label == user {
                    continue;
                }
                add(path, label);
            }
        }
    }

    places
}

pub fn start_location(remembered: Option<&Path>, last_video: Option<&Path>) -> PathBuf {
    remembered
        .filter(|path| path.is_dir())
        .map(|path| path.to_path_buf())
        .or_else(|| {
            last_video
                .and_then(|video| video.parent())
                .filter(|path| path.is_dir())
                .map(|path| path.to_path_buf())
        })
        .or_else(|| glib::home_dir().into())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Puts a separator back after a bare drive letter.
///
/// `H:` and `H:\` look alike and are not: the first is relative to whatever
/// directory that drive was last left in, so paths built from it cannot be
/// turned into URIs. Anything already rooted, and every path on Unix, is
/// returned untouched.
pub fn rooted(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut components = path.components();
    let is_bare_drive = matches!(components.next(), Some(Component::Prefix(_)))
        && !matches!(components.next(), Some(Component::RootDir));
    if !is_bare_drive {
        return path.to_path_buf();
    }

    let mut rooted = PathBuf::new();
    for (index, component) in path.components().enumerate() {
        rooted.push(component.as_os_str());
        if index == 0 {
            rooted.push(std::path::MAIN_SEPARATOR_STR);
        }
    }
    rooted
}

#[cfg(test)]
mod rooted_tests {
    use super::*;

    #[test]
    fn leaves_rooted_paths_alone() {
        for path in ["/home/scott/Videos", "/"] {
            assert_eq!(rooted(Path::new(path)), PathBuf::from(path));
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn roots_a_bare_drive() {
        assert_eq!(rooted(Path::new("H:")), PathBuf::from(r"H:\"));
        assert_eq!(
            rooted(Path::new(r"H:Videos\Movies")),
            PathBuf::from(r"H:\Videos\Movies")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn leaves_windows_roots_alone() {
        for path in [r"H:\", r"H:\Videos", r"\\server\share\Videos"] {
            assert_eq!(rooted(Path::new(path)), PathBuf::from(path));
        }
    }
}
