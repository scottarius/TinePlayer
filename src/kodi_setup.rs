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
//! out and everything else is left exactly as it was. A copy is taken first,
//! every time, and nothing is ever deleted.

use gtk::glib;
use std::path::{Path, PathBuf};

/// The template that ships with the source, used whole when there is no file
/// yet and mined for its `<player>` element when there is. Embedded rather
/// than read from disk so a packaged build needs nothing beside it.
const TEMPLATE: &str = include_str!("../data/templates/playercorefactory.xml");

/// The placeholder the template carries where the command belongs.
const PLACEHOLDER: &str = "TINEPLAYER_BINARY";

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
    pub fn describe(self) -> &'static str {
        match self {
            Registration::Absent => "Not set up",
            Registration::Offered => "Offered under \"Play using...\"",
            Registration::Default => "Playing every video",
        }
    }
}

/// Where Kodi keeps its settings, and what it currently says about us.
pub struct Setup {
    pub file: PathBuf,
    pub state: Registration,
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

/// Finds Kodi and reads what it currently says about TinePlayer.
///
/// `None` means no Kodi userdata directory was found, which is the answer for
/// a machine that does not have Kodi installed.
pub fn find() -> Option<Setup> {
    let userdata = candidates().into_iter().find(|path| path.is_dir())?;
    let file = userdata.join("playercorefactory.xml");
    let state = match std::fs::read_to_string(&file) {
        Ok(existing) => read_state(&existing),
        Err(_) => Registration::Absent,
    };
    Some(Setup { file, state })
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

/// The command Kodi should run.
///
/// A packaged build is launched directly. A build from source is launched
/// through the shim script, because Kodi starts external players from its own
/// program directory and Windows searches there for libraries first - so
/// GStreamer's DLLs lose to Kodi's copies and the player dies before `main`.
/// With the libraries beside the executable there is nothing to lose to.
fn command() -> Result<String, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("Can't work out where TinePlayer is running from: {e}"))?;

    #[cfg(target_os = "windows")]
    {
        let packaged = exe
            .parent()
            .is_some_and(|beside| beside.join("gstreamer-1.0-0.dll").exists());
        if !packaged {
            // target/release/TinePlayer.exe, so the source tree is three up.
            let shim = exe
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .map(|root| root.join("launch-tineplayer-windows.cmd"));
            if let Some(shim) = shim.filter(|path| path.exists()) {
                return Ok(shim.display().to_string());
            }
        }
    }

    Ok(exe.display().to_string())
}

/// Our `<player>` element, taken from the template so there is one copy of it
/// rather than a second buried in this file.
fn player_element(command: &str) -> Result<String, String> {
    let start = TEMPLATE
        .find("    <player name=\"TinePlayer\"")
        .ok_or("The bundled player template is missing its player element.")?;
    let end = TEMPLATE[start..]
        .find("</player>")
        .map(|offset| start + offset + "</player>".len())
        .ok_or("The bundled player template is missing its closing tag.")?;
    Ok(TEMPLATE[start..end].replace(PLACEHOLDER, command))
}

/// The rule that hands Kodi's video playback to us.
fn rules_element() -> &'static str {
    "  <rules action=\"prepend\">\n    <rule video=\"true\" player=\"TinePlayer\" />\n  </rules>"
}

