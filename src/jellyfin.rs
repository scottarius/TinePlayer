//! What TinePlayer remembers about a Jellyfin server it has been paired with.
//!
//! Kept out of `config.yaml` on purpose. The token below is a bearer
//! credential: anything holding it can read and stream the library as that
//! viewer. `config.yaml` is the file the documentation tells people to open
//! when something is wrong, and the one that gets copied to
//! `config.yaml.invalid` beside itself - so a token in it ends up in a bug
//! report sooner or later. A credential is also not a setting: nobody hand
//! edits this, and it is the one file worth keeping out of a support bundle.
//!
//! Not the operating system's keyring either, and the reason is what
//! TinePlayer is for rather than laziness. It often runs on a Pi wired to a
//! television with automatic login, where a keyring that wants unlocking after
//! a reboot means the machine comes up and is not on anybody's phone until
//! somebody finds a keyboard - and a headless Linux box may have no Secret
//! Service running at all. Windows and macOS would both manage it unattended;
//! Linux, which is the case that matters most here, would not.
//!
//! Obfuscating it would be theatre. Whatever TinePlayer can read unattended,
//! so can anything else running as that viewer. The honest answer is a file
//! only they can read, and documentation that says what is in it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// What TinePlayer calls itself to Jellyfin. This is shown in the viewer's
/// device list, so it is a name rather than an identifier.
const CLIENT: &str = "TinePlayer";

/// Long enough for a server waking a spun-down disk, short enough that one
/// which has gone away does not hold a worker thread for ever.
const TIMEOUT: u64 = 15;

/// A server this installation knows about, and the account it is signed in as
/// if it currently is.
///
/// The two are separate because they have different lifetimes. The device
/// identity outlives any number of pairings, while the account can be taken
/// away at any moment - revoked from Jellyfin's dashboard, or the user
/// deleted - and that is an ordinary thing for a viewer to do rather than an
/// error to defend against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pairing {
    /// Where the server is, as it was typed: `http://hoth:8096`.
    pub server: String,
    /// How Jellyfin knows this installation.
    ///
    /// Generated once and kept afterwards, including across a re-pairing.
    /// Jellyfin keys a session on it, so reusing it means connecting again
    /// replaces the existing device rather than leaving the viewer's device
    /// list full of one entry per attempt.
    pub device_id: String,
    /// Absent until Quick Connect has been approved, and absent again the
    /// moment the server stops accepting the token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<Account>,
}

/// Who TinePlayer is signed in as, and the token that says so.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    /// The bearer token. Everything in this file exists to protect this line.
    pub token: String,
    /// Jellyfin wants this outright on several endpoints rather than inferring
    /// it from the token, so it is kept rather than asked for each time.
    pub user_id: String,
    /// Only to show on screen: "Connected as scottarius".
    pub user_name: String,
}

impl Pairing {
    /// A server that has been named but not yet connected to.
    pub fn new(server: &str) -> Self {
        Self {
            server: normalize(server),
            device_id: new_device_id(),
            account: None,
        }
    }

    /// Whether there is a token to try. Not whether it still works - only the
    /// server can answer that, by refusing it.
    pub fn is_connected(&self) -> bool {
        self.account.is_some()
    }

    /// Drops the credentials and keeps the identity.
    ///
    /// What a 401 should lead to. The pairing is gone, but the server address
    /// is still the one the viewer chose and the device id must survive so
    /// that connecting again replaces the old device rather than adding to it.
    pub fn sign_out(&mut self) {
        self.account = None;
    }
}

/// A fresh identity for this installation.
///
/// `g_uuid_string_random` rather than anything of our own: it is already
/// linked, and an identifier that has to be unique across every device on
/// somebody's server is not a thing to improvise.
fn new_device_id() -> String {
    glib::uuid_string_random().to_string()
}

/// A server address without the trailing slash Jellyfin does not want.
///
/// Every URL is built by joining paths onto this, and `http://host:8096/` with
/// `/Sessions` after it is a double slash that some proxies answer differently
/// from the path that was meant.
fn normalize(server: &str) -> String {
    server.trim().trim_end_matches('/').to_string()
}

pub fn path() -> PathBuf {
    crate::config::jellyfin_path()
}

/// What is remembered, or nothing.
///
/// A file that cannot be read or understood is treated as no pairing at all.
/// The worst that costs is connecting again, which is a six-character code;
/// refusing to start over a damaged credentials file would be worse.
pub fn load() -> Option<Pairing> {
    let text = std::fs::read_to_string(path()).ok()?;
    match serde_json::from_str(&text) {
        Ok(pairing) => Some(pairing),
        Err(e) => {
            eprintln!("Ignoring an unreadable Jellyfin pairing: {e}");
            None
        }
    }
}

/// Writes it, readable only by this account where the platform can say so.
///
/// The mode is set as the file is created rather than afterwards, so there is
/// no moment when it exists and is readable by everyone. On Windows this
/// inherits the profile folder's own permissions, which already exclude other
/// accounts; a portable copy on a memory stick has no permissions to set at
/// all, which is worth knowing rather than working around - a lost stick is a
/// leaked credential, and that is the deal a portable install makes.
pub fn save(pairing: &Pairing) -> Result<(), String> {
    let path = path();
    let text = serde_json::to_string_pretty(pairing).map_err(|e| e.to_string())?;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    use std::io::Write;
    let mut file = options
        .open(&path)
        .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|e| format!("Failed to write {}: {e}", path.display()))
}

