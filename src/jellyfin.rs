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

/// The mark this device offers to the viewer's Jellyfin device list.
///
/// **Sent, accepted, stored - and not drawn.** Tried against 10.11.11 on
/// 2026-08-15: the dashboard shows Kodi and the browsers with their own icons
/// and TinePlayer without one. So the icons in that list do not come from
/// this field, whatever the API suggests, and where they do come from was not
/// chased further.
///
/// Kept rather than removed, because it costs one line in a request already
/// being made and it is what the server asks for: `ClientCapabilitiesDto`
/// carries an `IconUrl`, `DeviceManager` stores it against the device, and
/// `DeviceInfoDto` hands it back. If a version ever renders it, this is
/// already right.
///
/// An address rather than a picture, because that is the shape of the field:
/// whatever draws it fetches it, so it has to be somewhere public rather than
/// inside this binary. TinePlayer's own mark on TinePlayer's own site -
/// **never Jellyfin's**, whose logo needs their express permission. A site
/// that is down costs an icon and nothing else.
const ICON_URL: &str = "https://tineplayer.app/images/icon.png";

/// Long enough for a server waking a spun-down disk, short enough that one
/// which has gone away does not hold a worker thread for ever.
const TIMEOUT: u64 = 15;

/// Where a Jellyfin server listens for the question below, and the question.
///
/// Both are Jellyfin's, not ours: this is the same broadcast its own
/// applications make to fill their "select server" screen, and the string is
/// matched literally at the far end.
const DISCOVERY_PORT: u16 = 7359;
const DISCOVERY_ASK: &[u8] = b"Who is JellyfinServer?";

/// A server that answered the broadcast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// What the server calls itself: "hoth". What a viewer picks from a list.
    pub name: String,
    /// A complete address, as the server states it - `http://192.168.3.2:8096`.
    /// Taken as given rather than assembled here, since the server knows which
    /// port and scheme it is actually reachable on.
    pub address: String,
    /// Jellyfin's own id for the server, used only to tell two replies from
    /// one server apart from two servers.
    pub id: String,
}

/// Every Jellyfin server that answers on the networks this machine is on.
///
/// **Sent to each interface's own broadcast address, not to 255.255.255.255.**
/// Measured 2026-08-14 on a machine with VirtualBox, WSL and Hyper-V adapters
/// beside the real one: the all-ones broadcast is routed out whichever
/// interface wins on metric, which was a virtual adapter with nothing on it,
/// and the server answered the moment the subnet's own broadcast address was
/// used. The all-ones is sent as well, because it costs one datagram and
/// covers a machine whose interfaces cannot be listed at all.
///
/// Failure is an empty list rather than an error. Every way this can go wrong -
/// no interfaces, a socket the firewall will not open, a reply that is not
/// JSON - has the same answer for the viewer, which is to type the address
/// instead, and that is offered on the same panel whatever this returns.
pub fn discover(wait: std::time::Duration) -> Vec<Found> {
    let socket = match std::net::UdpSocket::bind("0.0.0.0:0") {
        Ok(socket) => socket,
        Err(e) => {
            eprintln!("Couldn't open a socket to look for Jellyfin servers: {e}");
            return Vec::new();
        }
    };
    if let Err(e) = socket.set_broadcast(true) {
        eprintln!("Couldn't broadcast to look for Jellyfin servers: {e}");
        return Vec::new();
    }
    // Short, so the loop below notices the deadline rather than sitting in a
    // read until something happens to arrive.
    let _ = socket.set_read_timeout(Some(std::time::Duration::from_millis(100)));

    for address in broadcast_addresses() {
        // One that fails is ordinary: a virtual adapter with nothing behind it
        // refuses, and the interface that matters is usually another one.
        let _ = socket.send_to(DISCOVERY_ASK, (address, DISCOVERY_PORT));
    }

    let deadline = std::time::Instant::now() + wait;
    let mut found: Vec<Found> = Vec::new();
    let mut buffer = [0u8; 1024];
    while std::time::Instant::now() < deadline {
        let Ok((size, _)) = socket.recv_from(&mut buffer) else {
            continue;
        };
        let Some(server) = read_discovery(&buffer[..size]) else {
            continue;
        };
        // One server answering on two interfaces is one server. Its id is what
        // says so; the address in each reply may legitimately differ.
        if !found.iter().any(|seen| seen.id == server.id) {
            found.push(server);
        }
    }
    // By name, so a list of them is in the same order every time rather than
    // in whatever order the replies happened to arrive.
    found.sort_by_key(|server| server.name.to_lowercase());
    found
}

