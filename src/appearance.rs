//! Adapting the interface to the machine it is running on: how large to draw
//! it, and whether to draw it dark.

use gtk::gdk;
use gtk::prelude::*;

use crate::config::Theme;

/// The display height the interface's sizes were chosen against. Anything
/// taller is scaled proportionally, so the menu occupies the same share of
/// the screen whatever it is plugged into.
///
/// Height rather than width, because width alone misjudges ultrawides badly:
/// a 3440x1440 monitor is not nearly twice the size of a 1080p one for
/// reading purposes, it is the same height with more room either side. Height
/// tracks how large text needs to be; width does not.
const REFERENCE_HEIGHT: f64 = 1080.0;
/// Never below the designed size: this is a ten-foot interface, and a small
/// screen is more likely to be close-range (where the config file can lower
/// it) than to want smaller text at a distance.
const MIN_SCALE: f64 = 1.0;
const MAX_SCALE: f64 = 3.0;
/// Quarter steps, so a 1440p display lands on a clean 1.25 rather than
/// 1.3333, which produces uneven rounding across the various sizes.
const STEP: f64 = 0.25;

/// Scale for a monitor, from its logical height.
///
/// Deliberately logical rather than physical pixels. GDK reports geometry in
/// application pixels, already divided by whatever scale factor the
/// compositor applies, so a 4K screen the compositor is already scaling 2x
/// reports 1080 and correctly gets 1.0 here, while the same screen unscaled
/// reports 2160 and gets 2.0. That cancellation is why the compositor's own
/// scaling never has to be consulted.
pub fn scale_for(monitor: &gdk::Monitor) -> f64 {
    let height = monitor.geometry().height() as f64;
    if height <= 0.0 {
        return MIN_SCALE;
    }
    let raw = height / REFERENCE_HEIGHT;
    ((raw / STEP).round() * STEP).clamp(MIN_SCALE, MAX_SCALE)
}

/// The monitor a window is actually on, once it has been realized.
pub fn monitor_for_window(window: &impl IsA<gtk::Window>) -> Option<gdk::Monitor> {
    let display = gdk::Display::default()?;
    let surface = window.as_ref().surface()?;
    display.monitor_at_surface(&surface)
}

/// Best guess before any window exists. The tallest monitor rather than the
/// first, because a television alongside a desk monitor is exactly the setup
/// this is for, and the television is the one being read from a distance.
pub fn tallest_monitor() -> Option<gdk::Monitor> {
    let display = gdk::Display::default()?;
    let monitors = display.monitors();
    let mut tallest: Option<gdk::Monitor> = None;
    for index in 0..monitors.n_items() {
        let Some(monitor) = monitors.item(index).and_downcast::<gdk::Monitor>() else {
            continue;
        };
        let taller = tallest
            .as_ref()
            .is_none_or(|current| monitor.geometry().height() > current.geometry().height());
        if taller {
            tallest = Some(monitor);
        }
    }
    tallest
}

/// Resolves the configured scale, detecting one if it was left unset.
pub fn resolve_scale(configured: Option<f64>, monitor: Option<&gdk::Monitor>) -> f64 {
    if let Some(scale) = configured {
        return scale;
    }
    monitor.map(scale_for).unwrap_or(MIN_SCALE)
}

/// Applies light or dark, resolving `Theme::Auto` against the desktop.
///
/// Auto means dark unless the desktop explicitly asks for light. Falling back
/// to dark rather than light is deliberate: this interface is usually on a
/// television in a darkened room, where a full-screen light menu between
/// films is genuinely unpleasant, and the desktops most likely to fail the
/// query are the minimal ones a media machine runs.
pub fn apply_theme(theme: Theme) -> bool {
    let dark = match theme {
        Theme::Light => false,
        Theme::Dark => true,
        Theme::Auto => !system_prefers_light(),
    };
    let Some(settings) = gtk::Settings::default() else {
        return dark;
    };
    settings.set_gtk_application_prefer_dark_theme(dark);

    // Measured on both platforms: clearing the preference returns Linux to
    // light immediately, and does nothing at all on Windows, where the theme
    // stays dark however it is prodded. Reassigning the theme name is enough
    // on Linux; Windows needs the process restarting, which the caller
    // offers. The intermediate name has to be one nothing could be called,
    // since GTK's own default theme is literally named "Default".
    let name = settings.gtk_theme_name();
    settings.set_gtk_theme_name(Some("tineplayer-force-reload"));
    settings.set_gtk_theme_name(name.as_deref());

    dark
}

/// Whether the desktop explicitly asks for a light interface. Anything else,
/// including being unable to ask at all, is not a request for light.
#[cfg(target_os = "windows")]
fn system_prefers_light() -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};

    // AppsUseLightTheme is the per-application setting, as distinct from
    // SystemUsesLightTheme, which is the taskbar and Start menu. Windows lets
    // them differ, and this one is what other applications follow.
    let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
        .encode_utf16()
        .collect();
    let name: Vec<u16> = "AppsUseLightTheme\0".encode_utf16().collect();

    let mut value: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    // A missing key means the question could not be answered, not that light
    // was asked for, so it does not count as a preference either way.
    status == 0 && value != 0
}

#[cfg(not(target_os = "windows"))]
fn system_prefers_light() -> bool {
    // 2 is the only value that actually requests light. 0 ("no preference")
    // and an unreachable portal both leave the decision to us.
    portal_color_scheme() == Some(2)
}

/// Reads `org.freedesktop.appearance color-scheme` from the desktop portal:
/// 0 no preference, 1 prefer dark, 2 prefer light.
///
/// Asked over D-Bus rather than through GSettings because the portal is the
/// cross-desktop answer, and works when GNOME's schemas are not installed at
/// all - as on the Raspberry Pi's labwc session.
#[cfg(not(target_os = "windows"))]
fn portal_color_scheme() -> Option<u32> {
    use gtk::gio;

    let proxy = gio::DBusProxy::for_bus_sync(
        gio::BusType::Session,
        gio::DBusProxyFlags::NONE,
        None,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Settings",
        gio::Cancellable::NONE,
    )
    .ok()?;

    let args = ("org.freedesktop.appearance", "color-scheme").to_variant();
    // ReadOne is the current method; Read is its deprecated predecessor, still
    // all that older portals offer (the Pi's included). They differ in how
    // deeply the reply is nested, so rather than encode that per method, the
    // wrappers are simply peeled off until a value appears.
    for method in ["ReadOne", "Read"] {
        let Ok(reply) = proxy.call_sync(
            method,
            Some(&args),
            gio::DBusCallFlags::NONE,
            2000,
            gio::Cancellable::NONE,
        ) else {
            continue;
        };

        // The type is checked before each unwrap rather than relying on
        // as_variant returning None: it asserts internally, so calling it on
        // the final non-variant value logs a GLib critical.
        let mut value = reply.child_value(0);
        while value.is_type(gtk::glib::VariantTy::VARIANT) {
            let Some(inner) = value.as_variant() else {
                break;
            };
            value = inner;
        }
        if let Some(scheme) = value.get::<u32>() {
            return Some(scheme);
        }
    }
    None
}