/// Forgets the server entirely, which is what disconnecting means.
///
/// Removing the file rather than emptying it: an absent file is how the rest
/// of TinePlayer knows Jellyfin was never set up, and leaving an empty one
/// behind would be a different state that means the same thing.
pub fn remove() -> Result<(), String> {
    match std::fs::remove_file(path()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// What went wrong, in the only two flavours a caller acts on differently.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The server refused the token. The pairing is gone - revoked from the
    /// dashboard, the device deleted, or the user removed - and no amount of
    /// retrying will bring it back. Callers sign out and offer to pair again.
    Unauthorized,
    /// Anything else: the server is down, the network is out, the reply made
    /// no sense. Worth retrying, and worth saying out loud.
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized => write!(f, "Jellyfin no longer accepts this connection"),
            Self::Failed(why) => write!(f, "{why}"),
        }
    }
}

/// The header every request carries.
///
/// Jellyfin reads the client, device and version out of it and shows them in
/// the viewer's device list, which is what makes TinePlayer identifiable there
/// rather than appearing as an anonymous session.
fn authorization(device_id: &str, token: Option<&str>) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let device = hostname();
    let mut header = format!("MediaBrowser Client=\"{CLIENT}\"");
    header.push_str(&format!(", Device=\"{device}\""));
    header.push_str(&format!(", DeviceId=\"{device_id}\""));
    header.push_str(&format!(", Version=\"{version}\""));
    if let Some(token) = token {
        header.push_str(&format!(", Token=\"{token}\""));
    }
    header
}

/// What to call this machine in the viewer's device list.
///
/// The computer's own name, because that is what somebody scanning the list
/// will recognise. Quotes and backslashes are dropped rather than escaped:
/// the value sits inside a quoted field in the header above, and a machine
/// named with one would end that field early.
fn hostname() -> String {
    glib::host_name().replace(['"', '\\'], "")
}

fn failed(code: i32, body: &str) -> Error {
    match code {
        401 | 403 => Error::Unauthorized,
        _ => Error::Failed(format!("Jellyfin answered {code}: {}", body.trim())),
    }
}

/// A server that has been paired with, and the calls made against it.
///
/// Holds what it needs rather than borrowing the pairing, because every call
/// here happens on a worker thread while the interface carries on.
#[derive(Debug, Clone)]
pub struct Client {
    server: String,
    device_id: String,
    token: String,
    user_id: String,
}

impl Client {
    /// `None` when the pairing has no account, which is the state a 401 leaves
    /// it in. Callers reach for Quick Connect instead of pretending.
    pub fn new(pairing: &Pairing) -> Option<Self> {
        let account = pairing.account.as_ref()?;
        Some(Self {
            server: pairing.server.clone(),
            device_id: pairing.device_id.clone(),
            token: account.token.clone(),
            user_id: account.user_id.clone(),
        })
    }

    fn get(&self, path: &str) -> Result<serde_json::Value, Error> {
        let response = minreq::get(format!("{}{path}", self.server))
            .with_header(
                "Authorization",
                authorization(&self.device_id, Some(&self.token)),
            )
            .with_timeout(TIMEOUT)
            .send()
            .map_err(|e| Error::Failed(e.to_string()))?;
        let body = response.as_str().unwrap_or_default().to_string();
        if response.status_code != 200 {
            return Err(failed(response.status_code, &body));
        }
        serde_json::from_str(&body).map_err(|e| Error::Failed(e.to_string()))
    }

    /// A POST whose reply is not wanted, which is all of them here: Jellyfin
    /// answers the reporting endpoints with 204 and no body.
    fn post(&self, path: &str, body: serde_json::Value) -> Result<(), Error> {
        let response = minreq::post(format!("{}{path}", self.server))
            .with_header(
                "Authorization",
                authorization(&self.device_id, Some(&self.token)),
            )
            .with_header("Content-Type", "application/json")
            .with_timeout(TIMEOUT)
            .with_body(body.to_string())
            .send()
            .map_err(|e| Error::Failed(e.to_string()))?;
        match response.status_code {
            200..=299 => Ok(()),
            code => Err(failed(code, response.as_str().unwrap_or_default())),
        }
    }

    /// Says what TinePlayer can be asked to do.
    ///
    /// Only `GeneralCommandType` values belong here. Pause, Stop and Seek are
    /// playstate commands, arrive by a different message, and are refused with
    /// a 400 naming the offending entry - which is how that was found out.
    ///
    /// Declaring this is not what makes TinePlayer castable. Measured
    /// 2026-08-13: the session still reported `SupportsRemoteControl: false`
    /// afterwards, and turned true only once a socket was open.
    pub fn announce(&self) -> Result<(), Error> {
        self.post(
            "/Sessions/Capabilities/Full",
            serde_json::json!({
                "PlayableMediaTypes": ["Video"],
                // Only what is actually acted on. A declared command is a
                // promise: a controller offers a button for each one, and a
                // button that does nothing is worse than one that is not
                // there. Volume was the clearest case - TinePlayer has a level
                // per output and Jellyfin offers one slider, so there is no
                // honest answer to give it - and the track selectors were
                // offered without anything behind them at all.
                "SupportedCommands": ["Play", "PlayState"],
                "SupportsMediaControl": true,
                "SupportsPersistentIdentifier": true,
            }),
        )
    }