/// The broadcast address of every network this machine is on, and the
/// all-ones as a fallback.
fn broadcast_addresses() -> Vec<std::net::Ipv4Addr> {
    let mut addresses = vec![std::net::Ipv4Addr::BROADCAST];
    let interfaces = match if_addrs::get_if_addrs() {
        Ok(interfaces) => interfaces,
        Err(e) => {
            eprintln!("Couldn't list this machine's networks: {e}");
            return addresses;
        }
    };
    for interface in interfaces {
        // Nothing is listening on this machine's own loopback, and a link
        // local address means an interface that never got a network.
        if interface.is_loopback() || interface.is_link_local() {
            continue;
        }
        let if_addrs::IfAddr::V4(v4) = interface.addr else {
            // Jellyfin's discovery is IPv4 broadcast, which IPv6 has no
            // equivalent of - it uses multicast instead, and Jellyfin does not
            // listen for one.
            continue;
        };
        // Worked out from the netmask when the system did not state one, which
        // is the whole point of asking for the interfaces at all.
        let broadcast = v4.broadcast.unwrap_or_else(|| {
            let ip = v4.ip.octets();
            let mask = v4.netmask.octets();
            std::net::Ipv4Addr::new(
                ip[0] | !mask[0],
                ip[1] | !mask[1],
                ip[2] | !mask[2],
                ip[3] | !mask[3],
            )
        });
        if !addresses.contains(&broadcast) {
            addresses.push(broadcast);
        }
    }
    addresses
}

