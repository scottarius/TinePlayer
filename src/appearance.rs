//! Adapting the interface to the machine it is running on: how large to draw
//! it, and keeping it dark.

use gtk::gdk;
use gtk::prelude::*;

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

/// What a size chosen by hand is held to: the ends of the slider that sets
/// it, which are three steps either side of the normal size.
///
/// Held rather than trusted because the file can say anything, and a size of
/// 0.05 draws an interface too small to find the setting that would put it
/// back - the only way out being the file it came from.
pub const MIN_CHOSEN_SCALE: f64 = 0.33;
pub const MAX_CHOSEN_SCALE: f64 = 3.0;
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
    scale_for_height(monitor.geometry().height() as f64)
}

/// Scale for any height in the same logical pixels, which is what lets a
/// window be measured by the rule a screen is.
///
/// `MIN_SCALE` is the whole of "never smaller than designed": a window shorter
/// than `REFERENCE_HEIGHT` produces a ratio below one and is held at one, so
/// making a window small shrinks how much fits rather than how large it is
/// drawn. There is no separate floor to apply.
pub fn scale_for_height(height: f64) -> f64 {
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
        return scale.clamp(MIN_CHOSEN_SCALE, MAX_CHOSEN_SCALE);
    }
    monitor.map(scale_for).unwrap_or(MIN_SCALE)
}

/// Draws the interface dark, and keeps it there.
///
/// There is no light theme and nothing to resolve against the desktop. That is
/// a deliberate narrowing rather than tidying for its own sake: two themes
/// meant every colour, every icon and every focus state existed twice and had
/// to be judged twice, and the light one was never the case this application
/// is for - a full-screen light menu between films, on a television in a
/// darkened room, is genuinely unpleasant.
///
/// The desktop is no longer asked what it prefers, and the Windows registry
/// and D-Bus portal probes that asked are gone with it.
///
/// If a high-contrast mode is ever wanted it belongs here, as a *third*
/// deliberate scheme rather than as the light theme brought back: what that
/// needs is maximum separation between foreground and background, which is
/// not what a light theme is.
pub fn force_dark() {
    let Some(settings) = gtk::Settings::default() else {
        return;
    };
    settings.set_gtk_application_prefer_dark_theme(true);

    // Reassigning the theme name is what makes GTK act on the preference
    // rather than only recording it. The intermediate name has to be one
    // nothing could be called, since GTK's own default theme is literally
    // named "Default".
    let name = settings.gtk_theme_name();
    settings.set_gtk_theme_name(Some("tineplayer-force-reload"));
    settings.set_gtk_theme_name(name.as_deref());
}

// ---------------------------------------------------------------------------
// Reading direction
// ---------------------------------------------------------------------------

/// Whether the interface is being laid out right to left.
///
/// **Asked of GTK rather than of the language**, because GTK is what actually
/// draws and the two can disagree. `main` sets the default direction when the
/// resolved language is right-to-left, but it never sets it back - so somebody
/// whose machine is Arabic gets a right-to-left layout from GTK's own reading
/// of the locale even though TinePlayer has no Arabic catalog and its words
/// are still in English. That is the correct behavior, and only asking GTK
/// sees it.
///
/// The initialization check is not defensive padding: `default_direction`
/// asserts if GTK has not started, which takes the process down. Nothing here
/// is called before the first window in practice, but a stylesheet built one
/// line too early would be a crash rather than a wrong margin, and the tests
/// below have no GTK at all.
pub fn rtl() -> bool {
    gtk::is_initialized() && gtk::Widget::default_direction() == gtk::TextDirection::Rtl
}