    /// What is known about one item, in the units TinePlayer counts in.
    pub fn item(&self, id: &str) -> Result<Item, Error> {
        let body = self.get(&format!("/Users/{}/Items/{id}", self.user_id))?;
        let text = |name: &str| {
            body.get(name)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let number = |name: &str| {
            body.get(name)
                .and_then(|value| value.as_u64())
                .map(|value| value as u32)
        };
        // The first media source, which is the file itself for anything that
        // has not been given a second version.
        let source = body
            .get("MediaSources")
            .and_then(|sources| sources.as_array())
            .and_then(|sources| sources.first());
        Ok(Item {
            id: id.to_string(),
            title: text("Name"),
            runtime_ns: body.get("RunTimeTicks").and_then(from_ticks),
            resume_ns: body
                .get("UserData")
                .and_then(|data| data.get("PlaybackPositionTicks"))
                .and_then(from_ticks)
                .filter(|position| *position > 0),
            plot: text("Overview"),
            year: number("ProductionYear"),
            certificate: text("OfficialRating"),
            rating: body.get("CommunityRating").and_then(|value| value.as_f64()),
            genres: body
                .get("Genres")
                .and_then(|genres| genres.as_array())
                .map(|genres| {
                    genres
                        .iter()
                        .filter_map(|genre| genre.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            // Both or neither: a season without an episode is not something
            // the page can say anything useful about.
            episode: number("ParentIndexNumber").zip(number("IndexNumber")),
            // The date arrives as a full timestamp and the page wants a day.
            aired: text("PremiereDate")
                .split('T')
                .next()
                .unwrap_or_default()
                .to_string(),
            poster_tag: body
                .get("ImageTags")
                .and_then(|tags| tags.get("Primary"))
                .and_then(|tag| tag.as_str())
                .map(str::to_string),
            backdrop_tag: body
                .get("BackdropImageTags")
                .and_then(|tags| tags.as_array())
                .and_then(|tags| tags.first())
                .and_then(|tag| tag.as_str())
                .map(str::to_string),
            container: source
                .and_then(|source| source.get("Container"))
                .and_then(|container| container.as_str())
                // Belt and braces: if this ever arrives as a list too, the
                // first name in it is the one to use.
                .and_then(|container| container.split(',').next())
                .unwrap_or_default()
                .to_string(),
            media_source_id: source
                .and_then(|source| source.get("Id"))
                .and_then(|id| id.as_str())
                .unwrap_or(id)
                .to_string(),
            streams: source.map(read_streams).unwrap_or_default(),
        })
    }

    /// One picture, as bytes ready to decode.
    ///
    /// Asked for at a size rather than whole: a library's backdrop can be
    /// several megabytes at full resolution, and the page draws it behind
    /// text at the width of a screen. The tag is what makes the answer
    /// cacheable, and quoting the wrong one gets a picture from before the
    /// artwork was changed.
    pub fn image(&self, id: &str, kind: &str, tag: &str, width: u32) -> Result<Vec<u8>, Error> {
        let response = minreq::get(format!(
            "{}/Items/{id}/Images/{kind}?tag={tag}&maxWidth={width}&api_key={}",
            self.server, self.token
        ))
        .with_timeout(TIMEOUT)
        .send()
        .map_err(|e| Error::Failed(e.to_string()))?;
        match response.status_code {
            200 => Ok(response.into_bytes()),
            code => Err(failed(code, "")),
        }
    }

    /// The original file, not a transcode.
    ///
    /// `static=true` is the whole feature. Without it Jellyfin re-encodes and
    /// delivers a single audio track, and TinePlayer exists to play two at
    /// once. Verified 2026-08-13 against a ten-track film, which arrived whole
    /// and with its track names intact - the names matter as much as the
    /// count, since described tracks are found by reading them.
    ///
    /// The token rides in the query string because GStreamer opens this URL
    /// itself and carries no headers of ours.
    ///
    /// The container goes on the end as an extension, and that is not
    /// cosmetic. Asked for as a bare `/stream`, Jellyfin answered a QuickTime
    /// file with `Content-Type: video/x-msvideo` - AVI's type - and GStreamer
    /// believed it, chose the AVI demuxer, and sat waiting for structures that
    /// were never coming. Every Harry Potter film in the library failed that
    /// way on 2026-08-14 while the Matroska ones were fine, which is what made
    /// it look like a problem with particular titles. With `.mov` on the end
    /// the same request answers `video/quicktime`. Matroska is unaffected
    /// either way, so this costs nothing where it was already working.
    pub fn stream_url(&self, item: &Item) -> String {
        let id = &item.id;
        let extension = match item.container.is_empty() {
            true => String::new(),
            false => format!(".{}", item.container),
        };
        format!(
            "{}/Videos/{id}/stream{extension}?static=true&mediaSourceId={}&api_key={}",
            self.server, item.media_source_id, self.token
        )
    }

    /// Says a video has started, which is what puts the transport controls
    /// on somebody's phone.
    ///
    /// The item id alone is not enough, and the shortfall is silent: Jellyfin
    /// answers 204 either way, and the session simply keeps `NowPlayingItem:
    /// null` so a controller has nothing to control. Measured 2026-08-14 by
    /// posting both and reading the session back. What it wants besides the
    /// item is the media source, a play session id, and to be told the player
    /// can seek - without which the controller offers no scrubber even when it
    /// does appear.
    ///
    /// `play_session` ties the three reports together as one viewing, so it
    /// must be the same string for started, progress and stopped.
    pub fn started(&self, id: &str, play_session: &str, position_ns: u64) -> Result<(), Error> {
        self.post(
            "/Sessions/Playing",
            serde_json::json!({
                "ItemId": id,
                // Direct play, so the source is the item itself.
                "MediaSourceId": id,
                "PlaySessionId": play_session,
                "PlayMethod": "DirectPlay",
                "PositionTicks": to_ticks(position_ns),
                "CanSeek": true,
                "IsPaused": false,
                "IsMuted": false,
            }),
        )
    }

    pub fn progress(
        &self,
        id: &str,
        play_session: &str,
        position_ns: u64,
        paused: bool,
    ) -> Result<(), Error> {
        self.post(
            "/Sessions/Playing/Progress",
            serde_json::json!({
                "ItemId": id,
                "MediaSourceId": id,
                "PlaySessionId": play_session,
                "PlayMethod": "DirectPlay",
                "PositionTicks": to_ticks(position_ns),
                "CanSeek": true,
                "IsPaused": paused,
                "IsMuted": false,
            }),
        )
    }

    pub fn stopped(&self, id: &str, play_session: &str, position_ns: u64) -> Result<(), Error> {
        self.post(
            "/Sessions/Playing/Stopped",
            serde_json::json!({
                "ItemId": id,
                "MediaSourceId": id,
                "PlaySessionId": play_session,
                "PositionTicks": to_ticks(position_ns),
            }),
        )
    }

    /// A name for one viewing, tying its three reports together.
    pub fn new_play_session() -> String {
        glib::uuid_string_random().to_string()
    }
}

/// A video as Jellyfin describes it, converted into what TinePlayer counts in.
///
/// Everything the media page draws, because a cast video has no sidecar beside
/// it and a stream's container tags are thin - without this it arrives with a
/// title and nothing else. The pictures are deliberately not here: this is
/// cloned on every progress report, and megabytes of artwork going round every
/// ten seconds would be a poor trade for tidiness. Their tags are, which is
/// what asking for them needs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Item {
    pub id: String,
    /// The library title, which is the whole reason to ask: "Avengers:
    /// Endgame" rather than the file name.
    pub title: String,
    pub runtime_ns: Option<u64>,
    /// Where this viewer stopped, or `None` for one they have not started -
    /// which includes a video watched to the end.
    pub resume_ns: Option<u64>,
    pub plot: String,
    pub year: Option<u32>,
    /// Already the short form Jellyfin stores - "PG-13" - which is the form
    /// the page wants.
    pub certificate: String,
    /// Out of ten, as the page shows it.
    pub rating: Option<f64>,
    pub genres: Vec<String>,
    /// Season and episode, for something that has them. Also how the page
    /// tells an episode from a film.
    pub episode: Option<(u32, u32)>,
    /// The day it first went out, for an episode.
    pub aired: String,
    /// What to quote when asking for each picture. Absent when the library has
    /// none, which is how not to ask for one that is not there.
    pub poster_tag: Option<String>,
    pub backdrop_tag: Option<String>,
    /// Every stream the library found, which is what spares TinePlayer from
    /// reading a four-gigabyte file across the house to learn the same thing.
    pub streams: Streams,
    /// The container, as the media source names it: `mkv`, `mov`.
    ///
    /// Taken from the media source rather than the item's own `Container`,
    /// which is ffmpeg's list of everything the format could be called -
    /// `mov,mp4,m4a,3gp,3g2,mj2` - and no use as an extension.
    pub container: String,
    /// Which source to stream, for an item that has more than one.
    pub media_source_id: String,
}

/// Jellyfin counts in ticks of a hundred nanoseconds and TinePlayer counts in
/// nanoseconds. Converted here and nowhere else, so the two cannot drift.
fn from_ticks(value: &serde_json::Value) -> Option<u64> {
    value.as_u64().map(|ticks| ticks * 100)
}

fn to_ticks(nanoseconds: u64) -> u64 {
    nanoseconds / 100
}

/// One stream of a video, as the library analysed it.
///
/// Kept close to what Jellyfin says rather than converted on the way in, so
/// that the two indices below cannot be confused with each other.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Stream {
    /// Jellyfin's own numbering, across every stream in the item. This is what
    /// asking for an external file quotes, and it is *not* a position among
    /// the audio tracks - an external subtitle sits at 0, before the video.
    pub index: u32,
    /// Whether it is a file beside the video rather than part of it.
    ///
    /// The whole design turns on this. An external stream is not in the
    /// container, so it can never be selected by position within it, and
    /// counting it would push every embedded track after it out of step -
    /// silently, which is the worst way to be wrong about which soundtrack
    /// somebody is listening to.
    pub external: bool,
    pub codec: String,
    pub language: String,
    pub title: String,
    pub channels: u32,
}

/// What Jellyfin knows about how a video is put together.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Streams {
    pub audio: Vec<Stream>,
    pub subtitles: Vec<Stream>,
    pub width: u32,
    pub height: u32,
    pub video_codec: String,
    pub fps: f64,
}

/// The codec name as a viewer would say it.
///
/// Jellyfin answers in short machine names - `eac3`, `h264` - where the probe
/// used to hand back what `pb_utils` calls them. The page shows these, so a
/// video that opened one way yesterday should not read differently today just
/// because the answer came from somewhere else.
fn codec_name(codec: &str) -> String {
    match codec.to_ascii_lowercase().as_str() {
        "aac" => "MPEG-4 AAC",
        "ac3" => "AC-3 (ATSC A/52)",
        "eac3" => "E-AC-3 (ATSC A/52B)",
        "dts" => "DTS",
        "truehd" => "Dolby TrueHD",
        "flac" => "FLAC",
        "opus" => "Opus",
        "vorbis" => "Vorbis",
        "mp3" => "MPEG-1 Layer 3 (MP3)",
        "mp2" => "MPEG-1 Layer 2",
        "pcm" | "pcm_s16le" | "pcm_s24le" => "Uncompressed PCM",
        "h264" => "H.264",
        "hevc" | "h265" => "H.265",
        "av1" => "AV1",
        "vp9" => "VP9",
        "vp8" => "VP8",
        "mpeg2video" => "MPEG-2",
        "" => return String::new(),
        other => return other.to_ascii_uppercase(),
    }
    .to_string()
}

impl Streams {
    /// What the probe would have found, without asking the file.
    ///
    /// Only the embedded streams are counted, and their positions are their
    /// positions among the embedded ones - which is exactly how the pipeline
    /// selects them. External files are left out entirely here and offered
    /// separately, because they are not in the container to be selected.
    ///
    /// Verified against a ten-track film on 2026-08-14: Jellyfin's order and
    /// the probe's order were identical, track for track.
    pub fn as_media(&self, duration_ns: u64) -> crate::probe::Media {
        let audio = self
            .audio
            .iter()
            .filter(|stream| !stream.external)
            .enumerate()
            .map(|(position, stream)| crate::probe::AudioTrack {
                index: position as u32,
                codec: codec_name(&stream.codec),
                channels: stream.channels,
                language: stream.language.clone(),
                title: stream.title.clone(),
            })
            .collect();

        let subtitles = self
            .subtitles
            .iter()
            .filter(|stream| !stream.external)
            .enumerate()
            .map(|(position, stream)| crate::probe::SubtitleTrack {
                index: position as u32,
                language: stream.language.clone(),
                title: stream.title.clone(),
                // Jellyfin carries a forced flag, but the rest of TinePlayer
                // reads forcedness out of the title as well, so nothing is
                // lost by leaving this to the same reading everything else
                // gets - see `subtitles::Subtitle::is_forced`.
                forced: false,
            })
            .collect();

        crate::probe::Media {
            audio,
            subtitles,
            duration_ns,
            video: crate::probe::VideoDetails {
                width: self.width,
                height: self.height,
                codec: codec_name(&self.video_codec),
                fps: self.fps,
            },
            // Left empty on purpose. These are the container's own tags, and
            // everything they would have offered - title, year, summary - the
            // library has already answered better.
            tags: crate::probe::Tags::default(),
        }
    }

