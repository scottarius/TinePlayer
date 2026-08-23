//! Registering TinePlayer with Kodi, from inside the application.
//!
//! Kodi has no interface for adding an external player: it reads
//! `playercorefactory.xml` from its userdata directory and offers whatever it
//! finds there. That file is otherwise hand-edited, which is a poor thing to
//! ask of somebody who just wants two soundtracks at once.
//!
//! The file is edited in place rather than replaced. Someone may have their
//! own players in there, with their own comments and their own formatting,
//! and none of that is ours to throw away - so our player is inserted or cut
//! out and everything else is left exactly as it was, and nothing is ever
//! deleted.
//!
//! A copy can be kept before the first change, which the wizard offers and
//! defaults to. Removing TinePlayer never restores one: the file may well
//! have been edited since, by hand or by Kodi, and putting an old copy back
//! would undo that. Removal cuts out our entry and leaves everything else.

use gtk::glib;
use std::borrow::Cow;
use std::path::{Path, PathBuf};

use crate::tr;

/// The template that ships with the source, used whole when there is no file
/// yet and mined for its `<player>` element when there is. Embedded rather
/// than read from disk so a packaged build needs nothing beside it.
const TEMPLATE: &str = include_str!("../data/templates/playercorefactory.xml");

/// The placeholders the template carries where the command belongs: the
/// program Kodi runs, and whatever has to come before the filename in its
/// arguments to get from there to TinePlayer.
const PLACEHOLDER: &str = "TINEPLAYER_BINARY";
const PLACEHOLDER_ARGS: &str = "TINEPLAYER_LAUNCH";
/// Where `--play` goes, or nothing when Kodi should hand over to the menu.
const PLACEHOLDER_PLAY: &str = "TINEPLAYER_PLAY";

/// The Flatpak application id, which is how a Flatpak build is started from
/// outside its own sandbox. Matches the id in `main.rs` and the manifest.
const FLATPAK_ID: &str = "app.tineplayer.TinePlayer";

/// What Kodi is currently set up to do with TinePlayer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Registration {
    /// Not mentioned in the file at all.
    Absent,
    /// Offered under "Play using...", with Kodi still playing videos itself.
    Offered,
    /// Handed every video.
    Default,
}

impl Registration {
    /// What this state is called where it is read rather than chosen: the
    /// value against the Player Type row.
    pub fn describe(self) -> Cow<'static, str> {
        match self {
            Registration::Absent => tr!("Not configured"),
            Registration::Offered => tr!("Additional Player"),
            Registration::Default => tr!("Default Player"),
        }
    }

    /// What this state is called as an entry in the Player Type chooser.
    ///
    /// Differs from [`describe`] in one place, and only when something is
    /// already set up: "Not configured" is a state to be in, but nobody
    /// *chooses* it - they remove what is there. So the entry says what
    /// pressing it does, and the two other entries, which are states either
    /// way, keep their names.
    ///
    /// [`describe`]: Registration::describe
    pub fn choice(self, configured: bool) -> Cow<'static, str> {
        match (self, configured) {
            (Registration::Absent, true) => tr!("Remove configuration"),
            _ => self.describe(),
        }
    }

    /// Every state, in the order the chooser offers them. Removal last: it is
    /// the one entry that takes something away, which is where the settings
    /// screen puts Clear Saved Playback Data for the same reason.
    pub const ALL: [Registration; 3] = [
        Registration::Default,
        Registration::Offered,
        Registration::Absent,
    ];
}

/// How the Kodi we found was installed.
///
/// This decides whether the command we write can be run as it stands. A Kodi
/// from a distribution's packages starts an external player on the machine
/// itself and anything runnable there will do. A Kodi that is a Flatpak or a
/// Snap starts it inside its own sandbox instead, which has no GTK 4 in it and
/// cannot see the user's home directory, so the command has to step out to the
/// host before it names TinePlayer at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Confinement {
    /// Installed by the system's package manager, and not sandboxed.
    None,
    /// A Flatpak, which can reach the host through `flatpak-spawn` once it is
    /// given permission to.
    Flatpak,
    /// A Snap. Same shape of problem, without an equivalent way out.
    Snap,
}

impl Confinement {
    /// Whether TinePlayer can be set up in a Kodi installed this way.
    ///
    /// A Snap cannot: it confines Kodi to its own view of the system with no
    /// supported way to start a program outside that, so the command would be
    /// written and then quietly do nothing. Better to say so before it is
    /// chosen than to let it be configured and appear to work.
    pub fn supported(self) -> bool {
        self != Confinement::Snap
    }

    /// Why not, for the one case where it is not.
    pub fn unsupported_reason(self) -> Option<Cow<'static, str>> {
        (!self.supported()).then(|| tr!("Snap installs do not support external players."))
    }

    /// What to add after the version to tell one Kodi from another.
    ///
    /// Every install gets a qualifier, including the ordinary one. That is a
    /// deliberate change: each installation now heads its own group of rows,
    /// and a run of headings where some are qualified and some are bare reads
    /// as though the bare ones are missing something. "Standard" is also the
    /// answer to the question the qualifier raises - having seen "(Flatpak)",
    /// the next thing anyone wants to know is what the other one is.
    ///
    /// Note this is not the whole qualifier: a folder named by hand is called
    /// "custom" whatever it holds, which [`Setup::label`] decides.
    pub fn describe(self) -> Cow<'static, str> {
        match self {
            Confinement::None => tr!("Default Installation"),
            // Product names, so they read the same in every language.
            Confinement::Flatpak => Cow::Borrowed("Flatpak"),
            Confinement::Snap => Cow::Borrowed("Snap"),
        }
    }
}

/// Where Kodi keeps its settings, how it was installed, and what it currently
/// says about us.
pub struct Setup {
    pub file: PathBuf,
    pub state: Registration,
    pub confinement: Confinement,
    /// Whether our entry here starts the film or opens the menu. Meaningless
    /// while `state` is `Absent`, and the row that shows it is disabled then.
    pub play: bool,
    /// What Kodi calls itself here, when that could be found out. `None` is a
    /// perfectly good answer: see [`version_of`].
    pub version: Option<String>,
}

