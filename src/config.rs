use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::tr;

/// What the per-user folder is called.
///
/// Capitalized on Windows, where `AppData\Local` is a list of application
/// names written the way their applications write them, and a lowercase one
/// reads as something that escaped rather than something that was chosen.
/// Lowercase everywhere else, which is the convention under `~/.config` and
/// `~/.local/share` and what anyone typing the path would expect.
#[cfg(windows)]
pub const DIR_NAME: &str = "TinePlayer";
#[cfg(not(windows))]
pub const DIR_NAME: &str = "tineplayer";

/// What a folder beside the executable has to be called to make that copy
/// portable. Lowercase on a platform that capitalizes, because this one is a
/// folder somebody makes and looks inside rather than an application name
/// buried in `AppData`.
#[cfg(windows)]
pub const PORTABLE_DIR_NAME: &str = "user";

/// Where the per-user files go.
enum Storage {
    /// A `user` folder beside the executable. Everything lives in the one
    /// folder rather than being split into config and data the way the
    /// per-user directories are: the split exists because the operating
    /// system asks for it, and a folder someone carries on a stick is easier
    /// to understand whole.
    ///
    /// Windows only, and gated rather than merely unused elsewhere: the macOS
    /// bundle and the Linux package are both installed rather than unpacked,
    /// so `resolve_storage` there can never build one, and a variant nothing
    /// constructs is a warning that CI treats as an error.
    #[cfg(windows)]
    Portable(PathBuf),
    /// The operating system's per-user directories, which is every installed
    /// copy and every platform that has no portable form.
    PerUser,
}

/// Worked out once. The answer cannot change while the process runs, and the
/// writability check below creates a file, which is not something to repeat
/// on every path lookup.
fn storage() -> &'static (Storage, Option<String>) {
    static RESOLVED: std::sync::OnceLock<(Storage, Option<String>)> = std::sync::OnceLock::new();
    RESOLVED.get_or_init(resolve_storage)
}

/// A copy is portable when someone put a `user` folder next to it, and not
/// otherwise.
///
/// Deliberately not inferred from whether the executable's folder happens to
/// be writable. That would make where the settings live depend on where the
/// application was unpacked, which is invisible until they disappear - and it
/// would quietly turn an installation into a portable copy on any machine
/// where Program Files is loose. A folder is something somebody chose.
///
/// Windows only: the macOS bundle and the Linux package are both installed
/// rather than unpacked, so neither has a portable form to support.
#[cfg(windows)]
fn resolve_storage() -> (Storage, Option<String>) {
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.join(PORTABLE_DIR_NAME)))
        .filter(|dir| dir.is_dir())
    else {
        return (Storage::PerUser, None);
    };

    // Said out loud rather than silently falling back. Somebody who made this
    // folder meant their settings to be in it, and a copy that quietly wrote
    // to AppData instead would look like it was working right up until the
    // stick was moved to another machine.
    if let Err(e) = writable(&dir) {
        return (
            Storage::PerUser,
            Some(format!(
                "{} cannot be written to, so settings are being kept in your \
                 user profile instead of travelling with this copy.\n\n{e}",
                dir.display()
            )),
        );
    }

    (Storage::Portable(dir), None)
}

#[cfg(not(windows))]
fn resolve_storage() -> (Storage, Option<String>) {
    (Storage::PerUser, None)
}