    /// The streams that are files of their own, which have to be fetched
    /// rather than selected.
    pub fn external_audio(&self) -> impl Iterator<Item = &Stream> {
        self.audio.iter().filter(|stream| stream.external)
    }

    pub fn external_subtitles(&self) -> impl Iterator<Item = &Stream> {
        self.subtitles.iter().filter(|stream| stream.external)
    }
}

/// Reads the streams out of a media source.
fn read_streams(source: &serde_json::Value) -> Streams {
    let mut streams = Streams::default();
    let Some(list) = source.get("MediaStreams").and_then(|list| list.as_array()) else {
        return streams;
    };
    for entry in list {
        let text = |name: &str| {
            entry
                .get(name)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let number = |name: &str| {
            entry
                .get(name)
                .and_then(|value| value.as_u64())
                .unwrap_or_default() as u32
        };
        let stream = Stream {
            index: number("Index"),
            external: entry
                .get("IsExternal")
                .and_then(|external| external.as_bool())
                .unwrap_or(false),
            codec: text("Codec"),
            language: text("Language"),
            // `Title` is what a viewer named the track; `DisplayTitle` is what
            // Jellyfin assembles when they did not. The first is better where
            // it exists, and it is what the described-track detection reads.
            title: match text("Title").is_empty() {
                true => text("DisplayTitle"),
                false => text("Title"),
            },
            channels: number("Channels"),
        };
        match text("Type").as_str() {
            "Audio" => streams.audio.push(stream),
            "Subtitle" => streams.subtitles.push(stream),
            "Video" if streams.width == 0 => {
                streams.width = number("Width");
                streams.height = number("Height");
                streams.video_codec = stream.codec;
                streams.fps = entry
                    .get("RealFrameRate")
                    .or_else(|| entry.get("AverageFrameRate"))
                    .and_then(|rate| rate.as_f64())
                    .unwrap_or(0.0);
            }
            _ => {}
        }
    }
    streams
}

/// A pairing part-way through: the code to show, and the secret that redeems
/// it once somebody has approved it.
#[derive(Debug, Clone)]
pub struct QuickConnect {
    /// Six characters for the viewer to type into a Jellyfin app they are
    /// already signed into. This is the whole reason no password is ever asked
    /// for here.
    pub code: String,
    secret: String,
}

/// Asks the server to start a pairing.
///
/// Quick Connect can be switched off by an administrator, in which case this
/// fails and there is nothing to do but say so plainly.
pub fn quick_connect_start(server: &str, device_id: &str) -> Result<QuickConnect, Error> {
    let response = minreq::post(format!("{}/QuickConnect/Initiate", normalize(server)))
        .with_header("Authorization", authorization(device_id, None))
        .with_timeout(TIMEOUT)
        .send()
        .map_err(|e| Error::Failed(e.to_string()))?;
    if response.status_code != 200 {
        return Err(Error::Failed(format!(
            "This server would not start a Quick Connect pairing ({}). It may be turned off.",
            response.status_code
        )));
    }
    let body: serde_json::Value = response.json().map_err(|e| Error::Failed(e.to_string()))?;
    match (
        body.get("Code").and_then(|code| code.as_str()),
        body.get("Secret").and_then(|secret| secret.as_str()),
    ) {
        (Some(code), Some(secret)) => Ok(QuickConnect {
            code: code.to_string(),
            secret: secret.to_string(),
        }),
        _ => Err(Error::Failed(
            "The server's reply made no sense".to_string(),
        )),
    }
}

/// Asks whether the code has been approved, and takes the token if it has.
///
/// `Ok(None)` means "not yet", which is the ordinary answer while somebody
/// finds their phone, so callers poll on this rather than blocking.
pub fn quick_connect_poll(
    server: &str,
    device_id: &str,
    pending: &QuickConnect,
) -> Result<Option<Account>, Error> {
    let server = normalize(server);
    let response = minreq::get(format!(
        "{server}/QuickConnect/Connect?secret={}",
        pending.secret
    ))
    .with_header("Authorization", authorization(device_id, None))
    .with_timeout(TIMEOUT)
    .send()
    .map_err(|e| Error::Failed(e.to_string()))?;
    if response.status_code != 200 {
        // Expired or denied. Not worth retrying: the viewer needs a new code.
        return Err(Error::Failed(
            "That code is no longer valid. Ask for another.".to_string(),
        ));
    }
    let body: serde_json::Value = response.json().map_err(|e| Error::Failed(e.to_string()))?;
    if body.get("Authenticated").and_then(|done| done.as_bool()) != Some(true) {
        return Ok(None);
    }

    let response = minreq::post(format!("{server}/Users/AuthenticateWithQuickConnect"))
        .with_header("Authorization", authorization(device_id, None))
        .with_header("Content-Type", "application/json")
        .with_timeout(TIMEOUT)
        .with_body(serde_json::json!({ "Secret": pending.secret }).to_string())
        .send()
        .map_err(|e| Error::Failed(e.to_string()))?;
    if response.status_code != 200 {
        return Err(failed(
            response.status_code,
            response.as_str().unwrap_or_default(),
        ));
    }
    let body: serde_json::Value = response.json().map_err(|e| Error::Failed(e.to_string()))?;
    let token = body
        .get("AccessToken")
        .and_then(|token| token.as_str())
        .ok_or_else(|| Error::Failed("The server sent no token".to_string()))?;
    let user = body.get("User");
    let field = |name: &str| {
        user.and_then(|user| user.get(name))
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    };
    Ok(Some(Account {
        token: token.to_string(),
        user_id: field("Id"),
        user_name: field("Name"),
    }))
}

/// What a controller asked for, once it has been turned into something
/// TinePlayer understands.
///
/// Jellyfin sends rather more than this. What is not here is not ignored by
/// accident: volume and mute belong to the outputs and are better left to the
/// person in the room, and the queue commands mean nothing to a player that
/// shows one video at a time.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Play this item. The position is Jellyfin's, which is not always the one
    /// stored against the item - a controller can say "play from here".
    Play {
        item_id: String,
        position_ns: Option<u64>,
    },
    Pause,
    Unpause,
    /// Whichever of the two the current state is not.
    PlayPause,
    Stop,
    Seek(u64),
    /// The server refused the token, so the pairing is gone. Sign out and ask
    /// to be paired again; retrying cannot help.
    SignedOut,
}

/// A live connection to a server, and the thread holding it.
///
/// Dropping this closes the socket and lets the thread end, which is how
/// disconnecting works: there is no stop flag to get out of step with.
pub struct Session {
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.running
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

/// What acts on a command, once it has reached the thread that can.
type Handler = std::rc::Rc<dyn Fn(Command)>;

thread_local! {
    /// What to do with a command, on the thread that can act on it.
    ///
    /// Held here for the same reason `media_keys` does it: the handler touches
    /// the interface and cannot be sent anywhere, while the socket lives on a
    /// thread of its own. Commands cross by `idle_add_once`, which runs them
    /// on the main loop, and the closure looks the handler up when it gets
    /// there rather than carrying it.
    static HANDLER: std::cell::RefCell<Option<Handler>> =
        const { std::cell::RefCell::new(None) };
}

/// Opens the connection and keeps it open.
///
/// This is what makes TinePlayer appear on somebody's phone. Measured
/// 2026-08-13: declaring capabilities is not enough on its own - the session
/// reported `SupportsRemoteControl: false` until a socket was open, and said
/// so again when it closed. So a dropped connection is not a background
/// nuisance to be retried quietly; while it is down TinePlayer is not there to
/// be cast to at all.
///
/// Retried with a widening gap, because a server being restarted is ordinary.
/// The one thing not retried is a refused token: that is the pairing being
/// revoked, and no amount of waiting brings it back.
pub fn connect(pairing: &Pairing, handler: impl Fn(Command) + 'static) -> Option<Session> {
    let account = pairing.account.as_ref()?;
    HANDLER.with(|held| *held.borrow_mut() = Some(std::rc::Rc::new(handler)));

    let url = socket_url(&pairing.server, &account.token, &pairing.device_id);
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let alive = running.clone();

    std::thread::Builder::new()
        .name("jellyfin".to_string())
        .spawn(move || {
            let mut wait = std::time::Duration::from_secs(1);
            while alive.load(std::sync::atomic::Ordering::Relaxed) {
                match hold(&url, &alive) {
                    // Refused outright. The pairing is gone; stop.
                    Err(Error::Unauthorized) => {
                        deliver(Command::SignedOut);
                        return;
                    }
                    Err(Error::Failed(why)) => {
                        eprintln!("Jellyfin connection lost: {why}");
                    }
                    Ok(()) => {}
                }
                if !alive.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(wait);
                // Up to half a minute: long enough not to hammer a server that
                // is down, short enough that somebody who restarts theirs does
                // not wait long for the television to come back.
                wait = (wait * 2).min(std::time::Duration::from_secs(30));
            }
        })
        .ok()?;

    Some(Session { running })
}

/// Where the socket lives, derived from the address the viewer gave.
///
/// `http` becomes `ws` and `https` becomes `wss`, so a server reached securely
/// keeps its socket secure rather than quietly falling back.
fn socket_url(server: &str, token: &str, device_id: &str) -> String {
    let base = match server.strip_prefix("https://") {
        Some(rest) => format!("wss://{rest}"),
        None => match server.strip_prefix("http://") {
            Some(rest) => format!("ws://{rest}"),
            None => format!("ws://{server}"),
        },
    };
    format!("{base}/socket?api_key={token}&deviceId={device_id}")
}

/// One connection, held until it closes or is told to stop.
fn hold(url: &str, alive: &std::sync::atomic::AtomicBool) -> Result<(), Error> {
    let (mut socket, response) = match tungstenite::connect(url) {
        Ok(pair) => pair,
        Err(tungstenite::Error::Http(response)) if response.status() == 401 => {
            return Err(Error::Unauthorized);
        }
        Err(e) => return Err(Error::Failed(e.to_string())),
    };
    if response.status() == 401 {
        return Err(Error::Unauthorized);
    }

    // Non-blocking so the loop can notice it has been asked to stop, rather
    // than sitting in a read until the server happens to say something.
    if let tungstenite::stream::MaybeTlsStream::Plain(stream) = socket.get_ref() {
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(1)));
    }

