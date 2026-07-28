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

/// Multiplies every font size and padding in the interface. The default
/// suits reading from across a room; lower it for close-range use on a
/// desktop monitor.
fn default_ui_scale() -> f64 {
    1.0
}

fn default_sounds() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub primary_sink: Option<String>,
    pub secondary_sink: Option<String>,
    #[serde(default = "default_ui_scale")]
    pub ui_scale: f64,
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
            ui_scale: default_ui_scale(),
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

pub fn load_positions() -> std::collections::HashMap<String, u64> {
    let path = positions_path();
    if !path.exists() {
        return Default::default();
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Default::default(),
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn save_positions(positions: &std::collections::HashMap<String, u64>) {
    let path = positions_path();
    if let Ok(text) = serde_json::to_string(positions) {
        let _ = std::fs::write(&path, text);
    }
}

/// Position stored in nanoseconds, keyed by absolute file path string.
pub fn save_position(path: &Path, position_ns: u64) {
    let mut positions = load_positions();
    positions.insert(path.to_string_lossy().to_string(), position_ns);
    save_positions(&positions);
}

pub fn clear_position(path: &Path) {
    let mut positions = load_positions();
    if positions
        .remove(&path.to_string_lossy().to_string())
        .is_some()
    {
        save_positions(&positions);
    }
}