/// Whether a file can actually be created in `dir`.
///
/// By writing one, because nothing cheaper is true on Windows: the read-only
/// attribute on a directory means something else entirely, and permissions are
/// an ACL evaluation that the metadata does not answer.
#[cfg(windows)]
fn writable(dir: &Path) -> Result<(), std::io::Error> {
    let probe = dir.join(".tineplayer-write-test");
    std::fs::write(&probe, b"")?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// What to tell someone about where their settings ended up, when it is not
/// where they asked. `None` in the ordinary case, which is both a portable
/// copy that works and every installed one.
pub fn storage_problem() -> Option<String> {
    storage().1.clone()
}

/// Somewhere writable to keep the fontconfig configuration and cache that
/// point at the fonts TinePlayer ships. Beside the settings, because the
/// installation itself may not be writable.
pub fn app_dir_for_fontconfig() -> Option<PathBuf> {
    let dir = app_dir(glib::user_config_dir()).join("fontconfig");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// The application's own folder under `base`, created if missing - or the
/// portable folder, when there is one, whatever `base` was going to be.
///
/// `base` is the per-user config or data directory. A portable copy collapses
/// the two, which is why the argument is ignored rather than joined onto.
fn app_dir(base: PathBuf) -> PathBuf {
    let dir = match &storage().0 {
        #[cfg(windows)]
        Storage::Portable(dir) => dir.clone(),
        Storage::PerUser => base.join(DIR_NAME),
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Somewhere to keep a cache that is not settings and not state - GStreamer's
/// plugin registry, which is rebuilt if it goes missing.
///
/// Beside everything else in a portable copy, so that copy leaves nothing on
/// the machine it ran on.
#[cfg(windows)]
pub fn cache_dir() -> Option<PathBuf> {
    let dir = match &storage().0 {
        Storage::Portable(dir) => dir.clone(),
        Storage::PerUser => PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(DIR_NAME),
    };
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Settings live in the per-user config directory rather than beside the
/// executable or in the working directory - unless a `user` folder beside the
/// executable says otherwise, which is what makes a copy portable.
///
/// Never the working directory: a relative path resolves against wherever the
/// process happened to be launched from, so running from a terminal and
/// double-clicking the executable would read and write different files. The
/// portable folder has no such problem, being derived from the executable's
/// own location rather than the caller's.
pub fn config_path() -> PathBuf {
    app_dir(glib::user_config_dir()).join("config.yaml")
}

fn default_sounds() -> bool {
    true
}

/// The default for a setting that is on unless somebody says otherwise.
fn yes() -> bool {
    true
}

fn default_check_for_updates() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub primary_sink: Option<String>,
    pub secondary_sink: Option<String>,
    /// Multiplies every font size and padding in the interface.
    ///
    /// Absent means "work it out from the display", which is what suits a
    /// television: a 4K screen the compositor is not already scaling would
    /// otherwise draw a ten-foot interface at desk-monitor size. Set it to
    /// pin the size regardless, and it is left out of the file when unset so
    /// that automatic sizing is not silently frozen the first time settings
    /// are saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_scale: Option<f64>,
    /// What language the interface is in, as a locale code: `de`, `pt-BR`.
    ///
    /// Absent means "whatever this machine is set to", which is right for
    /// almost everybody and is why it is left out of the file when unset - a
    /// person who moves a config between machines should not find one of them
    /// pinned to the other's language. Set it where the machine's language is
    /// not the one the viewer wants, which is common enough on a television
    /// that the row exists under Settings.
    ///
    /// `en` means English specifically, rather than "no preference": English
    /// is the language the source is written in, so it has no catalog and
    /// needs none. See `src/i18n.rs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Where the built-in browser last was, so it reopens there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_folder: Option<PathBuf>,
    /// Kodi userdata folders named by hand, kept only while TinePlayer is set
    /// up in them. A typed path cannot be rediscovered, so without this a Kodi
    /// configured from a chosen folder would vanish from the list and there
    /// would be no way to take TinePlayer back out of it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kodi_paths: Vec<PathBuf>,
    /// Font family and style for subtitles, without a size, e.g. "Sans Bold".
    ///
    /// Set because Pango's default resolves to a serif face, which is a poor
    /// choice over moving pictures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle_font: Option<String>,
    /// Subtitle size, in points against the video's own resolution rather
    /// than the screen's: subtitles are drawn into the frame and scaled up
    /// with it, so one number holds on any display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle_size: Option<u32>,
    /// Preferred language for each output, used to choose tracks for a file
    /// that has not been played before. Unset means "take the first track".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_language: Option<String>,
    /// Prefer a described track on this output, for a viewer who is blind or
    /// has low vision. Per output rather than one setting for both, because
    /// two people who need description may not share a language.
    /// Level for each output, 0.0 to 1.0, and whether it is silenced.
    ///
    /// Kept per output rather than per video: how loud the headphones are is a
    /// property of the headphones, not of the film.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_volume: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_volume: Option<f64>,
    /// One level over both outputs, which each output's own level is a
    /// fraction of. Kept because it is a setting somebody chose, unlike
    /// silencing everything for a knock at the door, which lasts the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_volume: Option<f64>,
    #[serde(default)]
    pub primary_muted: bool,
    #[serde(default)]
    pub secondary_muted: bool,
    /// How far to shift this output in time, in milliseconds, so it lines up
    /// with the picture and with the other output.
    ///
    /// Kept per output rather than per video for the same reason the level is:
    /// how late a set of headphones runs is a property of the headphones. A
    /// Bluetooth pair costs 100-200ms of encode, transmission and buffering,
    /// and no platform reports that to GStreamer - every sink reports its own
    /// buffer size instead, identically for a Bluetooth headset and an HDMI
    /// socket. So nothing can work it out on our behalf, and the only figure
    /// available is the one somebody sets by ear.
    ///
    /// Either direction. Positive holds this output back; negative pulls it
    /// forward, which is bounded by how much audio the pipeline has already
    /// buffered - measured working to at least 600ms on a Pi. Forward matters
    /// because the picture cannot be delayed: `gtk4paintablesink` has no
    /// offset, so audio that lags the video can only be fixed by hurrying the
    /// audio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_offset_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_offset_ms: Option<f64>,
    /// Whether that output's delay is applied. Off keeps the value and stops
    /// using it, which is what somebody wants when checking whether a delay
    /// is helping: the alternative is winding it to zero and having to find
    /// the setting again afterwards.
    ///
    /// Absent means off. Nobody starts out needing a delay, so the switch
    /// starts off and is turned on by whoever finds they need one. The cost
    /// is that a delay written into the file by hand does nothing until this
    /// is set alongside it, which the documentation says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_offset_on: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_offset_on: Option<bool>,
    #[serde(default)]
    pub primary_audio_description: bool,
    #[serde(default)]
    pub secondary_audio_description: bool,
    /// Unset means no subtitles unless chosen for the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle_language: Option<String>,
    /// How far in before stopping counts as a place to resume from, as a
    /// share of the running time. Unset means
    /// [`DEFAULT_RESUME_MIN_PERCENT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_min_percent: Option<f64>,
    /// The share past which a video counts as watched, so its position is
    /// dropped rather than saved. Unset means [`DEFAULT_WATCHED_PERCENT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watched_percent: Option<f64>,
    /// Reopened on the next run, so the menu comes back where you left it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_video: Option<PathBuf>,
    /// Remembered rather than reset each run: a machine wired to a television
    /// wants fullscreen every time, and saying so once should be enough.
    #[serde(default)]
    /// Whether to open fullscreen, as a deliberate choice rather than a
    /// record of how the window was last left.
    ///
    /// **Not written by the fullscreen button or F11.** It used to be, which
    /// made "start fullscreen" a side effect of whichever way round the window
    /// happened to be at the moment somebody quit - and left no way to say
    /// "open fullscreen" that a single stray keypress did not undo. Toggling
    /// during a session is about this session; this is about every one.
    pub fullscreen: bool,
    /// Whether to read the `.nfo` sidecar and the artwork files sitting beside
    /// a video.
    ///
    /// On by default, and off is a real answer: a library with no sidecars
    /// gains nothing from the looking, and somebody who would rather see file
    /// names than a scraper's idea of a title should be able to say so. Only
    /// what is read *beside* the file - the container's own tags come from the
    /// video itself and are read either way.
    #[serde(default = "yes")]
    pub read_metadata: bool,
    /// Whether the film's fanart is drawn behind the media page.
    ///
    /// Depends on [`Config::read_metadata`], the artwork being one of the
    /// things read beside the file: with that off there is nothing to draw and
    /// the row that sets this is disabled rather than lying about it.
    #[serde(default = "yes")]
    pub show_backdrop: bool,
    /// Whether where a video got to, and the tracks chosen for it, are written
    /// down at all.
    ///
    /// On by default, because coming back to a film where you left it is most
    /// of what the file is for. Off stops every write to `positions.json` and
    /// leaves whatever is already in it alone: a shared machine, or a viewer
    /// who would simply rather not have a list of what they have watched
    /// sitting in their data directory, is a real answer rather than a corner
    /// case. Clearing what is already saved is the row below, and stays
    /// available either way.
    ///
    /// The two thresholds under it decide *when* a position counts, which is
    /// nothing to decide when none are being kept, so they are disabled rather
    /// than left offering settings that do nothing.
    #[serde(default = "yes")]
    pub remember_positions: bool,
    /// The size the window was last left at, in pixels, so it opens where it
    /// was rather than at a default every time.
    ///
    /// Written only for a window that is neither maximized nor fullscreen:
    /// those are states rather than sizes, and recording the screen's
    /// dimensions as the window's own would leave nothing to restore to when
    /// they are turned off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_width: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_height: Option<i32>,
    /// Plays a short click when moving through the menus.
    #[serde(default = "default_sounds")]
    pub sounds: bool,
    /// Asks GitHub once a day whether a newer TinePlayer has been released.
    ///
    /// On by default, which is the useful answer for most people and the only
    /// way anyone finds out a release exists. It is the one thing TinePlayer
    /// does over the network without being asked, which is why it is a
    /// setting at all rather than simply how the application behaves. Nothing
    /// is ever downloaded or installed - see [`crate::updates`].
    #[serde(default = "default_check_for_updates")]
    pub check_for_updates: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xdg_runtime_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wayland_display: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            primary_sink: None,
            secondary_sink: None,
            ui_scale: None,
            language: None,
            last_folder: None,
            kodi_paths: Vec::new(),
            subtitle_font: None,
            subtitle_size: None,
            primary_language: None,
            secondary_language: None,
            primary_volume: None,
            secondary_volume: None,
            main_volume: None,
            primary_muted: false,
            secondary_muted: false,
            primary_offset_ms: None,
            secondary_offset_ms: None,
            primary_offset_on: None,
            secondary_offset_on: None,
            primary_audio_description: false,
            secondary_audio_description: false,
            subtitle_language: None,
            resume_min_percent: None,
            watched_percent: None,
            last_video: None,
            fullscreen: false,
            read_metadata: true,
            remember_positions: true,
            show_backdrop: true,
            window_width: None,
            window_height: None,
            sounds: default_sounds(),
            check_for_updates: default_check_for_updates(),
            xdg_runtime_dir: None,
            wayland_display: None,
            display: None,
        }
    }
}

impl Config {
    /// Reads the config, and says so when there was one it could not read.
    ///
    /// Never fails: an unreadable config leaves the application running on
    /// defaults rather than refusing to start, since the settings menu is the
    /// only place to put it right and refusing would put that out of reach.
    /// The returned message is for telling the user, and is `None` both when
    /// the file loaded and when there was no file at all - a first run is not
    /// a problem to report.
    pub fn load() -> (Config, Option<String>) {
        let (config, problem) = Self::read();
        set_remember_positions(config.remember_positions);
        // A copy that could not use its portable folder looks, from the
        // inside, exactly like one whose settings went missing. Said first
        // when both happened, because it explains the other.
        let problem = match (storage_problem(), problem) {
            (Some(storage), Some(rest)) => Some(format!("{storage}\n\n{rest}")),
            (Some(storage), None) => Some(storage),
            (None, rest) => rest,
        };
        // What the settings were when this run began, so the *first* change
        // made in a session is reported like every other one. Seeding on the
        // first save instead would swallow exactly the change most likely to
        // be the one somebody is asking about.
        if let Ok(text) = serde_yaml::to_string(&config) {
            seed_changes(&text);
        }
        (config, problem)
    }

    fn read() -> (Config, Option<String>) {
        let path = config_path();
        if !path.exists() {
            return (Config::default(), None);
        }

        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                return (
                    Config::default(),
                    Some(format!("Couldn't read {}.\n\n{e}", path.display())),
                );
            }
        };

        match serde_yaml::from_str(&text) {
            // An unset output device is not a broken file. It is what an
            // install that has not been through the menu yet looks like, and
            // the menu shows "Not set" for it. Treating it as a failure threw
            // away every other setting in the file along with it: languages,
            // scale, subtitle font, all of it.
            Ok(config) => (config, None),
            Err(e) => {
                // Saving anything would overwrite a file nobody has read yet,
                // and the typo in it is the one thing that would explain what
                // happened. Copied aside first, so it survives.
                let kept = Self::preserve_unreadable(&path);
                // On screen rather than only on stderr - it is the one
                // message somebody has to read to understand why their
                // settings went back to defaults - so it is translated,
                // unlike the diagnostics elsewhere in this file.
                let mut message = tr!(
                    "Couldn't read your settings from {path}.\n\n{reason}\n\nTinePlayer has started with default settings.",
                    path = path.display(),
                    reason = e,
                )
                .into_owned();
                match kept {
                    Ok(Some(backup)) => message.push_str(&tr!(
                        "\n\nThe file has been kept as {path}.",
                        path = backup.display()
                    )),
                    Ok(None) => {}
                    Err(e) => message
                        .push_str(&tr!("\n\nIt could not be backed up: {reason}", reason = e)),
                }
                (Config::default(), Some(message))
            }
        }
    }

    /// Clamped, because a share outside 0-100 has no meaning and a bad value
    /// in the file should not make videos unresumable.
    pub fn resume_min_percent(&self) -> f64 {
        self.resume_min_percent
            .unwrap_or(DEFAULT_RESUME_MIN_PERCENT)
            .clamp(0.0, 100.0)
    }

    /// Clamped, because a level outside 0 to 1 is either silence or
    /// distortion, and a bad number in the file should not produce either.
    pub fn volume(&self, role: &str) -> f64 {
        let stored = match role {
            "primary" => self.primary_volume,
            _ => self.secondary_volume,
        };
        stored.unwrap_or(1.0).clamp(0.0, 1.0)
    }

    /// The level over both outputs. Full unless somebody has moved it, so a
    /// configuration that predates it plays at exactly the level it always did.
    pub fn main_volume(&self) -> f64 {
        self.main_volume.unwrap_or(1.0).clamp(0.0, 1.0)
    }

    pub fn set_main_volume(&mut self, level: f64) {
        self.main_volume = Some((level.clamp(0.0, 1.0) * 100.0).round() / 100.0);
    }

    pub fn muted(&self, role: &str) -> bool {
        match role {
            "primary" => self.primary_muted,
            _ => self.secondary_muted,
        }
    }

    /// Rounded to a hundredth, which is finer than anyone can hear a
    /// difference at and keeps the file readable: floating point otherwise
    /// writes four fifths as 0.8499999999999999.
    pub fn set_volume(&mut self, role: &str, level: f64) {
        let level = (level.clamp(0.0, 1.0) * 100.0).round() / 100.0;
        match role {
            "primary" => self.primary_volume = Some(level),
            _ => self.secondary_volume = Some(level),
        }
    }

    pub fn set_muted(&mut self, role: &str, muted: bool) {
        match role {
            "primary" => self.primary_muted = muted,
            _ => self.secondary_muted = muted,
        }
    }

    /// Clamped, because a delay outside the offered range is either no delay
    /// at all or long enough to look like playback has stopped.
    pub fn offset_ms(&self, role: &str) -> f64 {
        let stored = match role {
            "primary" => self.primary_offset_ms,
            _ => self.secondary_offset_ms,
        };
        stored.unwrap_or(0.0).clamp(-MAX_OFFSET_MS, MAX_OFFSET_MS)
    }

    /// Rounded to the millisecond: finer than that is below what anyone can
    /// place by ear against a picture, and it keeps the file readable.
    pub fn set_offset_ms(&mut self, role: &str, ms: f64) {
        let ms = ms.clamp(-MAX_OFFSET_MS, MAX_OFFSET_MS).round();
        match role {
            "primary" => self.primary_offset_ms = Some(ms),
            _ => self.secondary_offset_ms = Some(ms),
        }
    }

    /// Whether that output's delay is being applied. Unset means off.
    pub fn offset_on(&self, role: &str) -> bool {
        match role {
            "primary" => self.primary_offset_on,
            _ => self.secondary_offset_on,
        }
        .unwrap_or(false)
    }

    pub fn set_offset_on(&mut self, role: &str, on: bool) {
        match role {
            "primary" => self.primary_offset_on = Some(on),
            _ => self.secondary_offset_on = Some(on),
        }
    }

    /// What the pipeline should actually use: the stored delay while it is
    /// on, and nothing while it is off. The stored value is left alone either
    /// way, so turning it back on restores what was set rather than zero.
    pub fn applied_offset_ms(&self, role: &str) -> f64 {
        if self.offset_on(role) {
            self.offset_ms(role)
        } else {
            0.0
        }
    }

    pub fn watched_percent(&self) -> f64 {
        self.watched_percent
            .unwrap_or(DEFAULT_WATCHED_PERCENT)
            .clamp(0.0, 100.0)
    }

    /// Keeps a copy of a config that could not be parsed, before anything
    /// saves over it. Returns where it went, or `None` if a copy was already
    /// kept from an earlier run: the first one is the interesting one, and
    /// overwriting it with a later copy of the same broken file gains
    /// nothing.
    fn preserve_unreadable(path: &Path) -> Result<Option<PathBuf>, String> {
        let backup = path.with_extension("yaml.invalid");
        if backup.exists() {
            return Ok(None);
        }
        std::fs::copy(path, &backup).map_err(|e| e.to_string())?;
        Ok(Some(backup))
    }

    pub fn save(&self) -> Result<(), String> {
        // Before the write rather than after, so a save that fails still
        // leaves the switch matching what the settings screen is showing.
        set_remember_positions(self.remember_positions);
        let path = config_path();
        let text = serde_yaml::to_string(self).map_err(|e| e.to_string())?;
        note_changes(&text);
        write_atomically(&path, &text, false)
    }

    /// Records which desktop session was active when the config was
    /// written, so a later launch from a context that doesn't inherit it
    /// (an SSH shell, or a process spawned by Kodi) can still find the
    /// display. No-op on platforms without that concept.
    pub fn capture_display_session(&mut self) {
        let display = crate::display::detect_display_env();
        if display.is_empty() {
            return;
        }
        self.xdg_runtime_dir = display.get("xdg_runtime_dir").cloned();
        self.wayland_display = display.get("wayland_display").cloned();
        self.display = display.get("display").cloned();
    }
}