    while alive.load(std::sync::atomic::Ordering::Relaxed) {
        match socket.read() {
            Ok(tungstenite::Message::Text(text)) => {
                if let Some(command) = interpret(&text) {
                    deliver(command);
                }
            }
            Ok(tungstenite::Message::Ping(payload)) => {
                let _ = socket.send(tungstenite::Message::Pong(payload));
            }
            Ok(tungstenite::Message::Close(_)) => return Ok(()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                // Nothing said in the last second, which is the usual case.
                continue;
            }
            Err(e) => return Err(Error::Failed(e.to_string())),
        }
    }
    let _ = socket.close(None);
    Ok(())
}

/// Hands a command to the thread that can act on it.
fn deliver(command: Command) {
    glib::idle_add_once(move || {
        let handler = HANDLER.with(|held| held.borrow().clone());
        if let Some(handler) = handler {
            handler(command);
        }
    });
}

/// Turns one message from the server into a command, or nothing.
///
/// Written against what the server actually sent on 2026-08-13 rather than
/// against the documentation: a Play carries `ItemIds`, a `PlayCommand` and
/// the id of whoever is controlling, and nothing else - no stream address and
/// no position. Everything else about the video is asked for afterwards.
fn interpret(text: &str) -> Option<Command> {
    let message: serde_json::Value = serde_json::from_str(text).ok()?;
    let data = message.get("Data");
    match message.get("MessageType")?.as_str()? {
        "Play" => {
            let data = data?;
            // Queue commands are answered as "play this now": TinePlayer shows
            // one video at a time, and refusing outright would look broken to
            // somebody who pressed a button and watched nothing happen.
            let item_id = data
                .get("ItemIds")?
                .as_array()?
                .first()?
                .as_str()?
                .to_string();
            Some(Command::Play {
                item_id,
                position_ns: data.get("StartPositionTicks").and_then(from_ticks),
            })
        }
        "Playstate" => {
            let data = data?;
            match data.get("Command")?.as_str()? {
                "Pause" => Some(Command::Pause),
                "Unpause" => Some(Command::Unpause),
                "PlayPause" => Some(Command::PlayPause),
                "Stop" => Some(Command::Stop),
                "Seek" => Some(Command::Seek(
                    data.get("SeekPositionTicks").and_then(from_ticks)?,
                )),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> Account {
        Account {
            token: "2a2e92aba1".to_string(),
            user_id: "2c9fab11dbcd437cac37de2e54453b28".to_string(),
            user_name: "scottarius".to_string(),
        }
    }

    #[test]
    fn survives_a_round_trip() {
        let mut pairing = Pairing::new("http://hoth:8096");
        pairing.account = Some(account());
        let text = serde_json::to_string(&pairing).unwrap();
        assert_eq!(serde_json::from_str::<Pairing>(&text).unwrap(), pairing);
    }

    #[test]
    fn reads_a_pairing_that_was_never_connected() {
        let pairing = Pairing::new("http://hoth:8096");
        assert!(!pairing.is_connected());
        let text = serde_json::to_string(&pairing).unwrap();
        // The absent account is left out rather than written as null, so the
        // file says nothing about credentials until there are some.
        assert!(!text.contains("account"));
        assert_eq!(serde_json::from_str::<Pairing>(&text).unwrap(), pairing);
    }

    #[test]
    fn signing_out_keeps_the_identity() {
        let mut pairing = Pairing::new("http://hoth:8096");
        pairing.account = Some(account());
        let device_id = pairing.device_id.clone();

        pairing.sign_out();

        assert!(!pairing.is_connected());
        assert_eq!(pairing.device_id, device_id, "device id must survive a 401");
        assert_eq!(pairing.server, "http://hoth:8096");
    }

    #[test]
    fn each_installation_gets_its_own_identity() {
        assert_ne!(
            Pairing::new("http://hoth:8096").device_id,
            Pairing::new("http://hoth:8096").device_id
        );
    }

    #[test]
    fn a_trailing_slash_does_not_survive() {
        assert_eq!(Pairing::new("http://hoth:8096/").server, "http://hoth:8096");
        assert_eq!(
            Pairing::new("  http://hoth:8096  ").server,
            "http://hoth:8096"
        );
    }

    #[test]
    fn ticks_and_nanoseconds_agree() {
        // Ten minutes, the way each side counts it.
        let ten_minutes_ns = 600 * 1_000_000_000u64;
        assert_eq!(to_ticks(ten_minutes_ns), 6_000_000_000);
        assert_eq!(
            from_ticks(&serde_json::json!(6_000_000_000u64)),
            Some(ten_minutes_ns)
        );
    }

    #[test]
    fn the_header_carries_the_token_only_when_there_is_one() {
        let signed_in = authorization("device-1", Some("secret-token"));
        assert!(signed_in.contains("Client=\"TinePlayer\""));
        assert!(signed_in.contains("DeviceId=\"device-1\""));
        assert!(signed_in.contains("Token=\"secret-token\""));

        // Quick Connect happens before there is a token to send.
        let pairing = authorization("device-1", None);
        assert!(pairing.contains("DeviceId=\"device-1\""));
        assert!(!pairing.contains("Token="));
    }

    fn item(container: &str) -> Item {
        Item {
            id: "abc123".to_string(),
            media_source_id: "abc123".to_string(),
            container: container.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn the_stream_url_insists_on_the_original_file() {
        let mut pairing = Pairing::new("http://hoth:8096");
        pairing.account = Some(account());
        let client = Client::new(&pairing).unwrap();
        let url = client.stream_url(&item("mkv"));
        // Without this the server transcodes and delivers one audio track,
        // which is the whole feature gone.
        assert!(url.contains("static=true"), "{url}");
        assert!(
            url.starts_with("http://hoth:8096/Videos/abc123/stream.mkv"),
            "{url}"
        );
    }

    #[test]
    fn the_container_goes_on_the_url() {
        let mut pairing = Pairing::new("http://hoth:8096");
        pairing.account = Some(account());
        let client = Client::new(&pairing).unwrap();
        // Without the extension the server calls a QuickTime file AVI and
        // GStreamer picks the wrong demuxer, which hangs rather than fails.
        assert!(
            client.stream_url(&item("mov")).contains("/stream.mov?"),
            "the container must reach the URL"
        );
        // A source that never said keeps the bare form rather than inventing
        // an extension that would be wrong.
        assert!(client.stream_url(&item("")).contains("/stream?"));
    }

    #[test]
    fn a_pairing_without_an_account_has_no_client() {
        assert!(Client::new(&Pairing::new("http://hoth:8096")).is_none());
    }

    /// The whole pairing, against a real server, driven by this code rather
    /// than by curl.
    ///
    /// Ignored by default because it needs a server and a person: somebody has
    /// to type the code into a Jellyfin app while it polls. Run it with
    ///
    /// ```text
    /// TINEPLAYER_JELLYFIN=http://raspberrypi:8096 \
    ///     cargo test --release jellyfin_live -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs a Jellyfin server and somebody to approve the code"]
    fn jellyfin_live() {
        let server = std::env::var("TINEPLAYER_JELLYFIN")
            .expect("set TINEPLAYER_JELLYFIN to the server address");
        let mut pairing = Pairing::new(&server);

        let pending = quick_connect_start(&pairing.server, &pairing.device_id)
            .expect("the server would not start a pairing");
        println!("\n    CODE: {}\n", pending.code);
        println!("    Approve it in Jellyfin: user menu -> Quick Connect");

        let account = (0..600)
            .find_map(|_| {
                std::thread::sleep(std::time::Duration::from_secs(1));
                quick_connect_poll(&pairing.server, &pairing.device_id, &pending)
                    .expect("polling failed")
            })
            .expect("nobody approved the code in ten minutes");
        println!("    connected as {}", account.user_name);
        pairing.account = Some(account);

        let client = Client::new(&pairing).expect("a connected pairing makes a client");
        client.announce().expect("capabilities were refused");
        println!("    capabilities accepted");

        // Written where the application reads it, so that running this once
        // leaves TinePlayer itself paired - which is how the wiring gets
        // tested before there is a settings screen to do it properly.
        save(&pairing).expect("the pairing could not be saved");
        println!("    saved to {}", path().display());

        // The socket, which is what actually puts TinePlayer on a phone.
        let main = glib::MainLoop::new(None, false);
        let quit = main.clone();
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let heard = seen.clone();
        let _session = connect(&pairing, move |command| {
            println!("    <- {command:?}");
            heard.borrow_mut().push(command);
            quit.quit();
        })
        .expect("the socket would not open");

        println!("\n    Now cast something to this device from Jellyfin.\n");
        // Ends on the first command, or after long enough to have found a
        // phone and given up.
        let give_up = main.clone();
        glib::timeout_add_seconds_local_once(240, move || give_up.quit());
        main.run();

        let seen = seen.borrow();
        let command = seen.first().expect("nothing was cast within four minutes");
        match command {
            Command::Play { item_id, .. } => {
                let item = client.item(item_id).expect("the item could not be read");
                println!(
                    "    resolved: {} ({:?} runtime)",
                    item.title, item.runtime_ns
                );
                println!("    stream:   {}", client.stream_url(&item));
                assert!(!item.title.is_empty(), "an item should have a title");
            }
            other => panic!("expected a Play, got {other:?}"),
        }
    }
}
