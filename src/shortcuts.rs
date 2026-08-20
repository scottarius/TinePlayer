//! The list of keys and buttons, on screen.
//!
//! **Written here rather than gathered from the handlers**, which cannot be
//! asked what they are bound to: the keyboard is `match` arms on `gdk::Key`
//! spread through one large handler, and the pad is a table of its own. A list
//! that walked them would be a second parser of the same code and would still
//! miss the ones that depend on what is playing.
//!
//! The cost is that this file has to be kept true by hand. Anything added to
//! the key handler belongs here in the same change - which is the same rule
//! `docs/usage.md` already lives under, and this list is the one people
//! actually read, being the only one on the television.

use std::borrow::Cow;

use gtk::prelude::*;

use crate::{tr, trc};

/// One binding: what to press, on either kind of control, and what it does.
///
/// The pad column is often empty, and deliberately so. Saying "-" in it would
/// read as a binding rather than as an absence.
struct Binding {
    keys: Cow<'static, str>,
    pad: Cow<'static, str>,
    means: Cow<'static, str>,
}

/// A heading and the bindings under it.
struct Group {
    title: Cow<'static, str>,
    bindings: Vec<Binding>,
}

/// Shorthand, so the table below stays a table rather than becoming a wall of
/// `Binding { keys: ..., pad: ..., means: ... }`.
fn binding(keys: Cow<'static, str>, pad: Cow<'static, str>, means: Cow<'static, str>) -> Binding {
    Binding { keys, pad, means }
}

/// Two groups: what a film answers to while it is playing, and what works
/// wherever you are.
///
/// **A function rather than a `const`**, which every table of interface text
/// in this project has had to become: a translated string is looked up at run
/// time and cannot be `&'static str`. The shape is unchanged, and the `tr!`
/// calls sit where the literals were so the extractor still finds them.
///
/// The key and button columns are translated too, with their own contexts.
/// A German keyboard says Strg rather than Ctrl and Eingabe rather than Enter,
/// and this page is the reference people actually read - it is the only one on
/// a television. The entries that are literal key caps, `A, S` and `+, -`, are
/// passed straight through by a translator, which costs them a glance and is
/// better than the page being half in one language.
fn groups() -> Vec<Group> {
    vec![
        Group {
            title: tr!("Player Control"),
            bindings: vec![
                binding(
                    trc!("keyboard keys", "Space"),
                    trc!("gamepad buttons", "A, Start"),
                    tr!("Toggle play/pause"),
                ),
                binding(
                    trc!("keyboard keys", "Left, Right"),
                    trc!("gamepad buttons", "D-pad"),
                    tr!("Skip ten seconds, hold to scrub"),
                ),
                binding(
                    trc!("keyboard keys", "Up, Down"),
                    trc!("gamepad buttons", "D-pad"),
                    tr!("Open/Close player controls"),
                ),
                binding(
                    trc!("keyboard keys", "A, S"),
                    Cow::Borrowed(""),
                    tr!("Next soundtrack on the first or second output"),
                ),
                binding(
                    trc!("keyboard keys", "+, -"),
                    Cow::Borrowed(""),
                    tr!("Adjust main volume (all outputs)"),
                ),
                binding(
                    trc!("keyboard keys", "M"),
                    trc!("gamepad buttons", "Hold X"),
                    tr!("Toggle mute"),
                ),
                binding(
                    trc!("keyboard keys", "C"),
                    trc!("gamepad buttons", "X"),
                    tr!("Toggle subtitles"),
                ),
                binding(
                    trc!("keyboard keys", "Esc"),
                    trc!("gamepad buttons", "B"),
                    tr!("Back, or close menus/controls"),
                ),
            ],
        },
        Group {
            title: tr!("General"),
            bindings: vec![
                binding(
                    trc!("keyboard keys", "Arrows"),
                    trc!("gamepad buttons", "D-pad"),
                    tr!("Navigate menus and controls"),
                ),
                binding(
                    trc!("keyboard keys", "Tab"),
                    trc!("gamepad buttons", "Bumpers"),
                    tr!("Navigate between menu sections"),
                ),
                binding(
                    trc!("keyboard keys", "Enter"),
                    trc!("gamepad buttons", "A"),
                    tr!("Select/Activate focused element"),
                ),
                binding(
                    trc!("keyboard keys", "P"),
                    trc!("gamepad buttons", "Start"),
                    tr!("Play or resume the film on the media page"),
                ),
                binding(
                    trc!("keyboard keys", "F, F11"),
                    trc!("gamepad buttons", "Y"),
                    // Sentence case, matching the other seven rows here and
                    // the same string in controls.rs and app.rs. It was
                    // "Toggle Fullscreen" until the duplicate report caught
                    // it: one capital letter, one extra string for every
                    // translator, and two spellings on screen.
                    tr!("Toggle fullscreen"),
                ),
                binding(
                    trc!("keyboard keys", "Ctrl+O"),
                    Cow::Borrowed(""),
                    tr!("Browse for a video file to open"),
                ),
                binding(
                    trc!("keyboard keys", "Ctrl+L"),
                    Cow::Borrowed(""),
                    tr!("Open a video by URL"),
                ),
                binding(
                    trc!("keyboard keys", "F1"),
                    trc!("gamepad buttons", "Select"),
                    tr!("Display shortcuts"),
                ),
            ],
        },
    ]
}

