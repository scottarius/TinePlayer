use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Per-user application directory under `base`, created if missing.
fn app_dir(base: PathBuf) -> PathBuf {
    let dir = base.join("tineplayer");
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

/// Which file picker to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserMode {
    /// The built-in browser when the last thing touched was a controller,
    /// the system dialog otherwise. Each is better in its own situation:
    /// the dialog can be typed into, the browser can be reached from a sofa.
    #[default]
    Automatic,
    System,
    #[serde(rename = "builtin")]
    BuiltIn,
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
    #[serde(default)]
    pub file_browser: BrowserMode,
    /// Where the built-in browser last was, so it reopens there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_folder: Option<PathBuf>,
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
    /// Unset means no subtitles unless chosen for the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle_language: Option<String>,
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
            file_browser: BrowserMode::default(),
            last_folder: None,
            subtitle_font: None,
            subtitle_size: None,
            primary_language: None,
            secondary_language: None,
            subtitle_language: None,
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
    pub fn load() -> Result<Config, String> {
        let path = config_path();
        if !path.exists() {
            return Err(format!(
                "No config found at {}.\nRun with --configure to set up your audio output devices.",
                path.display()
            ));
        }

        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        let config: Config = serde_yaml::from_str(&text)
            .map_err(|e| format!("Failed to parse {}: {e}", path.display()))?;

        // Only primary_sink is required — secondary_sink is optional (a
        // single-output setup is valid), so it's not validated here.
        if config.primary_sink.as_deref().unwrap_or("").is_empty() {
            return Err(format!("Missing 'primary_sink' in {}.", path.display()));
        }

        Ok(config)
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
/// per-user data directory — but for the same reason as the config, they
/// must not be relative to the working directory.
pub fn positions_path() -> PathBuf {
    app_dir(glib::user_data_dir()).join("positions.json")
}

/// Below this, a stored position is treated as no position at all. Stopping
/// a few seconds in is a false start rather than a place you left off, and
/// offering to resume from it is noise.
const RESUME_THRESHOLD_NS: u64 = 10_000_000_000;

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
    pub fn resume_position(&self) -> Option<u64> {
        (self.position_ns >= RESUME_THRESHOLD_NS).then_some(self.position_ns)
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

pub fn load_resume(path: &Path) -> Option<Resume> {
    load_all().get(&path.to_string_lossy().to_string()).cloned()
}

/// Applies `edit` to a file's entry, creating it if there isn't one.
fn update(path: &Path, edit: impl FnOnce(&mut Resume)) {
    let mut entries = load_all();
    edit(
        entries
            .entry(path.to_string_lossy().to_string())
            .or_default(),
    );
    save_all(&entries);
}

/// Position stored in nanoseconds, keyed by absolute file path string.
pub fn save_position(path: &Path, position_ns: u64) {
    update(path, |entry| entry.position_ns = position_ns);
}

/// Called when a file plays to the end. Only the position is forgotten: the
/// track choices are still the right ones next time you watch it.
pub fn clear_position(path: &Path) {
    update(path, |entry| entry.position_ns = 0);
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
    path: &Path,
    primary: Option<u32>,
    secondary: Option<u32>,
    subtitle: Option<crate::subtitles::SubtitleChoice>,
) {
    update(path, |entry| {
        entry.tracks = Some(TrackChoice {
            primary,
            secondary,
            subtitle,
        });
    });
}
