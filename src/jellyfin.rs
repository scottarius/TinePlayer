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
                "SupportedCommands": [
                    "Play", "PlayState", "SetVolume", "Mute", "Unmute", "ToggleMute",
                    "SetAudioStreamIndex", "SetSubtitleStreamIndex", "DisplayMessage",
                    "ToggleFullscreen"
                ],
                "SupportsMediaControl": true,
                "SupportsPersistentIdentifier": true,
            }),
        )
    }

    /// What is known about one item, in the units TinePlayer counts in.
    pub fn item(&self, id: &str) -> Result<Item, Error> {
        let body = self.get(&format!("/Users/{}/Items/{id}", self.user_id))?;
        Ok(Item {
            id: id.to_string(),
            title: body
                .get("Name")
                .and_then(|name| name.as_str())
                .unwrap_or_default()
                .to_string(),
            runtime_ns: body.get("RunTimeTicks").and_then(from_ticks),
            resume_ns: body
                .get("UserData")
                .and_then(|data| data.get("PlaybackPositionTicks"))
                .and_then(from_ticks)
                .filter(|position| *position > 0),
        })
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
    pub fn stream_url(&self, id: &str) -> String {
        format!(
            "{}/Videos/{id}/stream?static=true&api_key={}",
            self.server, self.token
        )
    }

    pub fn started(&self, id: &str, position_ns: u64) -> Result<(), Error> {
        self.post(
            "/Sessions/Playing",
            serde_json::json!({ "ItemId": id, "PositionTicks": to_ticks(position_ns) }),
        )
    }

    pub fn progress(&self, id: &str, position_ns: u64, paused: bool) -> Result<(), Error> {
        self.post(
            "/Sessions/Playing/Progress",
            serde_json::json!({
                "ItemId": id,
                "PositionTicks": to_ticks(position_ns),
                "IsPaused": paused,
            }),
        )
    }

    pub fn stopped(&self, id: &str, position_ns: u64) -> Result<(), Error> {
        self.post(
            "/Sessions/Playing/Stopped",
            serde_json::json!({ "ItemId": id, "PositionTicks": to_ticks(position_ns) }),
        )
    }
}

/// A video as Jellyfin describes it, converted into what TinePlayer counts in.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    /// The library title, which is the whole reason to ask: "Avengers:
    /// Endgame" rather than the file name.
    pub title: String,
    pub runtime_ns: Option<u64>,
    /// Where this viewer stopped, or `None` for one they have not started -
    /// which includes a video watched to the end.
    pub resume_ns: Option<u64>,
}

/// Jellyfin counts in ticks of a hundred nanoseconds and TinePlayer counts in
/// nanoseconds. Converted here and nowhere else, so the two cannot drift.
fn from_ticks(value: &serde_json::Value) -> Option<u64> {
    value.as_u64().map(|ticks| ticks * 100)
}

fn to_ticks(nanoseconds: u64) -> u64 {
    nanoseconds / 100
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

    #[test]
    fn the_stream_url_insists_on_the_original_file() {
        let mut pairing = Pairing::new("http://hoth:8096");
        pairing.account = Some(account());
        let client = Client::new(&pairing).unwrap();
        let url = client.stream_url("abc123");
        // Without this the server transcodes and delivers one audio track,
        // which is the whole feature gone.
        assert!(url.contains("static=true"), "{url}");
        assert!(
            url.starts_with("http://hoth:8096/Videos/abc123/stream"),
            "{url}"
        );
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
        println!("    stream URL: {}", client.stream_url("<item>"));
    }
}
