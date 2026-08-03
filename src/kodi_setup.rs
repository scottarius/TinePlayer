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
use std::path::{Path, PathBuf};

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
    /// What this state is called, wherever it is shown. The same two names
    /// the wizard offers when choosing, so that picking one and later reading
    /// it back are plainly the same thing.
    pub fn describe(self) -> &'static str {
        match self {
            Registration::Absent => "Not set up",
            Registration::Offered => "Optional Player",
            Registration::Default => "Default Player",
        }
    }
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
    pub fn unsupported_reason(self) -> Option<&'static str> {
        (!self.supported()).then_some("Cannot start other programs")
    }

    /// What to add after the version to tell one Kodi from another, or
    /// `None` when there is nothing worth saying.
    ///
    /// An ordinary install gets no qualifier: the point of this is to flag a
    /// sandbox, which changes how Kodi has to start TinePlayer, and calling
    /// the usual case "installed normally" labels a thing by the quality it
    /// does not have.
    pub fn describe(self) -> Option<&'static str> {
        match self {
            Confinement::None => None,
            Confinement::Flatpak => Some("Flatpak"),
            Confinement::Snap => Some("Snap"),
        }
    }
}

/// Where Kodi keeps its settings, how it was installed, and what it currently
/// says about us.
pub struct Setup {
    pub file: PathBuf,
    pub state: Registration,
    pub confinement: Confinement,
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
        // Whichever qualifier applies, and at most one: a folder chosen by
        // hand is only worth mentioning when nothing more specific is.
        let qualifier = self
            .confinement
            .describe()
            .map(str::to_string)
            .or_else(|| (!self.is_standard_location()).then(|| "custom".to_string()));
        if let Some(qualifier) = qualifier {
            label.push_str(&format!(" ({qualifier})"));
        }
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
    fn fill(&self, xml: &str) -> String {
        xml.replace(PLACEHOLDER, &self.filename)
            .replace(PLACEHOLDER_ARGS, &self.prefix)
            .replace(PLACEHOLDER_PLAY, if self.play { " --play" } else { "" })
    }
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

    // A directory Kodi has actually run from has guisettings.xml in it,
    // written on first shutdown. One that does not is most likely left behind
    // by an uninstalled Kodi, so it sorts last: still offered, since it might
    // be a fresh install nothing has run yet, but never the first suggestion.
    found.sort_by_key(|setup| !setup.looks_used());
    found
}

/// Reads what one Kodi location currently says, whether or not anything is
/// there. A location that does not exist yet is a valid answer: the viewer
/// may be pointing at a Kodi they are about to install.
pub fn setup_at(userdata: PathBuf) -> Setup {
    let file = userdata.join("playercorefactory.xml");
    let state = match std::fs::read_to_string(&file) {
        Ok(existing) => read_state(&existing),
        Err(_) => Registration::Absent,
    };
    let confinement = confinement_of(&userdata);
    Setup {
        version: version_of(confinement, &userdata),
        confinement,
        file,
        state,
    }
}

/// Turns whatever somebody typed into the directory to work in.
///
/// Both spellings people reach for are accepted: the userdata directory
/// itself, and the player file inside it. `~` is expanded, because a path
/// typed by hand is as likely to start with it as not.
pub fn userdata_from(chosen: PathBuf) -> PathBuf {
    // Asked to find "Kodi's userdata folder", somebody may reasonably stop at
    // .kodi, which contains it. Taking the userdata inside is what they meant,
    // and writing playercorefactory.xml one level too high would produce a
    // file Kodi never reads and a setup that silently does nothing.
    let inside = chosen.join("userdata");
    if inside.is_dir() { inside } else { chosen }
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
                    // target/release/TinePlayer.exe, so the tree is three up.
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
    pub what: &'static str,
    /// Why it is needed, in terms of what is actually going on.
    pub why: &'static str,
    /// The command to run, if there is one. Shown to be copied.
    pub command: Option<&'static str>,
    /// What it costs, so consent is informed rather than assumed.
    pub cost: &'static str,
    /// How to undo it afterwards.
    pub undo: Option<&'static str>,
}

/// The manual step for a given Kodi, or `None` when there is nothing left to
/// do by hand.
pub fn manual_step(confinement: Confinement) -> Option<ManualStep> {
    match confinement {
        Confinement::None => None,
        Confinement::Flatpak => Some(ManualStep {
            what: "Allow Kodi to start programs outside its sandbox",
            why: "Kodi is installed as a Flatpak. It starts an external player \
                  inside its own sandbox, where TinePlayer is not installed and \
                  your files are not visible, so it has to be allowed to run the \
                  command on the machine instead.",
            command: Some(
                "flatpak override --user --talk-name=org.freedesktop.Flatpak tv.kodi.Kodi",
            ),
            cost: "This lets Kodi run anything on this machine, not only \
                   TinePlayer. TinePlayer will not run it for you.",
            undo: Some("flatpak override --user --reset tv.kodi.Kodi"),
        }),
        // A Snap cannot be configured at all, so there is no step to
        // describe. See Confinement::supported.
        Confinement::Snap => None,
    }
}