/// Asks the thing that installed Kodi what version it installed.
///
/// Every route fails silently to `None`, and none of them is allowed to be
/// slow or to start Kodi. A label reading "Kodi (Flatpak)" is a small loss; a
/// label reading the wrong version is worse than no version at all.
///
/// Deliberately not read from the database file. `MyVideos121.db` does say
/// Kodi 20, but only through a table of magic numbers that gains a row with
/// every Kodi release and silently starts lying when it falls behind.
fn version_of(confinement: Confinement, userdata: &Path) -> Option<String> {
    let _ = userdata;

    // Each branch below is the whole of the function on the platform it is
    // compiled for, the others being cfg'd away - so each ends in a value
    // rather than returning one.
    #[cfg(target_os = "linux")]
    {
        match confinement {
            // "Kodi Media Center 20.5 (20.5.0) Git:20240501-8c8d7afa26"
            Confinement::None => {
                let out = ask("kodi", &["--version"])?;
                out.split_whitespace().nth(3).map(str::to_string)
            }
            Confinement::Flatpak => {
                let out = ask("flatpak", &["info", "tv.kodi.Kodi"])?;
                field(&out, "Version:")
            }
            Confinement::Snap => {
                // A header line, then the row for kodi: name, version, rev...
                let out = ask("snap", &["list", "kodi"])?;
                let row = out.lines().nth(1)?;
                row.split_whitespace().nth(1).map(str::to_string)
            }
        }
    }

    // A bundle carries its version in its metadata, so this costs a file read
    // rather than starting anything.
    #[cfg(target_os = "macos")]
    {
        let _ = confinement;
        let plist = std::fs::read_to_string("/Applications/Kodi.app/Contents/Info.plist").ok()?;
        let at = plist.find("<key>CFBundleShortVersionString</key>")?;
        let rest = &plist[at..];
        let open = rest.find("<string>")? + "<string>".len();
        let close = rest[open..].find("</string>")?;
        Some(rest[open..open + close].trim().to_string())
    }

    // Windows has no equally cheap answer - the version lives in an uninstall
    // registry key whose location depends on how Kodi was installed - so the
    // label goes without one rather than guessing.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = confinement;
        None
    }
}