/// One reply, or nothing.
///
/// A server with no name or no address is no use to a list somebody picks
/// from, so it is dropped rather than shown as a blank row.
fn read_discovery(reply: &[u8]) -> Option<Found> {
    let body: serde_json::Value = serde_json::from_slice(reply).ok()?;
    let text = |name: &str| {
        body.get(name)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let server = Found {
        name: text("Name"),
        address: normalize(&text("Address")),
        id: text("Id"),
    };
    match server.name.is_empty() || server.address.is_empty() {
        true => None,
        false => Some(server),
    }
}

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
    /// What the server calls itself - "hoth" - for saying which server this is
    /// without reciting an address at somebody.
    ///
    /// Absent when it could not be asked, which is not worth failing a pairing
    /// over: everywhere this is shown falls back to the address, which is
    /// always known. Absent too in every file written before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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
            name: None,
            account: None,
        }
    }

    /// What to call this server on screen: its own name where it gave one, and
    /// otherwise the address, which is always known.
    pub fn label(&self) -> String {
        self.name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| self.server.clone())
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

    /// Points this installation at a different server.
    ///
    /// The account goes with it. A token is issued by one server and means
    /// nothing to another, so keeping it would leave a pairing that reads as
    /// connected and is refused by everything it is used for. The device id
    /// stays, since it is this installation's name rather than the server's.
    pub fn set_server(&mut self, server: &str) {
        let server = normalize(server);
        if server != self.server {
            // The name belongs to the old server as much as the token does.
            self.account = None;
            self.name = None;
        }
        self.server = server;
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

/// Whether a status means "this pairing is gone" rather than "try again".
///
/// The distinction is the whole of how a dead pairing is told from a server
/// having a bad moment: one is retried with a backoff, the other stops and
/// asks to be paired again. Jellyfin says it both ways - 401 from the REST
/// endpoints, 403 from the WebSocket handshake - so the answer lives here
/// rather than being written out at each place that has to decide.
fn is_refusal(code: u16) -> bool {
    matches!(code, 401 | 403)
}

fn failed(code: i32, body: &str) -> Error {
    match u16::try_from(code).map(is_refusal) {
        Ok(true) => Error::Unauthorized,
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

    /// Ends the pairing at the server, which is half of disconnecting.
    ///
    /// **One call, and it does the whole job.** `POST /Sessions/Logout` reaches
    /// `SessionManager.Logout`, which looks the device up by its access token
    /// and calls `DeleteDevice` on it - so the token is revoked *and* the entry
    /// disappears from the viewer's Devices page. Read out of Jellyfin's own
    /// source on 2026-08-15.
    ///
    /// **It deliberately does not also `DELETE /Devices?id=`, which is what
    /// this used to do.** That endpoint is redundant after the logout above,
    /// and `DevicesController` carries
    /// `[Authorize(Policy = Policies.RequiresElevation)]` on the class, so an
    /// ordinary account is refused it. Pairing the two meant a viewer who is
    /// not an administrator got a 403 from a call that never needed making,
    /// and was told the server could not be told - when it had been, and had
    /// already removed the device. `Sessions/Logout` is plain `[Authorize]`:
    /// it is the viewer's own session, so anybody may end their own.
    ///
    /// Whatever this answers, the caller deletes the local file: a viewer who
    /// asked to disconnect has disconnected, whether or not a server that may
    /// be switched off agreed to hear about it.
    pub fn disconnect(&self) -> Result<(), Error> {
        self.post("/Sessions/Logout", serde_json::json!({}))
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
                //
                // Volume and the two selectors went in once there was something
                // behind each of them: one level over both outputs for the
                // slider, and the choosers on the control strip for the tracks.
                // Each drives the same thing the person in the room drives
                // rather than a second path to the same setting.
                "SupportedCommands": [
                    "Play",
                    "PlayState",
                    "SetVolume",
                    "Mute",
                    "Unmute",
                    "ToggleMute",
                    "SetAudioStreamIndex",
                    "SetSubtitleStreamIndex",
                ],
                "SupportsMediaControl": true,
                "SupportsPersistentIdentifier": true,
                "IconUrl": ICON_URL,
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
        // The backdrop, and which item it actually belongs to.
        //
        // **An episode has none of its own.** Verified across a library on
        // 2026-08-16: every episode in it answers with an empty
        // `BackdropImageTags`, because the artwork belongs to the *series* -
        // which the episode carries as `ParentBackdropImageTags`, alongside
        // the series id in `ParentBackdropItemId`. Asking for
        // `/Items/{episode}/Images/Backdrop/0` gets nothing, which is why a TV
        // show cast from Jellyfin came up with a bare page while a film did
        // not. The poster is unaffected: an episode has its own.
        //
        // The owner travels with the tag rather than being worked out later,
        // because a tag is only good against the item it came from.
        let first_tag = |field: &str| {
            body.get(field)
                .and_then(|tags| tags.as_array())
                .and_then(|tags| tags.first())
                .and_then(|tag| tag.as_str())
                .map(str::to_string)
        };
        let backdrop = first_tag("BackdropImageTags")
            .map(|tag| (tag, id.to_string()))
            .or_else(|| {
                let owner = body
                    .get("ParentBackdropItemId")
                    .and_then(|owner| owner.as_str())?;
                Some((first_tag("ParentBackdropImageTags")?, owner.to_string()))
            });

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
            // The series an episode belongs to, which is a thing in its own
            // right: it has the name and the poster that the episode has not.
            // Empty for a film, which belongs to nothing.
            series_name: text("SeriesName"),
            series_id: text("SeriesId"),
            season_id: text("SeasonId"),
            season_name: text("SeasonName"),
            series_poster_tag: body
                .get("SeriesPrimaryImageTag")
                .and_then(|tag| tag.as_str())
                .map(str::to_string),
            backdrop_tag: backdrop.as_ref().map(|(tag, _)| tag.clone()),
            backdrop_item: backdrop
                .map(|(_, owner)| owner)
                .unwrap_or_else(|| id.to_string()),
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
            "{}/Items/{id}/Images/{kind}?{}maxWidth={width}&api_key={}",
            self.server,
            // Quoted only when there is one. A tag makes the answer cacheable
            // and names a particular version; without one the server hands
            // back whatever is current, which is what a caller that never saw
            // a tag wants. Verified against 10.11.11 - the same bytes either
            // way.
            match tag.is_empty() {
                true => String::new(),
                false => format!("tag={tag}&"),
            },
            self.token
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
    /// Where to fetch one of the server's own subtitle files.
    ///
    /// `index` is Jellyfin's numbering across every stream in the item, not a
    /// position among the subtitles - an external subtitle commonly sits at 0,
    /// before the video. It is quoted back exactly as it was given.
    ///
    /// Asked for as `.srt` rather than the format it is stored in: Jellyfin
    /// converts on the way out, so one request shape covers every text format
    /// the library holds, and `subparse` needs no help identifying it.
    pub fn subtitle_url(&self, item: &Item, index: u32) -> String {
        format!(
            "{}/Videos/{}/{}/Subtitles/{index}/Stream.srt?api_key={}",
            self.server, item.id, item.media_source_id, self.token
        )
    }

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
    pub fn started(
        &self,
        id: &str,
        play_session: &str,
        position_ns: u64,
        sound: Sound,
    ) -> Result<(), Error> {
        let mut body = serde_json::json!({
                "ItemId": id,
                // Direct play, so the source is the item itself.
                "MediaSourceId": id,
                "PlaySessionId": play_session,
                "PlayMethod": "DirectPlay",
                "PositionTicks": to_ticks(position_ns),
                "CanSeek": true,
                "IsPaused": false,
                "IsMuted": sound.muted,
                "VolumeLevel": sound.percent(),
        });
        sound.add_streams(&mut body);
        self.post("/Sessions/Playing", body)
    }

    pub fn progress(
        &self,
        id: &str,
        play_session: &str,
        position_ns: u64,
        paused: bool,
        sound: Sound,
    ) -> Result<(), Error> {
        let mut body = serde_json::json!({
                "ItemId": id,
                "MediaSourceId": id,
                "PlaySessionId": play_session,
                "PlayMethod": "DirectPlay",
                "PositionTicks": to_ticks(position_ns),
                "CanSeek": true,
                "IsPaused": paused,
                "IsMuted": sound.muted,
                "VolumeLevel": sound.percent(),
        });
        sound.add_streams(&mut body);
        self.post("/Sessions/Playing/Progress", body)
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
    /// What the series is called, for an episode: "Breaking Bad". Empty for
    /// anything that is not one.
    pub series_name: String,
    /// The series itself, which is where its poster is fetched from.
    pub series_id: String,
    /// The season this episode is in, which is an item in its own right and
    /// carries its own poster - the one that says which run of a programme
    /// this is. Empty for anything that is not an episode.
    pub season_id: String,
    /// What the library calls that season: usually "Season 1", and sometimes
    /// something a number cannot say - "Specials" is season zero.
    pub season_name: String,
    /// The series' own poster, which an episode does not have and is not the
    /// same picture as its own Primary image - that is a still from the
    /// episode.
    pub series_poster_tag: Option<String>,
    /// Which item the backdrop belongs to, which is not always this one: an
    /// episode has no artwork of its own and wears the series'. Its own id
    /// where there is nothing to inherit, so a caller never has to ask which
    /// case it is in.
    pub backdrop_item: String,
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

/// What a controller shows about the player: how loud it is and what it is
/// playing.
///
/// Reported as well as obeyed. A remote that can set something but is never
/// told where it ended up shows whatever it last sent - which is why the
/// subtitle selector on a phone read "None" however many times a subtitle had
/// been chosen, from the phone or from the sofa. Reported by Scott, 2026-08-14.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Sound {
    /// The main level, as a fraction of full.
    pub level: f64,
    /// Whether everything is silenced at once. Each output's own mute is
    /// deliberately not folded in: it belongs to one of two people listening,
    /// where this is the state of the room.
    pub muted: bool,
    /// Jellyfin's own number for the soundtrack the first output is playing,
    /// and `None` when it is playing something the library has no number for -
    /// a separate audio file, say.
    ///
    /// The first output, because that is the one the remote drives. Reporting
    /// the second would tell a controller that its own last command had been
    /// ignored.
    pub audio: Option<u32>,
    /// The same for subtitles, where `-1` is Jellyfin's way of saying none and
    /// is a real answer rather than a missing one.
    pub subtitle: Option<i32>,
}

impl Sound {
    /// Jellyfin counts volume in whole percent.
    fn percent(&self) -> u32 {
        (self.level.clamp(0.0, 1.0) * 100.0).round() as u32
    }

    /// Adds what is playing to a report, leaving out anything the library has
    /// no number for rather than guessing at one.
    fn add_streams(&self, body: &mut serde_json::Value) {
        let Some(body) = body.as_object_mut() else {
            return;
        };
        if let Some(index) = self.audio {
            body.insert("AudioStreamIndex".into(), index.into());
        }
        if let Some(index) = self.subtitle {
            body.insert("SubtitleStreamIndex".into(), index.into());
        }
    }
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
                // Nothing to read. The library hands over a description of the
                // stream rather than the file, and it carries no equivalent of
                // Matroska's `FlagVisualImpaired` - so a cast item falls
                // through to its track title, which is where this always was.
                // Worth remembering that the server's flags are not reliably
                // better anyway: it reports `IsForced=False` on a track it
                // titles "Forced".
                described: None,
                commentary: None,
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
                // The server states a codec for these, which is what a row
                // shows where a local file shows its container's.
                format: codec_name(&stream.codec),
                // Left unstated rather than taken from the server. Jellyfin
                // reports `IsForced=False` on tracks it titles "Forced", so
                // its answer is not better than reading the title - and
                // `None` is what sends this to the title, where every other
                // source without flags already goes.
                forced: None,
                hearing_impaired: None,
                commentary: None,
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

    /// Where Jellyfin's own stream number sits among the embedded audio
    /// tracks, which is the number the pipeline selects by.
    ///
    /// `None` for a stream that is external, or is not audio at all. Both are
    /// answers rather than failures: an external track is not in the container
    /// to be selected, and a controller is free to send anything.
    pub fn audio_position(&self, index: u32) -> Option<u32> {
        position_of(&self.audio, index)
    }

    /// The same for subtitles.
    pub fn subtitle_position(&self, index: u32) -> Option<u32> {
        position_of(&self.subtitles, index)
    }

    /// Jellyfin's own number for the embedded audio stream at `position`,
    /// which is the reverse of [`Self::audio_position`] and is what a report
    /// has to quote.
    pub fn audio_index(&self, position: u32) -> Option<u32> {
        index_at(&self.audio, position)
    }

    /// The same for subtitles.
    pub fn subtitle_index(&self, position: u32) -> Option<u32> {
        index_at(&self.subtitles, position)
    }

    // There is deliberately no `external_audio` counterpart to the subtitles
    // below, and it is worth saying why rather than leaving the asymmetry to
    // look like an oversight.
    //
    // **Jellyfin will not serve an external audio track to a client.** Proved
    // against 10.11.11 on 2026-08-15, from both ends. Every `/Audio/` endpoint
    // - `main.m3u8`, `stream.mp3`, `universal` - accepts `audioStreamIndex`,
    // answers 200, plays perfectly, and returns the item's *default embedded*
    // audio whatever is asked for: indices 2, 4, 0 (a subtitle) and 99 (which
    // does not exist) all gave byte-identical output. The server's own source
    // says why - `GetInputArgument` adds the external file as a second ffmpeg
    // input on every path, and the audio-only command builders emit no `-map`
    // at all (`mapArgs` is `state.IsOutputVideo ? ... : string.Empty`), so
    // ffmpeg's default stream selection takes the embedded track instead.
    //
    // The one route that does work is `/Videos/{id}/main.m3u8`, which maps the
    // external input explicitly - at the price of transcoding the entire film
    // to extract one soundtrack, while the video is already direct-playing.
    // **Ruled out by Scott on 2026-08-15**, and not a direction to revisit.
    //
    // So an external soundtrack in a library is unreachable, and offering one
    // in the chooser would be offering a described track that silently plays a
    // different one. When the upstream `-map` lands this becomes three lines
    // and a URL; until then there is nothing honest to build.

    pub fn external_subtitles(&self) -> impl Iterator<Item = &Stream> {
        self.subtitles.iter().filter(|stream| stream.external)
    }

    /// The server's own subtitle files, as entries for the chooser.
    ///
    /// Only the external ones. The embedded ones are already offered by
    /// position through `as_media`, and listing them twice would give two
    /// entries that play the same subtitle by two different routes.
    ///
    /// Labelled the same way an embedded track is, so a list holding both
    /// reads as one list rather than as two that were joined.
    pub fn subtitle_options(&self) -> Vec<crate::subtitles::Subtitle> {
        self.external_subtitles()
            .map(|stream| crate::subtitles::Subtitle::Library {
                index: stream.index,
                label: match (stream.language.is_empty(), stream.title.is_empty()) {
                    (false, false) => format!("{} - {}", stream.language, stream.title),
                    (false, true) => stream.language.clone(),
                    (true, false) => stream.title.clone(),
                    (true, true) => crate::trc!("subtitle track", "Subtitles").into_owned(),
                },
                // Carried apart as well as together: the label is what a typed
                // choice is matched against, and the row is built from the two
                // halves - see `crate::subtitles::row`.
                language: stream.language.clone(),
                title: stream.title.clone(),
            })
            .collect()
    }
}

/// Where one of Jellyfin's stream numbers falls among the embedded streams of
/// its own kind.
///
/// External streams are skipped rather than counted, which is the whole trap
/// this exists to avoid: Jellyfin lists an external subtitle at index 0, before
/// the video, so counting them would put every embedded track after it out of
/// step - silently, and in the direction that hands somebody the wrong
/// language.
fn position_of(streams: &[Stream], index: u32) -> Option<u32> {
    streams
        .iter()
        .filter(|stream| !stream.external)
        .position(|stream| stream.index == index)
        .map(|position| position as u32)
}

/// Jellyfin's number for the embedded stream at `position` among its own kind.
///
/// The same skipping of external streams as [`position_of`], in the other
/// direction, and for the same reason.
fn index_at(streams: &[Stream], position: u32) -> Option<u32> {
    streams
        .iter()
        .filter(|stream| !stream.external)
        .nth(position as usize)
        .map(|stream| stream.index)
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

/// What the server calls itself, asked without a token.
///
/// `/System/Info/Public` needs no authentication, which is what makes this
/// safe to do before a pairing exists and safe to fail: a server that will not
/// say is shown by its address instead. Deliberately not `/System/Info`, which
/// is the same answer behind a token and would be one more thing to go wrong.
pub fn server_name(server: &str) -> Option<String> {
    let response = minreq::get(format!("{}/System/Info/Public", normalize(server)))
        .with_timeout(TIMEOUT)
        .send()
        .ok()?;
    if response.status_code != 200 {
        return None;
    }
    let body: serde_json::Value = response.json().ok()?;
    body.get("ServerName")
        .and_then(|name| name.as_str())
        .map(str::to_string)
        .filter(|name| !name.trim().is_empty())
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
/// accident: the queue commands mean nothing to a player that shows one video
/// at a time, and `DisplayMessage` has nowhere to put a message that would not
/// sit over the film.
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
    /// Where the main level should stand, as a fraction of full. Jellyfin
    /// counts in whole percent and TinePlayer in fractions, so the conversion
    /// happens on the way in and nowhere else.
    SetVolume(f64),
    Mute,
    Unmute,
    ToggleMute,
    /// **Jellyfin's own stream number**, across every stream in the item, and
    /// not a position among the audio tracks. Turned into one by
    /// [`Streams::audio_position`], which is the only place that knows the
    /// difference.
    ///
    /// It drives the first output. Jellyfin offers one selector and TinePlayer
    /// has two outputs, and the second is the one somebody chose deliberately
    /// for the other person in the room - a remote quietly changing it is worse
    /// than a remote that leaves it alone. Decided 2026-08-14.
    SetAudioStream(u32),
    /// The same numbering, and `None` for turning subtitles off - which is what
    /// Jellyfin means by an index below zero.
    SetSubtitleStream(Option<u32>),
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
        // **A revoked token is a 403 here and a 401 on every REST call**,
        // measured 2026-08-15 by deleting this device from the Jellyfin
        // dashboard and watching both: `/Users/Me` answered 401 and the socket
        // handshake answered 403 Forbidden. Reading only 401 is what made a
        // dead pairing retry for ever with a widening backoff - the one thing
        // the note below says must never happen - while the log said plainly
        // that the connection was no longer accepted.
        //
        // Both are read as "the pairing is gone", the same rule `failed`
        // already applies to the REST side. Two places deciding what a revoked
        // token looks like is how they came to disagree.
        Err(tungstenite::Error::Http(response)) if is_refusal(response.status().as_u16()) => {
            return Err(Error::Unauthorized);
        }
        Err(e) => return Err(Error::Failed(e.to_string())),
    };
    if is_refusal(response.status().as_u16()) {
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
        // Arguments arrive as strings, whatever they hold. `"Volume": "42"`
        // rather than `42`, so every one of these is parsed rather than read as
        // a number - and a controller sending a real number is read too, since
        // nothing is lost by trying both.
        "GeneralCommand" => {
            let data = data?;
            let argument = |name: &str| -> Option<i64> {
                let value = data.get("Arguments")?.get(name)?;
                match value.as_i64() {
                    Some(number) => Some(number),
                    None => value.as_str()?.trim().parse().ok(),
                }
            };
            match data.get("Name")?.as_str()? {
                "SetVolume" => Some(Command::SetVolume(
                    (argument("Volume")? as f64 / 100.0).clamp(0.0, 1.0),
                )),
                "Mute" => Some(Command::Mute),
                "Unmute" => Some(Command::Unmute),
                "ToggleMute" => Some(Command::ToggleMute),
                "SetAudioStreamIndex" => {
                    Some(Command::SetAudioStream(argument("Index")?.try_into().ok()?))
                }
                // Below zero is Jellyfin's way of saying none, which is a real
                // choice rather than a missing answer.
                "SetSubtitleStreamIndex" => Some(Command::SetSubtitleStream(
                    argument("Index").map(|index| u32::try_from(index).ok())?,
                )),
                _ => None,
            }
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

    fn stream(index: u32, external: bool) -> Stream {
        Stream {
            index,
            external,
            ..Stream::default()
        }
    }

    #[test]
    fn counts_embedded_streams_past_an_external_one() {
        // The shape 46 of 120 items in the library have: a subtitle file beside
        // the video, which Jellyfin lists at index 0, before the video stream.
        let streams = Streams {
            subtitles: vec![stream(0, true), stream(3, false), stream(4, false)],
            ..Streams::default()
        };
        assert_eq!(streams.subtitle_position(3), Some(0));
        assert_eq!(streams.subtitle_position(4), Some(1));
        // Not in the container, so there is no position in it to have.
        assert_eq!(streams.subtitle_position(0), None);
        assert_eq!(streams.subtitle_position(9), None);
    }

    /// A revoked pairing must be recognised however the server phrases it.
    ///
    /// Jellyfin says it two ways - 401 from the REST endpoints, 403 from the
    /// WebSocket handshake, both measured on 2026-08-15 against a device
    /// deleted from the dashboard. Reading only 401 left the socket retrying a
    /// dead pairing for ever, which is the exact failure the design set out to
    /// avoid: TinePlayer absent from every phone with nothing said about why.
    #[test]
    fn a_revoked_pairing_is_recognised_either_way() {
        assert!(is_refusal(401), "the REST endpoints answer 401");
        assert!(is_refusal(403), "the socket handshake answers 403");
        assert_eq!(failed(401, ""), Error::Unauthorized);
        assert_eq!(failed(403, ""), Error::Unauthorized);

        // A server having a bad moment is not a revoked pairing, and must stay
        // retryable - stopping on one of these would take TinePlayer off
        // everyone's phone until it was next started.
        for code in [500, 502, 503, 404, 400] {
            assert!(!is_refusal(code), "{code} is worth retrying");
            assert!(matches!(failed(code.into(), "trouble"), Error::Failed(_)));
        }
    }

    #[test]
    fn reads_a_volume_command() {
        let text = r#"{"MessageType":"GeneralCommand","Data":{"Name":"SetVolume",
            "Arguments":{"Volume":"42"}}}"#;
        assert_eq!(interpret(text), Some(Command::SetVolume(0.42)));
    }

    #[test]
    fn reads_a_track_selection() {
        let text = r#"{"MessageType":"GeneralCommand","Data":
            {"Name":"SetAudioStreamIndex","Arguments":{"Index":"2"}}}"#;
        assert_eq!(interpret(text), Some(Command::SetAudioStream(2)));
    }

    #[test]
    fn reads_subtitles_being_turned_off() {
        // Below zero is how Jellyfin says none, and it has to survive as a
        // choice rather than being dropped as an unreadable argument.
        let text = r#"{"MessageType":"GeneralCommand","Data":
            {"Name":"SetSubtitleStreamIndex","Arguments":{"Index":"-1"}}}"#;
        assert_eq!(interpret(text), Some(Command::SetSubtitleStream(None)));
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

    /// The reply this was written against, recorded from a live server on
    /// 2026-08-14 so the shape is not something anybody has to guess at again.
    #[test]
    fn reads_a_servers_answer() {
        let reply = br#"{"Address":"http://192.168.3.2:8096",
            "Id":"191fdc0b078747329587e739ce34cbcc","Name":"hoth",
            "EndpointAddress":null}"#;
        assert_eq!(
            read_discovery(reply),
            Some(Found {
                name: "hoth".to_string(),
                address: "http://192.168.3.2:8096".to_string(),
                id: "191fdc0b078747329587e739ce34cbcc".to_string(),
            })
        );
    }

    #[test]
    fn an_answer_that_says_nothing_useful_is_not_a_server() {
        // Anything at all can arrive on a broadcast port.
        assert_eq!(read_discovery(b"hello?"), None);
        assert_eq!(read_discovery(br#"{"Name":"hoth"}"#), None);
        assert_eq!(read_discovery(br#"{"Address":"http://hoth:8096"}"#), None);
    }

    /// The trailing slash goes here as well as on a typed address: this one
    /// ends up in exactly the same field, and a server that states itself with
    /// one would otherwise pair to a different string than the same server
    /// typed by hand.
    #[test]
    fn a_discovered_address_is_normalized_too() {
        let reply = br#"{"Address":"http://hoth:8096/","Id":"x","Name":"hoth"}"#;
        assert_eq!(read_discovery(reply).unwrap().address, "http://hoth:8096");
    }

    /// What a server is called on screen, and what stands in when it never
    /// said. The address is always known, so there is always an answer.
    #[test]
    fn a_server_is_named_where_it_gave_one() {
        let mut pairing = Pairing::new("http://192.168.3.2:8096");
        assert_eq!(pairing.label(), "http://192.168.3.2:8096");

        pairing.name = Some("hoth".to_string());
        assert_eq!(pairing.label(), "hoth");

        // A server that answered with nothing useful is the same as one that
        // did not answer: an empty name would read as no server at all.
        pairing.name = Some("   ".to_string());
        assert_eq!(pairing.label(), "http://192.168.3.2:8096");
    }

    /// The name belongs to the server that gave it, so pointing this
    /// installation somewhere else must not keep calling it by the old name.
    #[test]
    fn a_new_server_is_not_called_by_the_old_ones_name() {
        let mut pairing = Pairing::new("http://hoth:8096");
        pairing.name = Some("hoth".to_string());

        pairing.set_server("http://endor:8096");
        assert_eq!(pairing.name, None);
        assert_eq!(pairing.label(), "http://endor:8096");
    }

    #[test]
    fn naming_a_different_server_drops_the_account() {
        let mut pairing = Pairing::new("http://hoth:8096");
        pairing.account = Some(account());
        let device_id = pairing.device_id.clone();

        // Retyping the same address, which is what saving an unchanged field
        // does, must not sign anybody out.
        pairing.set_server("http://hoth:8096/");
        assert!(pairing.is_connected());

        // A different one must, because the token was issued by the old one.
        pairing.set_server("http://endor:8096");
        assert!(!pairing.is_connected());
        assert_eq!(pairing.server, "http://endor:8096");
        assert_eq!(
            pairing.device_id, device_id,
            "device id is ours, not theirs"
        );
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

    /// Discovery against whatever is really on this network.
    ///
    /// Ignored by default because it needs a Jellyfin server on the same
    /// subnet as the machine running it, which CI has not got. Worth running
    /// by hand on each platform, because this is the one part of the client
    /// whose behaviour is decided by the operating system's routing rather
    /// than by anything here:
    ///
    /// ```text
    /// cargo test jellyfin_discovery_live -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs a Jellyfin server on this network"]
    fn jellyfin_discovery_live() {
        for address in broadcast_addresses() {
            println!("    asking {address}");
        }
        let found = discover(std::time::Duration::from_secs(2));
        for server in &found {
            println!(
                "    found {} at {} ({})",
                server.name, server.address, server.id
            );
        }
        assert!(!found.is_empty(), "no server answered on this network");

        // And that the same server says what it is called when asked directly,
        // which is the route a typed address has to take - there is no
        // broadcast reply to read a name out of then.
        let server = &found[0];
        let asked = server_name(&server.address);
        println!("    {} calls itself {asked:?}", server.address);
        assert_eq!(
            asked.as_deref(),
            Some(server.name.as_str()),
            "the broadcast and the API should agree on the name"
        );
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
