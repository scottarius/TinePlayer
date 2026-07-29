//! Talking to the Kodi that launched us.
//!
//! Only ever active when `--kodi` was passed, which Kodi's
//! `playercorefactory.xml` does. It is never inferred: being on a television
//! and being launched by Kodi are separate facts, and guessing wrong would
//! change how the interface behaves for someone who just ran the player
//! themselves.
//!
//! Kodi exposes the same JSON-RPC API two ways: over HTTP, which is off by
//! default and demands a username and password, and over a plain TCP socket on
//! port 9090, which is on by default and bound to the loopback interface. We
//! are launched by Kodi on the same machine, so the socket is the one that
//! needs no setup from anybody. Messages are bare JSON objects written to the
//! socket; there is no HTTP framing.
//!
//! **Nothing here is allowed to fail loudly.** Every function returns an
//! `Option` or nothing at all, and a Kodi that is closed, busy or of the wrong
//! version simply means the player carries on with what it knows by itself.
//! Kodi is a nicety layered over local playback, never a dependency of it.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

const ADDRESS: &str = "127.0.0.1:9090";

/// Short enough that a Kodi which is not listening cannot delay startup
/// noticeably, long enough for a loopback round trip on a Pi.
const TIMEOUT: Duration = Duration::from_millis(1500);

/// How much of a video has to be watched before Kodi is told it was.
///
/// Kodi's own threshold is `playcountminimumpercent` in `advancedsettings.xml`,
/// which cannot be read over JSON-RPC — it is absent from `Settings.GetSettings`
/// even at expert level, because it is not a settings-menu item. This is Kodi's
/// default for it, so the two agree unless someone has changed theirs.
const WATCHED_PERCENT: f64 = 90.0;

/// What Kodi knows about a video that we do not.
pub struct Details {
    /// The library title, e.g. "28 Days" for a file named
    /// `28 Days (2000) WEBDL-1080p.mkv`. Empty when the file is not in the
    /// library, in which case the caller keeps using the file name.
    pub title: String,
    /// Where Kodi thinks playback stopped, in nanoseconds to match the rest of
    /// the player. `None` when Kodi has no resume point, which includes a
    /// video that was watched to the end.
    pub resume_ns: Option<u64>,
}

/// One JSON-RPC call, or `None` if anything at all goes wrong.
///
/// Deliberately opens a connection per call rather than holding one open: the
/// calls are seconds or minutes apart, and a socket held across a whole film is
/// a socket that can quietly die without anyone noticing.
fn call(method: &str, params: serde_json::Value) -> Option<serde_json::Value> {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });

    let address: SocketAddr = ADDRESS.parse().ok()?;
    let mut stream = TcpStream::connect_timeout(&address, TIMEOUT).ok()?;
    stream.set_read_timeout(Some(TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(TIMEOUT)).ok()?;
    stream.write_all(request.to_string().as_bytes()).ok()?;
    stream.flush().ok()?;

    // Kodi answers with one JSON object and then leaves the connection open
    // for more, so there is no end of stream to read to: reading until EOF
    // would wait out the timeout every single call. Instead the reply is
    // parsed as it arrives and the first complete object ends the read.
    //
    // An error object rather than a result is a perfectly ordinary answer from
    // a Kodi that is older, or does not have the file, and is not ours to
    // report — `get("result")` simply comes back empty.
    let mut reply = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return None,
            Ok(read) => {
                reply.extend_from_slice(&chunk[..read]);
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&reply) {
                    return value.get("result").cloned();
                }
            }
            Err(_) => return None,
        }
    }
}

/// Kodi's title and resume point for a file.
///
/// Keyed by the path Kodi gave us on the command line, which is the same path
/// it holds in its library, so nothing has to be looked up by database id.
pub fn details(file: &Path) -> Option<Details> {
    let result = call(
        "Files.GetFileDetails",
        serde_json::json!({
            "file": file.to_string_lossy(),
            "media": "video",
            "properties": ["title", "resume"],
        }),
    )?;
    let details = result.get("filedetails")?;

    // Seconds as a float on Kodi's side, nanoseconds on ours.
    let position = details
        .get("resume")
        .and_then(|resume| resume.get("position"))
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);

    Some(Details {
        title: details
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        resume_ns: (position > 0.0).then(|| (position * 1_000_000_000.0) as u64),
    })
}

/// Tells Kodi where playback has reached, so its library shows real progress.
///
/// Runs on a thread and reports nothing back: this is called while a film is
/// playing, and a stalled socket must never be able to stutter playback or the
/// interface. Past [`WATCHED_PERCENT`] the video is marked watched and its
/// resume point cleared, which is what Kodi's own player does — leaving a
/// resume point a few seconds from the end would offer to resume the credits.
pub fn report_position(file: &Path, position_ns: u64, duration_ns: u64) {
    if duration_ns == 0 {
        return;
    }
    let file = file.to_string_lossy().to_string();
    let position = position_ns as f64 / 1_000_000_000.0;
    let total = duration_ns as f64 / 1_000_000_000.0;
    let watched = position / total * 100.0 >= WATCHED_PERCENT;

    std::thread::spawn(move || {
        let params = if watched {
            serde_json::json!({
                "file": file,
                "media": "video",
                "playcount": 1,
                "resume": { "position": 0.0, "total": total },
            })
        } else {
            serde_json::json!({
                "file": file,
                "media": "video",
                "resume": { "position": position, "total": total },
            })
        };
        let _ = call("Files.SetFileDetails", params);
    });
}