/// Runs something that answers quickly, and treats every failure as "no
/// answer": a missing command, a non-zero exit, output that is not text.
///
/// Linux only: macOS reads the version out of a bundle's metadata and Windows
/// does not look for one at all, so building this anywhere else leaves dead
/// code that fails the clippy gate.
#[cfg(target_os = "linux")]
fn ask(command: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(command)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// The value after a `Name:` label in a block of key/value output.
#[cfg(target_os = "linux")]
fn field(text: &str, name: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix(name))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

impl Setup {
    /// The folder Kodi keeps its settings in, which is what gets shown and
    /// what a viewer would have typed.
    pub fn userdata(&self) -> &Path {
        self.file.parent().unwrap_or(&self.file)
    }

    /// How this instance reads on screen: "Kodi 20.5 (Flatpak)", or just
    /// "Kodi (Flatpak)" where the version could not be had. Never a bare
    /// path, which tells a viewer nothing about which Kodi it is.
    pub fn label(&self) -> String {
        let mut label = match &self.version {
            Some(version) => format!("Kodi {version}"),
            None => "Kodi".to_string(),
        };
        // Whichever qualifier applies, and at most one, in this order: how it
        // was installed when that is worth flagging, then whether the folder
        // was named by hand, then the ordinary case. A sandbox outranks
        // "custom" because it changes how Kodi has to start us, which is worth
        // more than where the folder came from - and "custom" outranks
        // "standard", or a hand-named folder would claim to be one of the
        // places TinePlayer looks by itself.
        let qualifier = match self.confinement {
            Confinement::None if !self.is_standard_location() => tr!("Custom"),
            confinement => confinement.describe(),
        };
        label.push_str(&format!(" ({qualifier})"));
        label
    }

    /// Whether this is one of the places TinePlayer looks by itself, as
    /// opposed to a folder somebody browsed to. Two ordinary-looking installs
    /// in the list would otherwise be indistinguishable.
    fn is_standard_location(&self) -> bool {
        candidates().iter().any(|known| known == self.userdata())
    }

    /// Whether the backup toggle should start on.
    ///
    /// On when TinePlayer has never been in this file, because then a copy
    /// preserves it as the viewer had it. Off when our own entry is already
    /// there, because a copy of our own work is worth little and re-running
    /// the wizard would otherwise leave a heap of near-identical files.
    pub fn backup_by_default(&self) -> bool {
        self.file.exists() && !self.is_configured()
    }

    /// Whether TinePlayer is set up here at all.
    pub fn is_configured(&self) -> bool {
        self.state != Registration::Absent
    }

    /// Whether Kodi has ever actually run from here. It writes guisettings.xml
    /// on first shutdown, so a directory without one is either brand new or
    /// left behind by an uninstalled Kodi.
    pub fn looks_used(&self) -> bool {
        self.userdata().join("guisettings.xml").exists()
    }
}

/// Whether TinePlayer is itself running inside a Flatpak sandbox.
///
/// Flatpak puts this file in every sandbox it starts, and it is the documented
/// way for an application to know. It matters because our own path is then a
/// path inside the sandbox, which means nothing to Kodi outside it.
fn we_are_flatpak() -> bool {
    Path::new("/.flatpak-info").exists()
}

/// What Kodi should be told to run, split the way its config file splits it.
struct Launch {
    /// Goes in `<filename>`: the program Kodi actually starts.
    filename: String,
    /// Goes in front of the video in `<args>`, with a trailing space when it
    /// is not empty. Everything between starting that program and it being
    /// TinePlayer receiving a file.
    prefix: String,
    /// Whether Kodi's hand-off should start the film or open the menu.
    play: bool,
}

impl Launch {
    /// Puts this command into a piece of the template.
    ///
    /// Both substitutions are escaped, because both are paths and a path is
    /// not XML text. `&` is legal in a Windows account name - so
    /// `C:\Users\Ben & Sue\...` is an ordinary thing to be installed under -
    /// and `<`, `>`, `"` and `'` are all legal in a POSIX filename besides.
    ///
    /// **Kodi's answer to a file it cannot parse is to ignore it, silently.**
    /// So without this the symptom is TinePlayer reporting that it registered
    /// successfully and then never appearing under "Play using...", with
    /// nothing anywhere to say why - a fault nobody would reproduce, because
    /// it depends entirely on what somebody's home folder is called.
    fn fill(&self, xml: &str) -> String {
        xml.replace(PLACEHOLDER, &escape_xml(&self.filename))
            .replace(PLACEHOLDER_ARGS, &escape_xml(&self.prefix))
            .replace(PLACEHOLDER_PLAY, if self.play { " --play" } else { "" })
    }
}

/// The five characters XML reserves, in the one order that works.
///
/// `&` first: doing it later would go back over the ampersands the other four
/// just introduced and turn `&lt;` into `&amp;lt;`.
///
/// All five rather than the three a text node strictly needs. `<filename>` is
/// a text node and `<args>` is too, so `"` and `'` would pass - but the
/// template quotes `{1}` inside `<args>`, and anything that ever moves one of
/// these into an attribute would break in a way that is invisible here.
fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Everywhere Kodi is known to keep its userdata, in the order worth trying.
fn candidates() -> Vec<PathBuf> {
    let home = dirs_home();
    let mut paths = Vec::new();

    #[cfg(target_os = "windows")]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            paths.push(PathBuf::from(appdata).join("Kodi").join("userdata"));
        }
        // The Microsoft Store build, which redirects its roaming data.
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            paths.push(
                PathBuf::from(local)
                    .join("Packages")
                    .join("XBMCFoundation.Kodi_4n2hpmxwrvr6p")
                    .join("LocalCache")
                    .join("Roaming")
                    .join("Kodi")
                    .join("userdata"),
            );
        }
    }

    #[cfg(target_os = "macos")]
    if let Some(home) = home.as_ref() {
        paths.push(
            home.join("Library")
                .join("Application Support")
                .join("Kodi")
                .join("userdata"),
        );
    }

    #[cfg(target_os = "linux")]
    if let Some(home) = home.as_ref() {
        paths.push(home.join(".kodi").join("userdata"));
        // Flatpak, then Snap.
        paths.push(
            home.join(".var")
                .join("app")
                .join("tv.kodi.Kodi")
                .join("data")
                .join("userdata"),
        );
        paths.push(
            home.join("snap")
                .join("kodi")
                .join("current")
                .join(".kodi")
                .join("userdata"),
        );
        // LibreELEC and other appliance builds, where Kodi is the system.
        paths.push(PathBuf::from("/storage/.kodi/userdata"));
    }

    let _ = home;
    paths
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Every Kodi on this machine, and every location the viewer has added by
/// hand, each with what it currently says about TinePlayer.
///
/// All of them rather than the first, because more than one can be installed
/// at once and because somebody who has set up two wants to see both - not
/// least to take TinePlayer back out of one of them.
pub fn find_all(extra: &[PathBuf]) -> Vec<Setup> {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut found = Vec::new();
    let discovered = candidates().into_iter().filter(|path| path.is_dir());
    for userdata in discovered.chain(extra.iter().cloned()) {
        if seen.contains(&userdata) {
            continue;
        }
        seen.push(userdata.clone());
        found.push(setup_at(userdata));
    }

    // The ones Kodi has actually run from first, then the rest.
    //
    // A directory Kodi has run from has guisettings.xml in it, written on
    // first shutdown. One that does not is most likely left behind by an
    // uninstalled Kodi, so it sorts last: still listed, since it might be a
    // fresh install nothing has started yet, but never the first thing read.
    //
    // **Deliberately not sorted by whether TinePlayer is set up in it**, which
    // is what this did first, on the reasoning that a configured installation
    // is the one somebody came to look at. It is, and it still must not sort
    // by that: configuring one changes its key, so the group moved to the top
    // of the pane at the moment it was configured and the group that had been
    // there dropped below it. The write went to the right file and the screen
    // said otherwise - it read exactly as though the setting had been applied
    // to the wrong installation.
    //
    // Everything left in this key is a fact about the machine rather than
    // about anything a viewer can change from this screen, which is the
    // property that keeps a group still while it is being worked on.
    found.sort_by_key(|setup| !setup.looks_used());
    found
}

/// Reads what one Kodi location currently says, whether or not anything is
/// there. A location that does not exist yet is a valid answer: the viewer
/// may be pointing at a Kodi they are about to install.
pub fn setup_at(userdata: PathBuf) -> Setup {
    let file = userdata.join("playercorefactory.xml");
    let existing = std::fs::read_to_string(&file).ok();
    let state = match &existing {
        Some(existing) => read_state(existing),
        None => Registration::Absent,
    };
    let play = existing.as_deref().is_some_and(read_play);
    let confinement = confinement_of(&userdata);
    Setup {
        version: version_of(confinement, &userdata),
        confinement,
        file,
        state,
        play,
    }
}

/// Turns whatever somebody chose into the directory to work in.
///
/// Both spellings people reach for are accepted: the userdata directory
/// itself, and the directory that contains it.
pub fn userdata_from(chosen: PathBuf) -> PathBuf {
    // Asked to find "Kodi's user data folder", somebody may reasonably stop at
    // .kodi, which contains it. Taking the userdata inside is what they meant,
    // and writing playercorefactory.xml one level too high would produce a
    // file Kodi never reads and a setup that silently does nothing.
    let inside = chosen.join("userdata");
    if inside.is_dir() { inside } else { chosen }
}