/// Where text sits inside its own box when it should hug the start of the
/// line: the left edge in English, the right edge in Arabic or Hebrew.
///
/// **`GtkLabel` mirrors `xalign` itself**, so this is 0.0 in both directions
/// and the name is the whole of what it adds. The label computes its text
/// position as `xalign` of the spare width in a left-to-right widget and
/// `1.0 - xalign` in a right-to-left one, which means 0.0 already reads as
/// "the edge the line starts at".
///
/// **This was got wrong first**, and the way it failed is worth keeping. The
/// pass that made the interface right-to-left assumed `xalign` was absolute
/// and flipped it here, so every bare label was flipped twice and came out on
/// the left in a mirrored interface. It looked exactly like the direction work
/// having no effect, which sent the search everywhere except at the one line
/// that had caused it. What settled it was measuring: a heading 1355px wide
/// holding 211px of text, with `xalign` reading 1.0, drawn hard against the
/// left - a combination GTK cannot produce unless it is doing the flip.
///
/// Rows built as a box with a name and a value never showed the fault at all:
/// a `GtkBox` mirrors its children, so those were right whatever `xalign`
/// said. Only labels standing alone depend on this.
pub fn text_start() -> f32 {
    0.0
}

/// The opposite edge: where a value sits when its name is at the start of the
/// row. Mirrored by the label, exactly as [`text_start`] is.
pub fn text_end() -> f32 {
    1.0
}

/// Which way the lines of a label are justified, in reading order.
///
/// **The second alignment property GTK does not mirror**, and the one that
/// catches you after `xalign` is dealt with. `xalign` places the text block
/// inside the label's allocation; `justify` places each *line* inside that
/// block, defaults to `Left`, and stays `Left` in a right-to-left interface.
/// A single-line label never shows the difference, which is why this is easy
/// to miss - it appears only where a label wraps, and then only as text
/// hugging the wrong edge inside a box that mirrored correctly.
pub fn text_justify() -> gtk::Justification {
    match rtl() {
        true => gtk::Justification::Right,
        false => gtk::Justification::Left,
    }
}

/// The CSS side that begins a line, for the few rules that indent from it.
///
/// GTK's CSS has no logical properties - there is no `margin-inline-start` -
/// so a stylesheet that wants to indent from the start of the line has to be
/// told which physical side that is.
pub fn css_start() -> &'static str {
    match rtl() {
        true => "right",
        false => "left",
    }
}

/// The side that ends a line. See [`css_start`].
pub fn css_end() -> &'static str {
    match rtl() {
        true => "left",
        false => "right",
    }
}

#[cfg(test)]
mod reading_direction {
    use super::*;

    /// The two edges are opposites in both directions, and a value has to sit
    /// against the one the name does not.
    ///
    /// GTK's default direction cannot be set in a test - it is global to the
    /// process and the whole test binary shares one - so what is checked is
    /// the relationship rather than either answer. A pair that agreed would
    /// put a row's name and its value on the same edge, on top of each other.
    #[test]
    fn the_two_edges_are_opposite() {
        assert_ne!(text_start(), text_end());
        assert_ne!(css_start(), css_end());
    }

    /// Both CSS sides have to be sides GTK will parse. A typo here is a
    /// declaration discarded whole and silently, which is the trap the
    /// stylesheet has fallen into before.
    #[test]
    fn the_css_sides_are_real_properties() {
        for side in [css_start(), css_end()] {
            assert!(matches!(side, "left" | "right"), "{side}");
        }
    }
}

#[cfg(test)]
mod chosen_sizes {
    use super::*;

    /// The file can hold anything, including a size that would draw an
    /// interface too small to find the setting that would put it back.
    #[test]
    fn a_size_set_by_hand_is_held_to_what_the_slider_offers() {
        assert_eq!(resolve_scale(Some(0.05), None), MIN_CHOSEN_SCALE);
        assert_eq!(resolve_scale(Some(12.0), None), MAX_CHOSEN_SCALE);
    }

    /// Anything the slider itself can produce passes through untouched.
    #[test]
    fn a_size_within_the_range_is_left_alone() {
        for scale in [MIN_CHOSEN_SCALE, 0.69, 1.0, 1.44, 2.08, MAX_CHOSEN_SCALE] {
            assert_eq!(resolve_scale(Some(scale), None), scale);
        }
    }

    /// With nothing chosen and no monitor to measure, the designed size.
    #[test]
    fn nothing_chosen_and_nothing_to_measure_is_the_designed_size() {
        assert_eq!(resolve_scale(None, None), MIN_SCALE);
    }
}
