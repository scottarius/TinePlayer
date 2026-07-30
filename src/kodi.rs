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
use std::time::Duration;

const ADDRESS: &str = "127.0.0.1:9090";

/// Short enough that a Kodi which is not listening cannot delay startup
/// noticeably, long enough for a loopback round trip on a Pi.
const TIMEOUT: Duration = Duration::from_millis(1500);

/// How much of a video has to be watched before Kodi is told it was.
///
/// Kodi's own threshold is `playcountminimumpercent` in `advancedsettings.xml`,
/// which cannot be read over JSON-RPC - it is absent from `Settings.GetSettings`
/// even at expert level, because it is not a settings-menu item. This is Kodi's
/// default for it, so the two agree unless someone has changed theirs.
const WATCHED_PERCENT: f64 = 90.0;

/// The library item Kodi is playing through us.
///
/// Everything here comes from asking Kodi what it is playing, never from the
/// argument we were launched with. Kodi resolves a `plugin://` item to a real
/// stream URL before handing it over, so the argument is good for playback and
/// useless for identity - it is not what Kodi knows the item by, and looking it
/// up by that URL fails. Asking sidesteps the whole problem, and works the same
/// whether the item came from a local folder or an add-on.
pub struct Item {
    /// `movie`, `episode`, `musicvideo`, and so on.
    pub kind: String,
    /// Kodi's own database id, unique within `kind`.
    pub id: i64,
    /// The path Kodi knows the item by - a local path, or the `plugin://` URL
    /// an add-on resolved. Progress is written back against this, not against
    /// the URL we play.
    pub file: String,
    /// The library title, far better than a file name: "Avengers: Endgame"
    /// rather than `Avengers - Endgame (2019) Bluray-1080p.mkv`.
    pub title: String,
    /// Where Kodi thinks playback stopped, in nanoseconds to match the rest of
    /// the player. `None` when there is no resume point, which includes a video
    /// watched to the end.
    pub resume_ns: Option<u64>,
    /// Kodi's idea of the length, in seconds, used to check that the item it
    /// described is the one we are actually playing.
    pub runtime_s: u64,
}

impl Item {
    /// How this video's position and track choices are filed.
    ///
    /// Kodi's own id, rather than the URL we were handed. An add-on's stream
    /// URL can carry an access token that changes when it is regenerated, and
    /// the same film is a `plugin://` path to Kodi and an HTTP URL to us - so
    /// the URL is the one thing here that is not stable.
    ///
    /// Known limitation, accepted deliberately: these ids belong to Kodi's
    /// database, and anything that rebuilds the library renumbers them. A
    /// library-syncing add-on taking over - PlexKodiConnect and Jellyfin for
    /// Kodi both do, and cannot coexist - reassigns every id, after which an
    /// entry filed here points at whatever film inherited its number. The
    /// alternative was `uniqueid` (an IMDb or TMDB id), which survives that and
    /// matches the same film across sources, but is absent for anything the
    /// library has not identified.
    pub fn key(&self) -> String {
        format!("kodi:{}/{}", self.kind, self.id)
    }
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
    // report - `get("result")` simply comes back empty.
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

/// The item Kodi is playing through us, if it will say.
///
/// Kodi has exactly one video player slot - `GetActivePlayers` appends at most
/// one entry of type `video` - and it launched us and is blocked waiting for
/// this process to exit. So a single video player reporting itself `external`
/// is necessarily this playback. That is a structural guarantee rather than a
/// guess between candidates.
///
/// `playertype` is computed once for the whole reply and stamped onto every
/// entry, so an audio entry reads `external` too. Both fields have to be
/// checked together; reading `playertype` off the first entry would be wrong.
pub fn current_item() -> Option<Item> {
    // Kodi registers the player around the same moment it starts us, so a first
    // look can genuinely be too early. Cheap to look again; giving up here just
    // means falling back to local behavior for the whole session.
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(300));
        }
        let Some(players) = call("Player.GetActivePlayers", serde_json::json!({})) else {
            continue;
        };
        let Some(player) = players.as_array().and_then(|players| {
            players.iter().find(|player| {
                player.get("type").and_then(serde_json::Value::as_str) == Some("video")
                    && player.get("playertype").and_then(serde_json::Value::as_str)
                        == Some("external")
            })
        }) else {
            continue;
        };
        let id = player.get("playerid")?.as_i64()?;

        let result = call(
            "Player.GetItem",
            serde_json::json!({
                "playerid": id,
                "properties": ["file", "title", "resume", "runtime"],
            }),
        )?;
        let item = result.get("item")?;

        // Seconds as a float on Kodi's side, nanoseconds on ours.
        let position = item
            .get("resume")
            .and_then(|resume| resume.get("position"))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);

        let string = |field: &str| {
            item.get(field)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        };

        return Some(Item {
            kind: string("type"),
            id: item.get("id").and_then(serde_json::Value::as_i64)?,
            file: string("file"),
            title: string("title"),
            resume_ns: (position > 0.0).then_some((position * 1_000_000_000.0) as u64),
            runtime_s: item
                .get("runtime")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
        });
    }
    None
}

/// Tells Kodi where playback has reached, so its library shows real progress.
///
/// `file` is the path Kodi itself reported for the item, not the URL we were
/// launched with: for an add-on item those differ, and only Kodi's own form is
/// one it will accept back.
///
/// Runs on a thread and reports nothing back: this is called while a film is
/// playing, and a stalled socket must never be able to stutter playback or the
/// interface. Past [`WATCHED_PERCENT`] the video is marked watched and its
/// resume point cleared, which is what Kodi's own player does - leaving a
/// resume point a few seconds from the end would offer to resume the credits.
pub fn report_position(file: &str, position_ns: u64, duration_ns: u64) {
    if duration_ns == 0 {
        return;
    }
    let file = file.to_string();
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