/// Whether a directory looks like Kodi's user data folder.
///
/// Asked before a folder somebody browsed to is taken at its word. The hazard
/// this exists for is quiet: pick the wrong folder and TinePlayer writes a
/// perfectly good `playercorefactory.xml` somewhere Kodi never reads, so the
/// rows say it is configured, Kodi carries on playing videos itself, and
/// nothing anywhere explains why. The likeliest way to land there is a Kodi
/// that has never been run, which has no user data folder yet at all - so the
/// folder above it gets chosen instead, and it is the one folder that looks
/// most like the right answer.
///
/// Deliberately generous, and never the last word: it decides whether to ask,
/// not whether to allow. Kodi writes most of these on first run and the rest
/// on first shutdown, and this is also the escape hatch for a layout nobody
/// here thought of, so a folder that fails this can still be used.
pub fn looks_like_userdata(path: &Path) -> bool {
    const MARKERS: [&str; 6] = [
        "guisettings.xml",
        "advancedsettings.xml",
        "sources.xml",
        "profiles.xml",
        "Database",
        "addon_data",
    ];
    MARKERS.iter().any(|marker| path.join(marker).exists())
}

/// How Kodi was installed, worked out from where it keeps its settings, since
/// that is the one thing we know about it for certain.
fn confinement_of(userdata: &Path) -> Confinement {
    let path = userdata.to_string_lossy();
    if path.contains("/.var/app/tv.kodi.Kodi") {
        Confinement::Flatpak
    } else if path.contains("/snap/kodi/") {
        Confinement::Snap
    } else {
        Confinement::None
    }
}

/// Whether our entry in this file starts the film or opens the menu.
///
/// Read from the `<args>` of our own `<player>` element and from nowhere else,
/// which is the whole difficulty: the template carries a comment saying "Add
/// --play to have the film start as soon as Kodi hands it over", so the string
/// is in the file whichever way it is configured and only its place in the
/// arguments means anything.
///
/// False for a file we are not in at all. That is not a claim about the file:
/// the row this feeds is disabled until something is set up, and "show the
/// menu" is what the absence of the flag means everywhere else.
fn read_play(xml: &str) -> bool {
    let Some(start) = xml.find("<player name=\"TinePlayer\"") else {
        return false;
    };
    let block = &xml[start..];
    // Ours ends where our player does, so a second player's arguments below it
    // are never read as ours.
    let block = match block.find("</player>") {
        Some(end) => &block[..end],
        None => block,
    };
    let Some(open) = block.find("<args>") else {
        return false;
    };
    let args = &block[open..];
    let args = match args.find("</args>") {
        Some(close) => &args[..close],
        None => args,
    };
    args.contains("--play")
}

/// What a file says about us: whether our player is in it, and whether a rule
/// hands it everything.
fn read_state(xml: &str) -> Registration {
    if !xml.contains("name=\"TinePlayer\"") {
        return Registration::Absent;
    }
    if xml.contains("player=\"TinePlayer\"") {
        Registration::Default
    } else {
        Registration::Offered
    }
}

/// The command Kodi should run, for how both programs happen to be installed.
///
/// Four combinations, and they are all real:
///
/// - **Installed normally, Kodi installed normally.** Kodi runs the executable
///   by path, which is the straightforward case.
/// - **A Flatpak, Kodi installed normally.** Our path is a path inside our own
///   sandbox and means nothing to Kodi, so it starts us the way anything else
///   outside would: `flatpak run`.
/// - **Kodi is a Flatpak.** Then Kodi starts external players inside *its*
///   sandbox, which is the freedesktop runtime - no GTK 4, and no sight of the
///   user's home directory - so whatever the command was has to be handed to
///   `flatpak-spawn --host` to be run on the machine instead. This works
///   whether TinePlayer is a Flatpak or a build from source, because the
///   command runs outside the sandbox either way. It needs a permission Kodi
///   does not ship with; [`permission_note`] is what says so.
/// - **Kodi is a Snap.** The same problem with no equivalent way out, so the
///   command is written for the host and [`permission_note`] is honest about
///   it possibly not working.
///
/// On Windows a build from source is launched through the shim script, because
/// Kodi starts external players from its own program directory and Windows
/// searches there for libraries first - so GStreamer's DLLs lose to Kodi's
/// copies and the player dies before `main`. With the libraries beside the
/// executable there is nothing to lose to.
fn launch(confinement: Confinement, play: bool) -> Result<Launch, String> {
    let launch = if we_are_flatpak() {
        Launch {
            filename: "flatpak".to_string(),
            prefix: format!("run {FLATPAK_ID} "),
            play,
        }
    } else {
        let exe = std::env::current_exe()
            .map_err(|e| format!("Can't work out where TinePlayer is running from: {e}"))?;

        #[cfg(target_os = "windows")]
        let exe = {
            let packaged = exe
                .parent()
                .is_some_and(|beside| beside.join("gstreamer-1.0-0.dll").exists());
            let shim = (!packaged)
                .then(|| {
                    // target/release/tineplayer.exe, so the tree is three up.
                    exe.parent()
                        .and_then(Path::parent)
                        .and_then(Path::parent)
                        .map(|root| root.join("launch-tineplayer-windows.cmd"))
                })
                .flatten()
                .filter(|path| path.exists());
            shim.unwrap_or(exe)
        };

        Launch {
            filename: exe.display().to_string(),
            prefix: String::new(),
            play,
        }
    };

    // Wrapped last, so it carries whichever of the above we ended up with.
    Ok(escape_from(launch, confinement))
}

/// Wraps a command so that a confined Kodi runs it on the machine rather than
/// inside itself.
///
/// Separate from [`launch`] because this half is decided entirely by its two
/// arguments, while the other half depends on where this process is running
/// from - and this is the half worth having tests for.
fn escape_from(launch: Launch, confinement: Confinement) -> Launch {
    match confinement {
        // A Snap gets the command unwrapped. There is no supported equivalent
        // of flatpak-spawn, so this is written in hope, and permission_note
        // says as much rather than pretending otherwise.
        Confinement::None | Confinement::Snap => launch,
        Confinement::Flatpak => Launch {
            // The absolute path rather than the bare name: this one is
            // resolved inside Kodi's sandbox, where flatpak-spawn is part of
            // the runtime and always exactly here.
            prefix: format!("--host {} {}", launch.filename, launch.prefix),
            filename: "/usr/bin/flatpak-spawn".to_string(),
            play: launch.play,
        },
    }
}