/// The same thing as flowing text, for the summary at the end and for anyone
/// reading the message rather than the wizard.
fn permission_note(confinement: Confinement) -> Option<String> {
    let step = manual_step(confinement)?;
    let mut note = format!("{}\n\n{}", step.what, step.why);
    if let Some(command) = step.command {
        note.push_str(&format!(
            "\n\nRun this once, in a terminal:\n\n    {command}"
        ));
    }
    note.push_str(&format!("\n\n{}", step.cost));
    if let Some(undo) = step.undo {
        note.push_str(&format!("\n\nTo undo it:\n\n    {undo}"));
    }
    Some(note)
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

/// Sets Kodi up, or takes us back out of it. Returns what to tell the viewer.
pub fn apply(
    setup: &Setup,
    want: Registration,
    backup: Option<&Path>,
    play: bool,
) -> Result<String, String> {
    let existing = std::fs::read_to_string(&setup.file).ok();

    let xml = match (&existing, want) {
        (Some(existing), Registration::Absent) => remove_from(existing)?,
        (Some(existing), _) => insert(
            existing,
            &launch(setup.confinement, play)?,
            want == Registration::Default,
        )?,
        (None, Registration::Absent) => return Ok("Kodi was not set up to begin with.".to_string()),
        (None, _) => fresh(
            &launch(setup.confinement, play)?,
            want == Registration::Default,
        ),
    };

    // Nothing to do, so nothing done: no write, and above all no backup.
    if existing.as_deref() == Some(xml.as_str()) {
        return Ok("Kodi is already set up that way.".to_string());
    }

    // Asked for, rather than decided here. The wizard offers it with a
    // sensible default - on when TinePlayer has never touched this file, off
    // when we are only updating our own entry - but the choice is the
    // viewer's, and removal never takes one: undoing our own edit is not
    // something worth keeping a copy of the file for.
    let backup = match backup.filter(|_| want != Registration::Absent && setup.file.exists()) {
        Some(to) => {
            back_up(&setup.file, to)?;
            Some(to.to_path_buf())
        }
        None => None,
    };

    if let Some(folder) = setup.file.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|e| format!("Couldn't reach {}: {e}", folder.display()))?;
    }
    std::fs::write(&setup.file, xml)
        .map_err(|e| format!("Couldn't write {}: {e}", setup.file.display()))?;

    let done = match want {
        Registration::Absent => "TinePlayer removed from Kodi.",
        Registration::Offered => "TinePlayer is now an optional player in Kodi.",
        Registration::Default => "TinePlayer is now Kodi's default player.",
    };
    let mut message = match backup {
        Some(backup) => format!(
            "{done}\nThe previous file was copied to {}.\nRestart Kodi for it to notice.",
            backup.display()
        ),
        None => format!("{done}\nRestart Kodi for it to notice."),
    };

    // Only when there is something to run: taking us back out of Kodi needs no
    // permission, and there is no point telling somebody to widen what Kodi
    // may do at the moment they stop using it.
    if want != Registration::Absent
        && let Some(note) = permission_note(setup.confinement)
    {
        message.push_str("\n\n");
        message.push_str(&note);
    }
    Ok(message)
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
    /// viewer is the one who has to allow it.
    #[test]
    fn a_confined_kodi_says_what_has_to_be_done_by_hand() {
        assert!(permission_note(Confinement::None).is_none());
        let flatpak = permission_note(Confinement::Flatpak).expect("a Flatpak Kodi needs a note");
        assert!(flatpak.contains("flatpak override"));
        assert!(flatpak.contains("tv.kodi.Kodi"));
        // Says what it costs, rather than only how to do it.
        assert!(flatpak.contains("anything on this machine"));
        // A Snap is refused before it can be configured, so it has no
        // manual step to describe.
        assert!(permission_note(Confinement::Snap).is_none());
    }

    #[test]
    fn a_fresh_file_is_valid_either_way() {
        assert_eq!(read_state(&fresh(&direct(), false)), Registration::Offered);
        assert_eq!(read_state(&fresh(&direct(), true)), Registration::Default);
        assert!(!fresh(&direct(), false).contains("RULES START"));
    }
}
