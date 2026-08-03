use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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

/// Somewhere writable to keep the fontconfig configuration and cache that
/// point at the fonts TinePlayer ships. Beside the settings, because the
/// installation itself may not be writable.
pub fn app_dir_for_fontconfig() -> Option<PathBuf> {
    let dir = app_dir(glib::user_config_dir()).join("fontconfig");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Per-user application directory under `base`, created if missing.
fn app_dir(base: PathBuf) -> PathBuf {
    let dir = base.join(DIR_NAME);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Settings live in the per-user config directory rather than beside the
/// executable or in the working directory. A relative path resolves against
/// wherever the process happened to be launched from, so running from a
/// terminal and double-clicking the executable would read and write
/// different files.
pub fn config_path() -> PathBuf {
    app_dir(glib::user_config_dir()).join("config.yaml")
}

fn default_sounds() -> bool {
    true
}

/// Which way round to draw the interface. `Auto` follows the desktop, and
/// falls back to dark when it cannot be asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Auto,
    Light,
    Dark,
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
    #[serde(default)]
    pub theme: Theme,
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
    #[serde(default)]
    pub primary_muted: bool,
    #[serde(default)]
    pub secondary_muted: bool,
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
    pub fullscreen: bool,
    /// Plays a short click when moving through the menus.
    #[serde(default = "default_sounds")]
    pub sounds: bool,
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
            theme: Theme::default(),
            last_folder: None,
            kodi_paths: Vec::new(),
            subtitle_font: None,
            subtitle_size: None,
            primary_language: None,
            secondary_language: None,
            primary_volume: None,
            secondary_volume: None,
            primary_muted: false,
            secondary_muted: false,
            primary_audio_description: false,
            secondary_audio_description: false,
            subtitle_language: None,
            resume_min_percent: None,
            watched_percent: None,
            last_video: None,
            fullscreen: false,
            sounds: default_sounds(),
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
                let mut message = format!(
                    "Couldn't read your settings from {}.\n\n{e}\n\nTinePlayer has started with default settings.",
                    path.display()
                );
                match kept {
                    Ok(Some(backup)) => message.push_str(&format!(
                        "\n\nThe file has been kept as {}.",
                        backup.display()
                    )),
                    Ok(None) => {}
                    Err(e) => message.push_str(&format!("\n\nIt could not be backed up: {e}")),
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
        let path = config_path();
        let text = serde_yaml::to_string(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, text)
            .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
        Ok(())
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

/// Resume positions are state rather than settings, so they live in the
/// per-user data directory - but for the same reason as the config, they
/// must not be relative to the working directory.
pub fn positions_path() -> PathBuf {
    app_dir(glib::user_data_dir()).join("positions.json")
}

/// How far in a video has to be before stopping counts as a place you left
/// off, as a share of its running time.
///
/// Every media server does this, because a minute into a three hour film is a
/// false start rather than progress. Jellyfin uses 5%, which is what this
/// matches; Kodi uses a flat 180 seconds, which is harsh on anything short.
pub const DEFAULT_RESUME_MIN_PERCENT: f64 = 5.0;

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
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrackChoice {
    pub primary: Option<u32>,
    pub secondary: Option<u32>,
    /// Independent of the audio pair: subtitles may be a third language
    /// again, or the same as one of them.
    #[serde(default)]
    pub subtitle: Option<crate::subtitles::SubtitleChoice>,
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

fn save_all(entries: &std::collections::HashMap<String, Resume>) {
    if let Ok(text) = serde_json::to_string(entries) {
        let _ = std::fs::write(positions_path(), text);
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

pub fn save_tracks(
    key: &str,
    primary: Option<u32>,
    secondary: Option<u32>,
    subtitle: Option<crate::subtitles::SubtitleChoice>,
) {
    update(key, |entry| {
        entry.tracks = Some(TrackChoice {
            primary,
            secondary,
            subtitle,
        });
    });
}