/// Something the viewer has to do themselves, because TinePlayer either
/// cannot do it or should not.
///
/// Deliberately a thing to read and run rather than something done quietly on
/// their behalf. Granting Kodi the permission below lets it run *any* command
/// on the machine, which is a real widening of what an installed application
/// can do, and that is not a choice to make for somebody without telling them.
pub struct ManualStep {
    /// One line: what still has to happen.
    pub what: Cow<'static, str>,
    /// Why it is needed, in terms of what is actually going on.
    pub why: Cow<'static, str>,
    /// The command to run, if there is one. Shown to be copied.
    ///
    /// **Not translated, and neither is `undo`.** Both are shell commands to
    /// be typed exactly as written, and a translated command is a broken one.
    pub command: Option<&'static str>,
    /// What it costs, so consent is informed rather than assumed.
    pub cost: Cow<'static, str>,
    /// How to undo it afterwards. A command, so not translated - see above.
    pub undo: Option<&'static str>,
}

/// The manual step for a given Kodi, or `None` when there is nothing left to
/// do by hand.
pub fn manual_step(confinement: Confinement) -> Option<ManualStep> {
    match confinement {
        Confinement::None => None,
        Confinement::Flatpak => Some(ManualStep {
            what: tr!("Allow Kodi to start programs outside its sandbox"),
            why: tr!(
                "Kodi is installed as a Flatpak. It starts an external player inside its own \
                 sandbox, where TinePlayer is not installed and your files are not visible, \
                 so it has to be allowed to run the command on the machine instead."
            ),
            command: Some(
                "flatpak override --user --talk-name=org.freedesktop.Flatpak tv.kodi.Kodi",
            ),
            cost: tr!(
                "This lets Kodi run anything on this machine, not only TinePlayer. TinePlayer \
                 will not run it for you."
            ),
            undo: Some("flatpak override --user --reset tv.kodi.Kodi"),
        }),
        // A Snap cannot be configured at all, so there is no step to
        // describe. See Confinement::supported.
        Confinement::Snap => None,
    }
}

/// Our `<player>` element, taken from the template so there is one copy of it
/// rather than a second buried in this file.
fn player_element(launch: &Launch) -> Result<String, String> {
    let start = TEMPLATE
        .find("    <player name=\"TinePlayer\"")
        .ok_or("The bundled player template is missing its player element.")?;
    let end = TEMPLATE[start..]
        .find("</player>")
        .map(|offset| start + offset + "</player>".len())
        .ok_or("The bundled player template is missing its closing tag.")?;
    Ok(launch.fill(&TEMPLATE[start..end]))
}

/// The rule that hands Kodi's video playback to us.
fn rules_element() -> &'static str {
    "  <rules action=\"prepend\">\n    <rule video=\"true\" player=\"TinePlayer\" />\n  </rules>"
}

/// Writes the whole template out, for a machine with no such file yet.
fn fresh(launch: &Launch, as_default: bool) -> String {
    let mut xml = launch.fill(TEMPLATE);
    if as_default {
        xml = xml
            .lines()
            .filter(|line| {
                !line.contains("<!-- RULES START -->") && !line.contains("<!-- RULES END -->")
            })
            .collect::<Vec<_>>()
            .join("\n");
    } else {
        let start = xml.find("<!-- RULES START -->");
        let end = xml.find("<!-- RULES END -->");
        if let (Some(start), Some(end)) = (start, end) {
            let end = end + "<!-- RULES END -->".len();
            let end = xml[end..]
                .find('\n')
                .map(|offset| end + offset + 1)
                .unwrap_or(end);
            xml.replace_range(start..end, "");
        }
    }
    if !xml.ends_with('\n') {
        xml.push('\n');
    }
    xml
}

/// The start of the line an offset falls on, so an insertion lands above a
/// line rather than inside it.
fn line_start(xml: &str, at: usize) -> usize {
    xml[..at].rfind('\n').map(|found| found + 1).unwrap_or(0)
}

/// Puts our player into a file that already exists, leaving everything else
/// in it alone.
fn insert(existing: &str, launch: &Launch, as_default: bool) -> Result<String, String> {
    let mut xml = remove_from(existing)?;

    let anchor = xml
        .find("</players>")
        .ok_or("Kodi's player file has no <players> section to add to.")?;
    // On its own line above the closing tag, rather than in front of it:
    // inserting at the tag itself takes the indentation that belongs to it,
    // and then the file no longer matches what it was once ours comes back
    // out again.
    let at = line_start(&xml, anchor);
    xml.insert_str(at, &format!("{}\n", player_element(launch)?));

    if as_default {
        let anchor = xml
            .find("</playercorefactory>")
            .ok_or("Kodi's player file has no <playercorefactory> section.")?;
        let at = line_start(&xml, anchor);
        xml.insert_str(at, &format!("{}\n", rules_element()));
    }
    Ok(xml)
}

/// Cuts our player, and any rule naming it, out of a file. Everything that is
/// not ours is left untouched, including whitespace and comments.
fn remove_from(existing: &str) -> Result<String, String> {
    let mut xml = existing.to_string();

    while let Some(start) = xml.find("<player name=\"TinePlayer\"") {
        let end = xml[start..]
            .find("</player>")
            .map(|offset| start + offset + "</player>".len())
            .ok_or("Found our player in Kodi's file but not where it ends.")?;
        // Back up over the indentation and forward over the newline, so
        // removing a block does not leave a blank line behind it.
        let start = xml[..start].rfind('\n').map(|at| at + 1).unwrap_or(start);
        let end = xml[end..]
            .find('\n')
            .map(|offset| end + offset + 1)
            .unwrap_or(end);
        xml.replace_range(start..end, "");
    }

    while let Some(at) = xml.find("player=\"TinePlayer\"") {
        let start = xml[..at].rfind('\n').map(|from| from + 1).unwrap_or(0);
        let end = xml[at..]
            .find('\n')
            .map(|offset| at + offset + 1)
            .unwrap_or(xml.len());
        xml.replace_range(start..end, "");
    }

    // An empty rules block left behind by that is ours to tidy: it only ever
    // held our rule.
    let empty = "  <rules action=\"prepend\">\n  </rules>\n";
    xml = xml.replace(empty, "");
    Ok(xml)
}

