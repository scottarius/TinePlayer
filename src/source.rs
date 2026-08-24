//! What is being played: a file on disk, or something remote.
//!
//! GStreamer works in URIs throughout, so a local file is just one kind of
//! source rather than the only kind. Keeping the distinction explicit - rather
//! than passing a `PathBuf` around and hoping it never holds a URL - is what
//! makes the difference visible at the places it actually matters: finding
//! subtitle files sitting beside a video, and the built-in browser, neither of
//! which mean anything for a remote source.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    File(PathBuf),
    Remote(String),
}

impl Source {
    /// Reads a command-line argument or a path handed over by another
    /// application.
    ///
    /// A Windows path is not mistaken for a URI: `C:\...` has no `//` after the
    /// colon, and a single-letter scheme is not one anyway.
    pub fn parse(argument: &str) -> Self {
        match argument.split_once("://") {
            Some((scheme, _))
                if scheme.len() > 1
                    && scheme
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '.') =>
            {
                if scheme.eq_ignore_ascii_case("file") {
                    // Turn it straight back into a path, so everything local
                    // keeps working the same however it was named.
                    match glib::filename_from_uri(argument) {
                        Ok((path, _)) => Self::File(path),
                        Err(_) => Self::Remote(argument.to_string()),
                    }
                } else {
                    Self::Remote(argument.to_string())
                }
            }
            _ => Self::File(PathBuf::from(argument)),
        }
    }

    /// What GStreamer is given to open.
    pub fn uri(&self) -> String {
        match self {
            Self::File(path) => glib::filename_to_uri(path, None)
                .map(|uri| uri.to_string())
                .unwrap_or_else(|e| {
                    // Whatever comes out of here will not open: the fallback
                    // is a plain concatenation, and on Windows that leaves
                    // backslashes and a missing slash in a string GStreamer
                    // reads as a host name. Said out loud rather than left to
                    // surface as an unexplained "could not open".
                    log::error!("Could not form a URI for {}: {e}", path.display());
                    format!("file://{}", path.display())
                }),
            Self::Remote(uri) => uri.clone(),
        }
    }

    /// The path on disk, for the things only a local file can do: listing the
    /// folder beside it for subtitles, and reopening the browser there.
    pub fn local(&self) -> Option<&Path> {
        match self {
            Self::File(path) => Some(path),
            Self::Remote(_) => None,
        }
    }

    /// Whether this can be played at all. A remote source is taken on trust:
    /// reaching over the network to find out belongs to playback, not to
    /// starting up.
    pub fn is_available(&self) -> bool {
        match self {
            Self::File(path) => path.exists(),
            Self::Remote(_) => true,
        }
    }

    /// A `file://` URI that could not be read as a path, and so ended up here
    /// as a remote source it is not.
    ///
    /// `file://video.mkv` is the mistake this catches: two slashes rather than
    /// three names a host called `video.mkv` with no path at all, which is why
    /// `filename_from_uri` refuses it. Left as remote it would be taken on
    /// trust by `is_available` above - a network resource that might yet turn
    /// up - when it is a local reference that is simply malformed, and saying
    /// so up front is more use than a window reporting that nothing opened.
    pub fn is_broken_file_uri(&self) -> bool {
        match self {
            Self::Remote(uri) => uri.to_ascii_lowercase().starts_with("file://"),
            Self::File(_) => false,
        }
    }

    /// What to call it on screen when nothing better is known. Kodi's library
    /// title is preferred over this wherever there is one.
    pub fn label(&self) -> String {
        match self {
            Self::File(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string_lossy().to_string()),
            // The last path segment, without any query string. For a Jellyfin
            // stream that is an opaque id rather than a name, which is ugly but
            // honest, and only ever seen when the launcher offered no title.
            Self::Remote(uri) => uri
                .split('?')
                .next()
                .unwrap_or(uri)
                .rsplit('/')
                .find(|segment| !segment.is_empty())
                .unwrap_or(uri)
                .to_string(),
        }
    }

    /// How this reads in the log: the name, and what was diagnostic about
    /// where it lived rather than where it lived.
    ///
    /// **The name is kept and the directory is not**, which is a deliberate
    /// split rather than caution applied evenly. A file name is load-bearing
    /// for a whole class of fault: the `&` in a path that broke Kodi's player
    /// file was a character bug, subtitles beside a video are matched by name
    /// convention, and "it will not play *this* file" is a large share of what
    /// a media player is ever asked. Strip names and the log still answers
    /// "it crashed" while losing "it does not work with this".
    ///
    /// The directory answers none of that and gives away a great deal - the
    /// account name, how somebody organises their library, what else is on the
    /// disk. What it *did* answer is kept in another form: whether the file is
    /// on a network share, which is a real source of trouble, and how long the
    /// whole path is, because Windows still has a limit and a path near it
    /// fails in ways that look like nothing else.
    ///
    /// A remote source keeps its address minus the query string, which is
    /// where a token would be - see `crate::logging`. There is no directory to
    /// drop: the path is the server's, not the viewer's.
    pub fn for_log(&self) -> String {
        match self {
            Self::File(path) => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| "(unnamed)".to_string());
                let whole = path.to_string_lossy();
                // Both spellings: Windows writes a UNC path `\\host\share`,
                // and a URI or a shell-typed one arrives with forward slashes.
                let network = whole.starts_with(r"\\") || whole.starts_with("//");
                format!(
                    "{name:?} ({} path, {} characters)",
                    match network {
                        true => "network",
                        false => "local",
                    },
                    whole.chars().count()
                )
            }
            Self::Remote(uri) => uri.split('?').next().unwrap_or(uri).to_string(),
        }
    }

    /// How saved positions and track choices are filed when nothing steadier
    /// is available.
    ///
    /// A local file keeps using its plain path, so positions saved by earlier
    /// versions still resolve. A remote source has only its URI, which is
    /// weaker: a Jellyfin URL carries an access token that changes when it is
    /// regenerated, and its entry is orphaned when it does. Launchers that can
    /// name something stabler are preferred - see `kodi::Item::key`.
    pub fn key(&self) -> String {
        match self {
            Self::File(path) => path.to_string_lossy().to_string(),
            Self::Remote(uri) => uri.clone(),
        }
    }
}