/// Says which settings just changed, by comparing this save against the last.
///
/// Here rather than at the eight places that call `save`, and by comparison
/// rather than by each of them naming what it touched, for one reason: a
/// setting added next year is covered by this without anybody remembering to
/// cover it. The settings screen is exactly where "it stopped working after I
/// changed something" comes from, and the answer is usually a setting the
/// person no longer recalls touching.
///
/// The first save of a session is seeded rather than reported. Everything
/// would otherwise read as a change on the first write, which is noise: what
/// the settings were at startup is a different question, and `main` already
/// logs the two that matter.
///
/// Values are compared as YAML, so an unset field reads as `null` and a
/// changed one carries both sides. Nothing here can fail in a way worth
/// reporting - a config that will not serialise has already been caught by the
/// caller - so every error is a silent return.
static LAST_SAVED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Records the settings as they stood, without reporting anything.
fn seed_changes(text: &str) {
    if let Ok(mut last) = LAST_SAVED.lock() {
        *last = Some(text.to_string());
    }
}

fn note_changes(text: &str) {
    let Ok(mut last) = LAST_SAVED.lock() else {
        return;
    };
    let Some(before) = last.replace(text.to_string()) else {
        return;
    };
    let changed = changed_settings(&before, text);
    if !changed.is_empty() {
        // One entry, so a burst of changes from one screen reads as one event
        // rather than as five things that happened at the same instant.
        log::info!(
            "Settings changed:{}",
            changed
                .iter()
                .map(|line| format!("\n  {line}"))
                .collect::<String>()
        );
    }
}