/// Copies the file before changing it, named for when it was taken.
/// What a backup of this file would be called.
///
/// Worked out separately from taking it, so the summary can name the file it
/// is about to write and then write exactly that one. Computing it twice
/// would produce two different names, a second or so apart, and the screen
/// would have promised a file that never appeared.
pub fn backup_path(file: &Path) -> PathBuf {
    // Named for when it was taken, in a form somebody can read at a glance in
    // a file listing, the way the scripts this replaced did it.
    let stamp = glib::DateTime::now_local()
        .and_then(|now| now.format("%Y%m%d-%H%M%S"))
        .map(|stamp| stamp.to_string())
        .unwrap_or_else(|_| "backup".to_string());
    file.with_extension(format!("xml.{stamp}.bak"))
}

fn back_up(file: &Path, to: &Path) -> Result<(), String> {
    std::fs::copy(file, to).map_err(|e| format!("Couldn't back up {}: {e}", file.display()))?;
    Ok(())
}

/// Sets Kodi up, changes how it is set up, or takes us back out of it.
///
/// Says nothing on success. It used to return a sentence for the wizard's last
/// screen to show, and that screen is gone: the rows on the Integrations pane
/// state what is configured by reading it back out of the file, so a message
/// saying what was just done would only repeat the row that says it.
pub fn apply(
    setup: &Setup,
    want: Registration,
    backup: Option<&Path>,
    play: bool,
) -> Result<(), String> {
    let existing = std::fs::read_to_string(&setup.file).ok();

    let xml = match (&existing, want) {
        (Some(existing), Registration::Absent) => remove_from(existing)?,
        (Some(existing), _) => insert(
            existing,
            &launch(setup.confinement, play)?,
            want == Registration::Default,
        )?,
        (None, Registration::Absent) => return Ok(()),
        (None, _) => fresh(
            &launch(setup.confinement, play)?,
            want == Registration::Default,
        ),
    };

    // Nothing to do, so nothing done: no write, and above all no backup.
    if existing.as_deref() == Some(xml.as_str()) {
        return Ok(());
    }

    // Decided by the caller, which takes one the first time TinePlayer edits a
    // file and not on later changes to our own entry. Removal never takes one:
    // undoing our own edit is not something worth keeping a copy of the file
    // for.
    if let Some(to) = backup.filter(|_| want != Registration::Absent && setup.file.exists()) {
        back_up(&setup.file, to)?;
    }

    if let Some(folder) = setup.file.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|e| format!("Couldn't reach {}: {e}", folder.display()))?;
    }
    std::fs::write(&setup.file, xml)
        .map_err(|e| format!("Couldn't write {}: {e}", setup.file.display()))?;

    // What was written and where. Kodi ignores a player file it cannot parse
    // and says nothing about it, so "I set it up and TinePlayer never appears"
    // is answered by this line plus the file it names - which is the only
    // evidence there is that our side of it worked.
    log::info!("Kodi: wrote {:?} to {}", want, setup.file.display());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A build from source, started by path: the plain case the others are
    /// variations on.
    fn direct() -> Launch {
        Launch {
            filename: "/opt/tineplayer".to_string(),
            prefix: String::new(),
            play: false,
        }
    }

    /// A file with somebody else's player in it, which is the case that must
    /// survive untouched.
    const FOREIGN: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<playercorefactory>
  <players>
    <player name="MPV" type="ExternalPlayer" video="true">
      <filename>/usr/bin/mpv</filename>
      <!-- Somebody's own comment -->
    </player>
  </players>
  <rules action="prepend">
    <rule video="true" player="MPV" />
  </rules>
