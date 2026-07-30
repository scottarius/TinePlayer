//! Desktop-session display detection - Linux-only. On Windows this whole
//! concept doesn't apply: there's a single desktop and GTK finds it without
//! any env-var plumbing.
//!
//! This exists because development and Kodi hand-off both launch the player
//! from a non-graphical context (an SSH shell, a spawned process) that
//! doesn't inherit `WAYLAND_DISPLAY`/`DISPLAY`/`XDG_RUNTIME_DIR` from the
//! logged-in desktop session. GTK reads those from the environment to find
//! the compositor, so we detect and set them before GTK initializes.

use std::collections::HashMap;

use crate::config::Config;

#[cfg(target_os = "linux")]
pub fn detect_display_env() -> HashMap<String, String> {
    let mut result = HashMap::new();

    let uid = unsafe { libc::getuid() };
    let xdg_runtime_dir = format!("/run/user/{uid}");

    let mut wayland_sockets: Vec<String> = std::fs::read_dir(&xdg_runtime_dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|name| name.starts_with("wayland-") && !name.ends_with(".lock"))
                .collect()
        })
        .unwrap_or_default();
    wayland_sockets.sort();

    if let Some(socket) = wayland_sockets.into_iter().next() {
        result.insert("xdg_runtime_dir".to_string(), xdg_runtime_dir);
        result.insert("wayland_display".to_string(), socket);
        return result;
    }

    let mut x11_sockets: Vec<String> = std::fs::read_dir("/tmp/.X11-unix")
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|name| name.starts_with('X'))
                .collect()
        })
        .unwrap_or_default();
    x11_sockets.sort();

    if let Some(socket) = x11_sockets.into_iter().next() {
        let display_num = &socket[1..];
        result.insert("display".to_string(), format!(":{display_num}"));
    }

    result
}

#[cfg(not(target_os = "linux"))]
pub fn detect_display_env() -> HashMap<String, String> {
    HashMap::new()
}

/// Which display session to target: whatever's already in the live
/// environment (e.g. running directly at the console), then
/// the saved config, then live detection.
pub fn resolve_display(config: &Config) -> HashMap<String, String> {
    let mut result = HashMap::new();

    if let Ok(v) = std::env::var("WAYLAND_DISPLAY") {
        result.insert("wayland_display".to_string(), v);
        return result;
    }
    if let Ok(v) = std::env::var("DISPLAY") {
        result.insert("display".to_string(), v);
        return result;
    }

    if let Some(v) = &config.xdg_runtime_dir {
        result.insert("xdg_runtime_dir".to_string(), v.clone());
    }
    if let Some(v) = &config.wayland_display {
        result.insert("wayland_display".to_string(), v.clone());
    }
    if let Some(v) = &config.display {
        result.insert("display".to_string(), v.clone());
    }

    if result.is_empty() {
        return detect_display_env();
    }
    result
}

/// Set WAYLAND_DISPLAY/DISPLAY/XDG_RUNTIME_DIR in this process's own
/// environment before GTK initializes, so it can find the desktop session
/// when launched over SSH (which doesn't inherit them).
pub fn apply_display_env(display: &HashMap<String, String>) {
    // Nothing here reaches a session bus, and GTK otherwise logs a warning
    // about being unable to reach the accessibility bus every launch.
    if std::env::var("GTK_A11Y").is_err() {
        unsafe { std::env::set_var("GTK_A11Y", "none") };
    }

    if let Some(v) = display.get("xdg_runtime_dir")
        && std::env::var("XDG_RUNTIME_DIR").is_err()
    {
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", v) };
    }
    if let Some(v) = display.get("wayland_display") {
        if std::env::var("WAYLAND_DISPLAY").is_err() {
            unsafe { std::env::set_var("WAYLAND_DISPLAY", v) };
        }
    } else if let Some(v) = display.get("display")
        && std::env::var("DISPLAY").is_err()
    {
        unsafe { std::env::set_var("DISPLAY", v) };
    }
}