/// Which settings differ between two serialisations, as `key: old -> new`.
///
/// Split from the reporting above so it can be tested: the caller holds a
/// process-wide snapshot, which a test cannot set up twice.
fn changed_settings(before: &str, after: &str) -> Vec<String> {
    if before == after {
        return Vec::new();
    }
    let parse = |text: &str| {
        serde_yaml::from_str::<std::collections::BTreeMap<String, serde_yaml::Value>>(text).ok()
    };
    let (Some(before), Some(after)) = (parse(before), parse(after)) else {
        return Vec::new();
    };

    let show = |value: Option<&serde_yaml::Value>| match value {
        // Absent and explicitly null are the same answer to a reader, and
        // several of these fields mean "work it out" when unset.
        None | Some(serde_yaml::Value::Null) => "unset".to_string(),
        // Quoted, so a device name with trailing space or punctuation in it is
        // visible rather than being read as part of the sentence.
        Some(serde_yaml::Value::String(text)) => format!("{text:?}"),
        Some(value) => serde_yaml::to_string(value)
            .map(|text| text.trim().to_string())
            .unwrap_or_else(|_| "?".to_string()),
    };

    // Absent and explicitly null are compared as one, not merely displayed
    // as one. Several fields here are left out of the file when unset, so a
    // version that started writing an explicit null would otherwise report
    // every one of them as `unset -> unset` on the first save.
    let value = |map: &std::collections::BTreeMap<String, serde_yaml::Value>, key: &str| {
        map.get(key).cloned().unwrap_or(serde_yaml::Value::Null)
    };

    after
        .keys()
        .chain(before.keys())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter(|key| value(&before, key) != value(&after, key))
        .map(|key| {
            format!(
                "{key}: {} -> {}",
                show(before.get(key)),
                show(after.get(key))
            )
        })
        .collect()
}