</playercorefactory>
"#;

    #[test]
    fn adds_without_disturbing_another_player() {
        let out = insert(FOREIGN, &direct(), false).unwrap();
        assert!(out.contains("name=\"MPV\""));
        assert!(out.contains("/usr/bin/mpv"));
        assert!(out.contains("Somebody's own comment"));
        assert!(out.contains("player=\"MPV\""));
        assert!(out.contains("name=\"TinePlayer\""));
        assert!(out.contains("/opt/tineplayer"));
        assert_eq!(read_state(&out), Registration::Offered);
    }

    #[test]
    fn as_default_adds_a_rule() {
        let out = insert(FOREIGN, &direct(), true).unwrap();
        assert_eq!(read_state(&out), Registration::Default);
        assert!(out.contains("player=\"MPV\""));
    }

    #[test]
    fn removing_puts_the_file_back_as_it_was() {
        let added = insert(FOREIGN, &direct(), true).unwrap();
        let removed = remove_from(&added).unwrap();
        assert_eq!(read_state(&removed), Registration::Absent);
        assert!(!removed.contains("tineplayer"));
        assert_eq!(removed, FOREIGN);
    }

    #[test]
    fn switching_mode_does_not_duplicate_us() {
        let once = insert(FOREIGN, &direct(), false).unwrap();
        let twice = insert(&once, &direct(), true).unwrap();
        assert_eq!(twice.matches("name=\"TinePlayer\"").count(), 1);
        assert_eq!(twice.matches("player=\"TinePlayer\"").count(), 1);
    }

    /// A Flatpak TinePlayer, which cannot be started by the path it sees.
    fn as_flatpak() -> Launch {
        Launch {
            filename: "flatpak".to_string(),
            prefix: format!("run {FLATPAK_ID} "),
            play: false,
        }
    }

    /// The four ways the two programs can be installed, and what each has to
    /// tell Kodi to run. The whole point is that none of them is assumed.
    #[test]
    fn the_command_suits_how_both_are_installed() {
        // Both installed normally: run the executable, nothing in the way.
        let plain = escape_from(direct(), Confinement::None);
        assert_eq!(plain.filename, "/opt/tineplayer");
        assert_eq!(plain.prefix, "");

        // We are a Flatpak, Kodi is not: Kodi starts us the way anything on
        // the machine would.
        let ours = escape_from(as_flatpak(), Confinement::None);
        assert_eq!(ours.filename, "flatpak");
        assert_eq!(ours.prefix, format!("run {FLATPAK_ID} "));

        // Kodi is a Flatpak and we are a build from source. This is the case
        // that looks impossible and is not: flatpak-spawn --host runs the
        // command outside Kodi's sandbox, where that build's libraries are.
        let escaped = escape_from(direct(), Confinement::Flatpak);
        assert_eq!(escaped.filename, "/usr/bin/flatpak-spawn");
        assert_eq!(escaped.prefix, "--host /opt/tineplayer ");

        // Both Flatpaks: out of Kodi's sandbox, then into ours.
        let both = escape_from(as_flatpak(), Confinement::Flatpak);
        assert_eq!(both.filename, "/usr/bin/flatpak-spawn");
        assert_eq!(both.prefix, format!("--host flatpak run {FLATPAK_ID} "));
    }

    /// The command has to survive into the file as something Kodi can run,
    /// with the video still the argument after it.
    #[test]
    fn a_wrapped_command_lands_correctly_in_the_file() {
        let out = insert(
            FOREIGN,
            &escape_from(as_flatpak(), Confinement::Flatpak),
            false,
        )
        .unwrap();
        assert!(out.contains("<filename>/usr/bin/flatpak-spawn</filename>"));
        assert!(out.contains(&format!(
            "<args>--host flatpak run {FLATPAK_ID} \"{{1}}\" --fullscreen --kodi</args>"
        )));
        // No placeholder left anywhere in what we wrote.
        assert!(!out.contains(PLACEHOLDER));
        assert!(!out.contains(PLACEHOLDER_ARGS));
    }

    /// Whichever shape the command took, taking TinePlayer back out has to
    /// leave the file exactly as it was found.
    #[test]
    fn removing_a_wrapped_command_still_restores_the_file() {
        for launch in [
            escape_from(direct(), Confinement::None),
            escape_from(as_flatpak(), Confinement::None),
            escape_from(direct(), Confinement::Flatpak),
            escape_from(as_flatpak(), Confinement::Flatpak),
        ] {
            let added = insert(FOREIGN, &launch, true).unwrap();
            assert_eq!(remove_from(&added).unwrap(), FOREIGN);
        }
    }

    /// The hand-over choice has to reach Kodi's arguments, and has to be
    /// absent rather than false when the menu is wanted: there is no
    /// --no-play, so writing nothing is what says "show the menu".
    #[test]
    fn the_handover_choice_lands_in_the_arguments() {
        let playing = Launch {
            filename: "/opt/tineplayer".to_string(),
            prefix: String::new(),
            play: true,
        };
        // The arguments line rather than the whole file: the player element
        // carries a comment explaining how to add the flag by hand, so the
        // string appears either way and only its place in <args> means
        // anything.
        let args_of = |xml: &str| -> String {
            xml.lines()
                .find(|line| line.contains("<args>"))
                .unwrap_or_default()
                .to_string()
        };

        let playing = args_of(&insert(FOREIGN, &playing, false).unwrap());
        assert!(playing.contains("--kodi --play</args>"));

        let menu = args_of(&insert(FOREIGN, &direct(), false).unwrap());
        assert!(menu.contains("--kodi</args>"));
        assert!(!menu.contains("--play"));
    }

    /// Which Kodi we are looking at is worked out from where its settings are,
    /// because that is the one thing we know for certain about it.
    #[test]
    fn confinement_is_read_from_the_path() {
        assert_eq!(
            confinement_of(Path::new("/home/vi/.kodi/userdata")),
            Confinement::None
        );
        assert_eq!(
            confinement_of(Path::new("/home/vi/.var/app/tv.kodi.Kodi/data/userdata")),
            Confinement::Flatpak
        );
        assert_eq!(
            confinement_of(Path::new("/home/vi/snap/kodi/current/.kodi/userdata")),
            Confinement::Snap
        );
    }

    /// A confined Kodi cannot start anything without being allowed to, and the
    /// viewer is the one who has to allow it. Shown on the Sandbox Permission
    /// row, which exists only for the installation that has one of these.
    #[test]
    fn a_confined_kodi_says_what_has_to_be_done_by_hand() {
        assert!(manual_step(Confinement::None).is_none());
        let flatpak = manual_step(Confinement::Flatpak).expect("a Flatpak Kodi needs a step");
        let command = flatpak.command.expect("and a command to run");
        assert!(command.contains("flatpak override"));
        assert!(command.contains("tv.kodi.Kodi"));
        // Says what it costs, rather than only how to do it.
        assert!(flatpak.cost.contains("anything on this machine"));
        // A Snap is refused before it can be configured, so it has no
        // manual step to describe.
        assert!(manual_step(Confinement::Snap).is_none());
    }

    /// The handover row reads its value back out of the file, and the file is
    /// laid out to make that easy to get wrong: our own template carries a
    /// comment saying to add `--play`, so the string is there either way.
    #[test]
    fn the_handover_is_read_from_the_arguments_and_not_the_comments() {
        let playing = Launch {
            filename: "/opt/tineplayer".to_string(),
            prefix: String::new(),
            play: true,
        };
        // Written into a file of our own making, comments and all, which is
        // the case the naive check gets wrong.
        assert!(read_play(&fresh(&playing, false)));
        assert!(!read_play(&fresh(&direct(), false)));
        // And into somebody else's file, where only our element is ours to
        // read.
        assert!(read_play(&insert(FOREIGN, &playing, false).unwrap()));
        assert!(!read_play(&insert(FOREIGN, &direct(), false).unwrap()));
        // A file we are not in at all says "menu", which is what no flag
        // means everywhere else.
        assert!(!read_play(FOREIGN));
    }

    /// A folder browsed to by hand is checked before it is taken at its word,
    /// because getting it wrong fails silently: TinePlayer writes a good file
    /// somewhere Kodi never reads.
    ///
    /// The case worth naming is a Kodi that has never been run, which has no
    /// user data folder at all - so the folder above it is what gets chosen,
    /// and that is the one folder most likely to look right.
    #[test]
    fn a_folder_is_checked_for_being_kodis_user_data() {
        let temp = std::env::temp_dir().join("tineplayer-userdata-test");
        let never_run = temp.join(".kodi");
        let userdata = never_run.join("userdata");
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&never_run).unwrap();

        // Kodi installed and never started: the folder exists, the user data
        // inside it does not, and nothing in it says otherwise.
        assert!(!looks_like_userdata(&never_run));
        // And resolving stops there rather than inventing a userdata folder,
        // which is exactly why the check above has to exist.
        assert_eq!(userdata_from(never_run.clone()), never_run);

        // Once Kodi has run, both answers change.
        std::fs::create_dir_all(&userdata).unwrap();
        std::fs::write(userdata.join("guisettings.xml"), "<settings/>").unwrap();
        assert!(looks_like_userdata(&userdata));
        assert!(!looks_like_userdata(&never_run));
        // Stopping at the folder above is a reasonable reading of "Kodi's
        // folder", and lands on the right one.
        assert_eq!(userdata_from(never_run.clone()), userdata);
        assert_eq!(userdata_from(userdata.clone()), userdata);

        let _ = std::fs::remove_dir_all(&temp);
    }

    /// Nothing a viewer can change from the pane may change where a group
    /// sits on it.
    ///
    /// The ordering is what puts `Item::KodiType(n)` against an installation,
    /// so a key that moves when a setting is applied moves the groups under
    /// the hand that applied it. Configuring the second of two installations
    /// sent it to the top and dropped the other one below it, which reads as
    /// the setting having landed on the wrong installation - the file written
    /// was right and the screen said it was not.
    #[test]
    fn ordering_does_not_depend_on_anything_this_screen_can_change() {
        let temp = std::env::temp_dir().join("tineplayer-order-test");
        let _ = std::fs::remove_dir_all(&temp);
        let ours = temp.join("custom");
        std::fs::create_dir_all(&ours).unwrap();

        // The same folder, before and after being configured. Nothing else
        // about it differs, so its place in the list must not either.
        let before = setup_at(ours.clone());
        assert_eq!(before.state, Registration::Absent);
        let key_before = !before.looks_used();

        std::fs::write(&before.file, fresh(&direct(), false)).unwrap();
        let after = setup_at(ours.clone());
        assert_eq!(after.state, Registration::Offered);
        assert_eq!(key_before, !after.looks_used());

        let _ = std::fs::remove_dir_all(&temp);
    }

    /// The qualifier on a heading, which is every installation's name now that
    /// each one heads its own group of rows.
    #[test]
    fn every_installation_says_how_it_was_installed() {
        assert_eq!(Confinement::None.describe(), "Default Installation");
        assert_eq!(Confinement::Flatpak.describe(), "Flatpak");
        assert_eq!(Confinement::Snap.describe(), "Snap");
    }

    /// Removal is an entry in the Player Type chooser rather than an action of
    /// its own, and it is the one entry that says what pressing it does rather
    /// than what state it leaves behind.
    #[test]
    fn the_chooser_names_removal_only_when_there_is_something_to_remove() {
        assert_eq!(Registration::Absent.describe(), "Not configured");
        assert_eq!(Registration::Absent.choice(false), "Not configured");
        assert_eq!(Registration::Absent.choice(true), "Remove configuration");
        // The other two are states either way, so they read alike in both.
        for state in [Registration::Offered, Registration::Default] {
            assert_eq!(state.choice(false), state.describe());
            assert_eq!(state.choice(true), state.describe());
        }
    }

    #[test]
    fn a_fresh_file_is_valid_either_way() {
        assert_eq!(read_state(&fresh(&direct(), false)), Registration::Offered);
        assert_eq!(read_state(&fresh(&direct(), true)), Registration::Default);
        assert!(!fresh(&direct(), false).contains("RULES START"));
    }
}