/// Writes the whole template out, for a machine with no such file yet.
fn fresh(command: &str, as_default: bool) -> String {
    let mut xml = TEMPLATE.replace(PLACEHOLDER, command);
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
fn insert(existing: &str, command: &str, as_default: bool) -> Result<String, String> {
    let mut xml = remove_from(existing)?;

    let anchor = xml
        .find("</players>")
        .ok_or("Kodi's player file has no <players> section to add to.")?;
    // On its own line above the closing tag, rather than in front of it:
    // inserting at the tag itself takes the indentation that belongs to it,
    // and then the file no longer matches what it was once ours comes back
    // out again.
    let at = line_start(&xml, anchor);
    xml.insert_str(at, &format!("{}\n", player_element(command)?));

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
fn back_up(file: &Path) -> Result<Option<PathBuf>, String> {
    if !file.exists() {
        return Ok(None);
    }
    // Named for when it was taken, in a form somebody can read at a glance in
    // a file listing, the way the scripts this replaced did it.
    let stamp = glib::DateTime::now_local()
        .and_then(|now| now.format("%Y%m%d-%H%M%S"))
        .map(|stamp| stamp.to_string())
        .unwrap_or_else(|_| "backup".to_string());
    let backup = file.with_extension(format!("xml.{stamp}.bak"));
    std::fs::copy(file, &backup)
        .map_err(|e| format!("Couldn't back up {}: {e}", file.display()))?;
    Ok(Some(backup))
}

/// Sets Kodi up, or takes us back out of it. Returns what to tell the viewer.
pub fn apply(setup: &Setup, want: Registration) -> Result<String, String> {
    let existing = std::fs::read_to_string(&setup.file).ok();

    let xml = match (&existing, want) {
        (Some(existing), Registration::Absent) => remove_from(existing)?,
        (Some(existing), _) => insert(existing, &command()?, want == Registration::Default)?,
        (None, Registration::Absent) => return Ok("Kodi was not set up to begin with.".to_string()),
        (None, _) => fresh(&command()?, want == Registration::Default),
    };

    // Nothing to do, so nothing done: no write, and above all no backup.
    if existing.as_deref() == Some(xml.as_str()) {
        return Ok("Kodi is already set up that way.".to_string());
    }

    // Only when the file is not already one of ours. The point of a backup is
    // to keep whatever was there before TinePlayer touched it; once our
    // player is in the file, every later change is ours to undo with Remove,
    // and taking a copy each time would bury the one that matters under a
    // heap of near-identical ones.
    let backup = match setup.state {
        Registration::Absent => back_up(&setup.file)?,
        _ => None,
    };

    if let Some(folder) = setup.file.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|e| format!("Couldn't reach {}: {e}", folder.display()))?;
    }
    std::fs::write(&setup.file, xml)
        .map_err(|e| format!("Couldn't write {}: {e}", setup.file.display()))?;

    let done = match want {
        Registration::Absent => "TinePlayer removed from Kodi.",
        Registration::Offered => "Kodi will offer TinePlayer under \"Play using...\".",
        Registration::Default => "Kodi will play every video through TinePlayer.",
    };
    Ok(match backup {
        Some(backup) => format!(
            "{done}\nThe previous file was copied to {}.\nRestart Kodi for it to notice.",
            backup.display()
        ),
        None => format!("{done}\nRestart Kodi for it to notice."),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let out = insert(FOREIGN, "/opt/tineplayer", false).unwrap();
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
        let out = insert(FOREIGN, "/opt/tineplayer", true).unwrap();
        assert_eq!(read_state(&out), Registration::Default);
        assert!(out.contains("player=\"MPV\""));
    }

    #[test]
    fn removing_puts_the_file_back_as_it_was() {
        let added = insert(FOREIGN, "/opt/tineplayer", true).unwrap();
        let removed = remove_from(&added).unwrap();
        assert_eq!(read_state(&removed), Registration::Absent);
        assert!(!removed.contains("tineplayer"));
        assert_eq!(removed, FOREIGN);
    }

    #[test]
    fn switching_mode_does_not_duplicate_us() {
        let once = insert(FOREIGN, "/opt/tineplayer", false).unwrap();
        let twice = insert(&once, "/opt/tineplayer", true).unwrap();
        assert_eq!(twice.matches("name=\"TinePlayer\"").count(), 1);
        assert_eq!(twice.matches("player=\"TinePlayer\"").count(), 1);
    }

    #[test]
    fn a_fresh_file_is_valid_either_way() {
        assert_eq!(
            read_state(&fresh("/opt/tineplayer", false)),
            Registration::Offered
        );
        assert_eq!(
            read_state(&fresh("/opt/tineplayer", true)),
            Registration::Default
        );
        assert!(!fresh("/opt/tineplayer", false).contains("RULES START"));
    }
}