/// Writes a file so that a crash or a power cut leaves either the old contents
/// or the new ones, never half of either.
///
/// `std::fs::write` truncates and then writes, so there is a window - short,
/// but real - where the file on disk is empty or partial. Every state file
/// this application keeps went through it. **TinePlayer's stated home is a Pi
/// wired to a television, and the way televisions get turned off is at the
/// wall**, so that window is not theoretical here the way it might be on a
/// desktop. `Config::preserve_unreadable` exists because a config has already
/// been found unreadable at least once.
///
/// Temp file, flush, then rename over the target. The rename is what makes it
/// atomic: it is a single directory operation on every filesystem this runs
/// on, and Windows replaces an existing destination as POSIX does. `sync_all`
/// before it matters as much as the rename - without it the rename can reach
/// the disk before the bytes do, which turns a torn file into an empty one.
///
/// `private` sets `0o600` as the file is created, for the Jellyfin token. It
/// has to be the *temp* file that gets the mode, since that is the one that
/// becomes the real file - setting it afterwards would leave a moment where a
/// credential is world-readable. Windows has no equivalent and inherits the
/// profile folder's permissions, which already exclude other accounts.
///
/// The temp file sits beside the target rather than in a system temp folder,
/// because a rename across filesystems is not atomic and may not be a rename
/// at all. A leftover `.tmp` means this failed partway; it is overwritten on
/// the next attempt rather than being cleaned up separately, which would be
/// one more thing to fail.
pub fn write_atomically(path: &Path, text: &str, private: bool) -> Result<(), String> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let temp = path.with_file_name(format!("{name}.tmp"));

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;

    let failed =
        |what: &str, e: std::io::Error| format!("Failed to {what} {}: {e}", path.display());

    {
        use std::io::Write;
        let mut file = options
            .open(&temp)
            .map_err(|e| failed("open a temporary file beside", e))?;
        file.write_all(text.as_bytes())
            .map_err(|e| failed("write", e))?;
        // Before the rename, not after: see above.
        file.sync_all().map_err(|e| failed("flush", e))?;
    }

    std::fs::rename(&temp, path).map_err(|e| failed("replace", e))
}
/// Resume positions are state rather than settings, so they live in the
/// per-user data directory - but for the same reason as the config, they
/// must not be relative to the working directory.
pub fn positions_path() -> PathBuf {
    app_dir(glib::user_data_dir()).join("positions.json")
}

/// Where the diagnostic logs go: beside the positions and the pairing, in the
/// per-user data directory.
///
/// An `Option` where its neighbours return a bare path, because those are
/// paths a caller is about to read or write and can report a failure on. This
/// one is asked for by the logger, before there is anywhere to report a
/// failure to - so "there is no such folder" has to be an answer it can be
/// given rather than a file operation that fails later.
pub fn log_dir() -> Option<PathBuf> {
    let dir = app_dir(glib::user_data_dir());
    dir.is_dir().then_some(dir)
}