#[cfg(test)]
mod escaping_tests {
    use super::*;

    #[test]
    fn the_five_reserved_characters_are_escaped() {
        assert_eq!(
            escape_xml(r#"a & b < c > d " e ' f"#),
            "a &amp; b &lt; c &gt; d &quot; e &apos; f"
        );
    }

    /// The ampersand has to go first, or it re-escapes what the others wrote.
    #[test]
    fn escaping_is_not_applied_twice() {
        assert_eq!(escape_xml("<"), "&lt;");
        assert!(!escape_xml("<").contains("&amp;"));
    }

    #[test]
    fn an_ordinary_path_is_untouched() {
        let path = r"C:\Program Files\TinePlayer\TinePlayer.exe";
        assert_eq!(escape_xml(path), path);
    }

    /// The case this exists for: a Windows account name may contain `&`, and
    /// the file Kodi gets has to still be well-formed XML.
    #[test]
    fn an_ampersand_in_the_path_survives_into_the_file() {
        let launch = Launch {
            filename: r"C:\Users\Ben & Sue\TinePlayer.exe".to_string(),
            prefix: String::new(),
            play: false,
        };
        let out = fresh(&launch, false);
        assert!(
            out.contains(r"<filename>C:\Users\Ben &amp; Sue\TinePlayer.exe</filename>"),
            "{out}"
        );
        // A bare ampersand anywhere is what makes Kodi discard the file, so
        // the whole document is checked rather than just the one element.
        for (at, _) in out.match_indices('&') {
            let tail = &out[at..];
            assert!(
                ["&amp;", "&lt;", "&gt;", "&quot;", "&apos;"]
                    .iter()
                    .any(|entity| tail.starts_with(entity)),
                "bare & at {at}: {}",
                &tail[..tail.len().min(40)]
            );
        }
    }

    /// A Flatpak writes its path into `<args>` rather than `<filename>`, so
    /// that substitution needs the same treatment.
    #[test]
    fn the_args_prefix_is_escaped_too() {
        let launch = Launch {
            filename: "/usr/bin/flatpak-spawn".to_string(),
            prefix: "--host /home/a & b/tineplayer ".to_string(),
            play: false,
        };
        let out = fresh(&launch, false);
        assert!(out.contains("/home/a &amp; b/tineplayer"), "{out}");
    }
}