/// The list itself, built at `scale`.
///
/// Rows of labels held in step by two size groups rather than one grid per
/// heading. A grid lines its own columns up and knows nothing of the grid
/// above it, so the two tables came out with keys of different widths and read
/// as two lists that happened to be near each other. The size groups span
/// every row on the page, so one column runs the whole way down.
pub fn page(scale: f64) -> gtk::Box {
    let px = |base: f64| (base * scale).round() as i32;
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(px(20.0))
        .build();

    // What makes the columns agree across both tables.
    let key_widths = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
    let pad_widths = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);

    for (index, group) in groups().iter().enumerate() {
        // The menus' own heading, rather than one that merely resembles it:
        // capitals, the same letter spacing at this scale, and the same class.
        // Two ways of drawing one thing is how they come to differ.
        let heading = crate::app::group_heading(&group.title.to_uppercase(), scale, index == 0);
        // Spelled back into words for a screen reader, which may otherwise
        // read capitals a letter at a time.
        heading.update_property(&[gtk::accessible::Property::Label(&crate::app::title_case(
            &group.title,
        ))]);
        page.append(&heading);

        let table = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        // Keys, then buttons, then what they do. The two ways of pressing the
        // same thing sit together so a reader looks down one pair of columns
        // rather than across the page, and both are centered in their own
        // column - the entries are single words of very different lengths, and
        // ragged against the left edge they read as a list of mistakes.
        for (row, binding) in group.bindings.iter().enumerate() {
            let line = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .css_classes(["tp-shortcut-row"])
                .build();
            // Every other row shaded, which is what lets an eye cross a wide
            // gap between a key and its description without losing the line.
            // Counted within the table rather than down the page, so both
            // tables start the same way and the pattern reads as deliberate.
            if row % 2 == 1 {
                line.add_css_class("tp-shortcut-stripe");
            }

            let keys = gtk::Label::builder()
                .label(binding.keys.as_ref())
                .halign(gtk::Align::Center)
                .css_classes(["tp-shortcut-keys"])
                .build();
            // Present even when there is no button for this, so the column
            // stays a column. Empty rather than a dash, which would read as a
            // binding of its own.
            let pad = gtk::Label::builder()
                .label(binding.pad.as_ref())
                .halign(gtk::Align::Center)
                .css_classes(["tp-shortcut-keys"])
                .build();
            let means = gtk::Label::builder()
                .label(binding.means.as_ref())
                .halign(gtk::Align::Start)
                .hexpand(true)
                .wrap(true)
                .css_classes(["tp-shortcut-means"])
                .build();
            key_widths.add_widget(&keys);
            pad_widths.add_widget(&pad);

            line.append(&keys);
            line.append(&pad);
            line.append(&means);
            table.append(&line);
        }
        page.append(&table);
    }
    page
}