/// Which Jellyfin server this installation is paired with, and the token that
/// says so.
///
/// Its own file rather than a corner of `config.yaml`: see `jellyfin.rs` for
/// why a bearer credential is kept away from the file people are told to open
/// when something is wrong. State rather than settings, so it sits with the
/// positions rather than with the preferences.
pub fn jellyfin_path() -> PathBuf {
    app_dir(glib::user_data_dir()).join("jellyfin.json")
}

/// What the version check remembers between runs. State rather than settings,
/// for the same reason the resume positions are, so it sits beside them.
pub fn updates_path() -> PathBuf {
    app_dir(glib::user_data_dir()).join("updates.json")
}

/// How far in a video has to be before stopping counts as a place you left
/// off, as a share of its running time.
///
/// Every media server does this, because a minute into a three hour film is a
/// false start rather than progress. Jellyfin uses 5%, which is what this
/// matches; Kodi uses a flat 180 seconds, which is harsh on anything short.
pub const DEFAULT_RESUME_MIN_PERCENT: f64 = 5.0;

/// The furthest an output can be shifted, in milliseconds, in either
/// direction.
///
/// Bluetooth costs 100-200ms and the platform already absorbs some of it - on
/// Linux about 150ms, on macOS about 240ms, on Windows only 60ms - so the
/// correction anyone actually dials in is well under half a second. A second
/// is generous enough to cover a badly muxed file too, and short enough that
/// holding the key never strands somebody a long way from where they meant to
/// be.
///
/// The same figure serves both directions, though they are not symmetrical in
/// what they cost. Holding a sink back is free. Pulling one forward spends
/// buffered audio, and past what the pipeline holds it would arrive late and
/// be dropped - measured working to at least 600ms on a Pi, which is where
/// the limit was left rather than tuned to the edge.
pub const MAX_OFFSET_MS: f64 = 1000.0;

/// The floor under that share, for videos short enough that 5% is seconds.
/// Also the whole rule for an entry saved before durations were recorded.
const RESUME_MIN_NS: u64 = 10_000_000_000;

/// Past this share, a video counts as watched rather than part-way through,
/// and its position is dropped instead of saved. Jellyfin, Plex and Kodi all
/// use 90%, and so does what we report to Kodi - see `kodi::WATCHED_PERCENT`.
pub const DEFAULT_WATCHED_PERCENT: f64 = 90.0;

/// What is remembered about a video between runs: where you stopped, and
/// which track was going to which output.
///
/// Tracks live here rather than in the config because they are a property of
/// the file, not of the machine. Picking up a film you were halfway through
/// should restore the languages you had chosen for it, not the ones you last
/// used on something else.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Resume {
    #[serde(default)]
    pub position_ns: u64,
    /// Absent when no selection has ever been saved for this file.
    ///
    /// Nested rather than two bare fields so that "never chosen" and "chosen
    /// as no audio" stay distinguishable. Flattened, an entry carried over
    /// from an older version looked exactly like a deliberate choice of no
    /// tracks at all, and silently overwrote the real one.
    #[serde(default)]
    pub tracks: Option<TrackChoice>,
    /// The video's running time, so the thresholds above can be shares of it
    /// rather than flat seconds. Zero for an entry written before this was
    /// recorded, or a source that could not say how long it was.
    #[serde(default)]
    pub duration_ns: u64,
    /// What aligning this video against a separate audio file worked out, in
    /// milliseconds, keyed by that file's path.
    ///
    /// Kept per pairing rather than per video, because it describes the two
    /// files together: the same film aligns differently against a different
    /// description track. Measuring costs seconds of decoding, so it is
    /// measured once and remembered, and the viewer can ask for it again.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub alignments: std::collections::BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrackChoice {
    pub primary: Option<u32>,
    pub secondary: Option<u32>,
    /// Independent of the audio pair: subtitles may be a third language
    /// again, or the same as one of them.
    #[serde(default)]
    pub subtitle: Option<crate::subtitles::SubtitleChoice>,
    /// A separate audio file chosen for an output, which stands in place of
    /// any track inside the video.
    ///
    /// Stored beside the track rather than instead of it, so clearing the file
    /// falls back to whatever was chosen before it. Absent in every entry
    /// written before this existed, which `serde(default)` reads as none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_file: Option<std::path::PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_file: Option<std::path::PathBuf>,
}

impl Resume {
    /// Where playback should pick up, if anywhere.
    ///
    /// Only the near-the-start rule lives here. The other end is applied when
    /// saving: a position past [`WATCHED_PERCENT`] is never written, so an
    /// entry that exists is one worth offering.
    pub fn resume_position(&self, min_percent: f64) -> Option<u64> {
        let minimum = if self.duration_ns > 0 {
            let share = (self.duration_ns as f64 * min_percent / 100.0) as u64;
            share.max(RESUME_MIN_NS)
        } else {
            RESUME_MIN_NS
        };
        (self.position_ns >= minimum).then_some(self.position_ns)
    }
}

