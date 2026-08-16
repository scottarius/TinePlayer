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

use gtk::prelude::*;

/// One binding: what to press, on either kind of control, and what it does.
///
/// The pad column is often empty, and deliberately so. Saying "-" in it would
/// read as a binding rather than as an absence.
struct Binding {
    keys: &'static str,
    pad: &'static str,
    means: &'static str,
}

/// A heading and the bindings under it.
struct Group {
    title: &'static str,
    bindings: &'static [Binding],
}

/// Two groups: what a film answers to while it is playing, and what works
/// wherever you are.
const GROUPS: &[Group] = &[
    Group {
        title: "Player Control",
        bindings: &[
            Binding {
                keys: "Space",
                pad: "A, Start",
                means: "Toggle play/pause",
            },
            Binding {
                keys: "Left, Right",
                pad: "D-pad",
                means: "Skip ten seconds, hold to scrub",
            },
            Binding {
                keys: "Up, Down",
                pad: "D-pad",
                means: "Open/Close player controls",
            },
            Binding {
                keys: "A, S",
                pad: "",
                means: "Next soundtrack on the first or second output",
            },
            Binding {
                keys: "+, -",
                pad: "",
                means: "Adjust main volume (all outputs)",
            },
            Binding {
                keys: "M",
                pad: "Hold X",
                means: "Toggle mute",
            },
            Binding {
                keys: "C",
                pad: "X",
                means: "Toggle subtitles",
            },
            Binding {
                keys: "Esc",
                pad: "B",
                means: "Stop, or close menus/controls",
            },
        ],
    },
    Group {
        title: "General",
        bindings: &[
            Binding {
                keys: "Arrows",
                pad: "D-pad",
                means: "Navigate menus and controls",
            },
            Binding {
                keys: "Tab",
                pad: "Bumpers",
                means: "Navigate between menu sections",
            },
            Binding {
                keys: "Enter",
                pad: "A",
                means: "Select/Activate focused element",
            },
            Binding {
                keys: "F, F11",
                pad: "Y",
                means: "Toggle Fullscreen",
            },
            Binding {
                keys: "Ctrl+O",
                pad: "",
                means: "Browse for a video file to open",
            },
            Binding {
                keys: "Ctrl+L",
                pad: "",
                means: "Open a video by URL",
            },
            Binding {
                keys: "F1",
                pad: "Select",
                means: "Display shortcuts",
            },
        ],
    },
];

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

    for (index, group) in GROUPS.iter().enumerate() {
        // The menus' own heading, rather than one that merely resembles it:
        // capitals, the same letter spacing at this scale, and the same class.
        // Two ways of drawing one thing is how they come to differ.
        let heading = crate::app::group_heading(&group.title.to_uppercase(), scale, index == 0);
        // Spelled back into words for a screen reader, which may otherwise
        // read capitals a letter at a time.
        heading.update_property(&[gtk::accessible::Property::Label(&crate::app::title_case(
            group.title,
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
                .label(binding.keys)
                .halign(gtk::Align::Center)
                .css_classes(["tp-shortcut-keys"])
                .build();
            // Present even when there is no button for this, so the column
            // stays a column. Empty rather than a dash, which would read as a
            // binding of its own.
            let pad = gtk::Label::builder()
                .label(binding.pad)
                .halign(gtk::Align::Center)
                .css_classes(["tp-shortcut-keys"])
                .build();
            let means = gtk::Label::builder()
                .label(binding.means)
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