fn load_all() -> std::collections::HashMap<String, Resume> {
    let path = positions_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Default::default();
    };
    if let Ok(entries) = serde_json::from_str(&text) {
        return entries;
    }
    // Earlier versions stored a bare position per file. Read those rather
    // than discarding them, so upgrading doesn't lose everyone's place.
    serde_json::from_str::<std::collections::HashMap<String, u64>>(&text)
        .map(|old| {
            old.into_iter()
                .map(|(file, position_ns)| {
                    (
                        file,
                        Resume {
                            position_ns,
                            ..Default::default()
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether positions and track choices may be written down.
///
/// A process-wide switch rather than an argument threaded through every
/// writer: the functions below are free functions reached from all over the
/// player - the tick that saves a position, the chooser that remembers a
/// track, the aligner - and none of them otherwise needs to know what the
/// settings say. Kept in step by `Config::load` and `Config::save`, which are
/// the only two moments the answer can change.
///
/// Defaults to writing. A copy whose config could not be read falls back to
/// defaults, and losing somebody's resume points because their settings file
/// was unreadable would be a worse failure than the one it came from.
static REMEMBER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Says whether positions may be written from here on.
pub fn set_remember_positions(on: bool) {
    REMEMBER.store(on, std::sync::atomic::Ordering::Relaxed);
}

fn save_all(entries: &std::collections::HashMap<String, Resume>) {
    // The one place every write goes through, which is why the switch sits
    // here rather than at each caller. Reading is untouched: what was saved
    // before the setting was turned off is still there and still resumed
    // from, because turning it off says "stop keeping track", not "forget
    // what you knew".
    if !REMEMBER.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    if let Ok(text) = serde_json::to_string(entries) {
        let _ = write_atomically(&positions_path(), &text, false);
    }
}

pub fn load_resume(key: &str) -> Option<Resume> {
    load_all().get(key).cloned()
}

/// Applies `edit` to a file's entry, creating it if there isn't one.
fn update(key: &str, edit: impl FnOnce(&mut Resume)) {
    let mut entries = load_all();
    edit(entries.entry(key.to_string()).or_default());
    save_all(&entries);
}

/// Position stored in nanoseconds.
///
/// The key identifies the video: a local file's path, a remote source's URI,
/// or - better than either - an id from whatever launched us. See
/// `Source::key` and `kodi::Item::key`.
/// `duration_ns` may be zero when the source could not say how long it is,
/// which only costs the percentage rules; the floor still applies.
pub fn save_position(key: &str, position_ns: u64, duration_ns: u64, watched_percent: f64) {
    // Watched to the end in all but name. Kodi and the media servers stop
    // offering to resume here, and a position a few minutes from the credits
    // is worse than none: it sends you back into the ending you just watched.
    let finished =
        duration_ns > 0 && position_ns as f64 >= duration_ns as f64 * watched_percent / 100.0;
    update(key, |entry| {
        entry.position_ns = if finished { 0 } else { position_ns };
        entry.duration_ns = duration_ns;
    });
}

/// Called when a file plays to the end. Only the position is forgotten: the
/// track choices are still the right ones next time you watch it.
pub fn clear_position(key: &str) {
    update(key, |entry| entry.position_ns = 0);
}

/// Forgets one video outright: its position and its track choices, rather
/// than only the position. Reports whether there was anything to forget, so a
/// caller can tell "done" from "there was nothing there".
pub fn forget(key: &str) -> bool {
    let mut entries = load_all();
    let had = entries.remove(key).is_some();
    if had {
        save_all(&entries);
    }
    had
}

/// How many videos are remembered.
pub fn remembered() -> usize {
    load_all().len()
}

/// Forgets every remembered position and track choice.
pub fn clear_all_resume() -> Result<(), String> {
    let path = positions_path();
    if !path.exists() {
        return Ok(());
    }
    std::fs::remove_file(&path).map_err(|e| format!("Failed to clear {}: {e}", path.display()))
}

/// What was worked out about this video and that audio file, if anything.
pub fn load_alignment(key: &str, audio: &Path) -> Option<f64> {
    load_resume(key)?
        .alignments
        .get(&audio.to_string_lossy().to_string())
        .copied()
}

/// Remembers an alignment, or forgets it when asked to align again.
pub fn save_alignment(key: &str, audio: &Path, millis: Option<f64>) {
    let audio = audio.to_string_lossy().to_string();
    update(key, |entry| match millis {
        Some(millis) => {
            entry.alignments.insert(audio.clone(), millis);
        }
        None => {
            entry.alignments.remove(&audio);
        }
    });
}

pub fn save_tracks(
    key: &str,
    primary: Option<u32>,
    secondary: Option<u32>,
    subtitle: Option<crate::subtitles::SubtitleChoice>,
    primary_file: Option<std::path::PathBuf>,
    secondary_file: Option<std::path::PathBuf>,
) {
    update(key, |entry| {
        entry.tracks = Some(TrackChoice {
            primary,
            secondary,
            subtitle,
            primary_file,
            secondary_file,
        });
    });
}

#[cfg(test)]
mod offsets {
    use super::{Config, MAX_OFFSET_MS};

    /// Either role, since they are stored in separate fields and a match arm
    /// that writes the wrong one silently moves the other output.
    #[test]
    fn each_role_keeps_its_own_offset() {
        let mut config = Config::default();
        config.set_offset_ms("primary", 120.0);
        config.set_offset_ms("secondary", -80.0);
        assert_eq!(config.offset_ms("primary"), 120.0);
        assert_eq!(config.offset_ms("secondary"), -80.0);
    }

    /// An output nothing has shifted is not shifted, rather than carrying
    /// whatever a missing field decoded to.
    #[test]
    fn an_unset_offset_is_no_offset() {
        let config = Config::default();
        assert_eq!(config.offset_ms("primary"), 0.0);
        assert_eq!(config.offset_ms("secondary"), 0.0);
    }

    /// Both directions, and on the way in as well as the way out: the file is
    /// editable by hand, so a value past the limit can arrive without ever
    /// having been through a slider.
    #[test]
    fn an_offset_past_the_limit_is_brought_back_to_it() {
        let mut config = Config::default();
        config.set_offset_ms("primary", 5_000.0);
        assert_eq!(config.offset_ms("primary"), MAX_OFFSET_MS);
        config.set_offset_ms("primary", -5_000.0);
        assert_eq!(config.offset_ms("primary"), -MAX_OFFSET_MS);

        let hand_edited = Config {
            primary_offset_ms: Some(9_000.0),
            secondary_offset_ms: Some(-9_000.0),
            ..Default::default()
        };
        assert_eq!(hand_edited.offset_ms("primary"), MAX_OFFSET_MS);
        assert_eq!(hand_edited.offset_ms("secondary"), -MAX_OFFSET_MS);
    }

    /// Off until somebody turns it on, since nobody starts out needing a
    /// delay - including when a value is present without the switch, which is
    /// what a config edited by hand looks like.
    #[test]
    fn an_offset_does_nothing_until_it_is_turned_on() {
        let mut config = Config::default();
        assert!(!config.offset_on("primary"));

        config.set_offset_ms("primary", 120.0);
        assert!(!config.offset_on("primary"));
        assert_eq!(config.applied_offset_ms("primary"), 0.0);

        config.set_offset_on("primary", true);
        assert_eq!(config.applied_offset_ms("primary"), 120.0);
    }

    /// The whole point of the switch: turning it off stops the delay being
    /// used without losing the value, which somebody spent time finding by
    /// ear and would otherwise have to find again.
    #[test]
    fn an_offset_turned_off_is_kept_but_not_applied() {
        let mut config = Config::default();
        config.set_offset_ms("secondary", -150.0);
        config.set_offset_on("secondary", true);
        config.set_offset_on("secondary", false);

        assert_eq!(config.applied_offset_ms("secondary"), 0.0);
        assert_eq!(config.offset_ms("secondary"), -150.0);

        config.set_offset_on("secondary", true);
        assert_eq!(config.applied_offset_ms("secondary"), -150.0);
    }

    /// Each output's switch is its own, like its delay.
    #[test]
    fn turning_one_output_off_leaves_the_other_alone() {
        let mut config = Config::default();
        config.set_offset_ms("primary", 100.0);
        config.set_offset_ms("secondary", 200.0);
        config.set_offset_on("primary", true);
        config.set_offset_on("secondary", true);
        config.set_offset_on("primary", false);

        assert_eq!(config.applied_offset_ms("primary"), 0.0);
        assert_eq!(config.applied_offset_ms("secondary"), 200.0);
        assert!(!config.offset_on("primary"));
        assert!(config.offset_on("secondary"));
    }

    /// Stored to the millisecond, which is finer than anyone can place by ear
    /// and keeps the file readable.
    #[test]
    fn an_offset_is_stored_rounded() {
        let mut config = Config::default();
        config.set_offset_ms("primary", 12.6);
        assert_eq!(config.offset_ms("primary"), 13.0);
        config.set_offset_ms("secondary", -12.6);
        assert_eq!(config.offset_ms("secondary"), -13.0);
    }
}

#[cfg(test)]
mod change_tests {
    use super::changed_settings;

    #[test]
    fn an_unchanged_config_reports_nothing() {
        let text = "sounds: true\nprimary_sink: Speakers\n";
        assert!(changed_settings(text, text).is_empty());
    }

    #[test]
    fn a_changed_value_carries_both_sides() {
        assert_eq!(
            changed_settings("sounds: true\n", "sounds: false\n"),
            vec!["sounds: true -> false"]
        );
    }

    /// The commonest real change, and the one a device name has to survive
    /// intact: they carry spaces, parentheses and punctuation.
    #[test]
    fn a_device_name_is_quoted() {
        assert_eq!(
            changed_settings(
                "secondary_sink: null\n",
                "secondary_sink: Headphones (2- Arctis Nova Pro Wireless)\n"
            ),
            vec![r#"secondary_sink: unset -> "Headphones (2- Arctis Nova Pro Wireless)""#]
        );
    }

    /// An absent key and an explicit null mean the same thing to a reader, so
    /// moving between them is not a change worth reporting.
    #[test]
    fn absent_and_null_are_the_same_answer() {
        assert!(
            changed_settings("ui_scale: null\n", "sounds: true\n")
                .iter()
                .all(|line| !line.starts_with("ui_scale"))
        );
    }

    /// Several at once read as one event, in a stable order.
    #[test]
    fn every_change_is_listed_and_sorted() {
        assert_eq!(
            changed_settings(
                "sounds: true\nui_scale: 1.0\n",
                "sounds: false\nui_scale: 2.0\n"
            ),
            vec!["sounds: true -> false", "ui_scale: 1.0 -> 2.0"]
        );
    }
}

#[cfg(test)]
mod atomic_tests {
    use super::write_atomically;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("tp-atomic");
        std::fs::create_dir_all(&dir).expect("the temporary directory is writable");
        dir.join(name)
    }

    #[test]
    fn it_writes_the_content() {
        let path = scratch("plain.json");
        let _ = std::fs::remove_file(&path);
        write_atomically(&path, "{\"a\":1}", false).expect("it writes");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":1}");
    }

    #[test]
    fn it_replaces_what_was_there() {
        let path = scratch("replaced.json");
        std::fs::write(&path, "old and longer than the new one").unwrap();
        write_atomically(&path, "new", false).expect("it writes");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    }

    /// The temp file is an implementation detail and must not be left in the
    /// user's data folder beside their settings.
    #[test]
    fn it_leaves_no_temporary_file_behind() {
        let path = scratch("tidy.json");
        let _ = std::fs::remove_file(&path);
        write_atomically(&path, "x", false).expect("it writes");
        assert!(!path.with_file_name("tidy.json.tmp").exists());
    }

    /// The Jellyfin token's requirement, and the reason `private` exists: the
    /// file that appears must never have been readable by anyone else, so the
    /// mode belongs on the temp file rather than being applied afterwards.
    #[cfg(unix)]
    #[test]
    fn a_private_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let path = scratch("secret.json");
        let _ = std::fs::remove_file(&path);
        write_atomically(&path, "token", true).expect("it writes");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "mode was {:o}", mode & 0o777);
    }
}
