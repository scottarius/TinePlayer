//! Widgets, layout figures, and the small helpers the interface is built
//! from.
//!
//! Everything here is free-standing: it takes what it needs as arguments and
//! knows nothing about [`App`](super::App) or the state it holds. That is the
//! line the split was drawn along - these are the pieces, and the module above
//! is what assembles them.

use std::cell::Cell;

use gtk::prelude::*;
use gtk::{gdk, glib};

use super::READING_CHARS;
use crate::appearance;
use crate::probe::AudioTrack;
use crate::tr;

/// A screen laid out as fixed header, scrolling list, and whatever the
/// caller pins below. The list scrolls rather than the page as a whole, so
/// a long list can never push the header or a footer button off-screen.
/// Always builds the back button, even on screens that have nowhere to go
/// back to, where it's made invisible instead of omitted. Leaving it out
/// changes the header's height, which shifted the heading and the whole
/// list every time the user moved between the menu and a chooser.
pub(crate) fn list_page(
    title: &str,
    show_back: bool,
    scale: f64,
) -> (gtk::Box, gtk::ListBox, gtk::Button, gtk::Box) {
    let heading = heading_label(title);
    heading.set_xalign(appearance::text_start());
    heading.set_justify(appearance::text_justify());
    let page = list_page_with(&heading, show_back, scale);
    // The list carries the page's title, so arriving on one says where you
    // are before it says what row you are on. A reader gives the container's
    // name, then the position, then the row - which is the whole context in
    // one breath, and none of it read out unasked.
    name_it(&page.1, title);
    page
}

/// The same page with a heading of the caller's choosing, for the browser's
/// path trail.
pub(crate) fn list_page_with(
    heading: &impl IsA<gtk::Widget>,
    show_back: bool,
    scale: f64,
) -> (gtk::Box, gtk::ListBox, gtk::Button, gtk::Box) {
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(24)
        .margin_top(40)
        .margin_bottom(40)
        .margin_start(56)
        .margin_end(56)
        .build();

    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(16)
        .css_classes(["tp-header"])
        .build();
    // Its own box rather than sizing the widgets themselves: a button adds
    // padding and borders to whatever minimum it is given, so the arrow and
    // the mark never agree on a size. An empty box takes exactly the size the
    // stylesheet asks for, and the child sits centered inside it.
    let slot = gtk::Box::builder()
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .css_classes(["tp-leading"])
        .build();

    let back = back_button(scale);
    if !show_back {
        // Kept in the layout so it still occupies its space, but invisible
        // and skipped by focus.
        back.set_opacity(0.0);
        back.set_sensitive(false);
        back.set_can_focus(false);
    }
    slot.append(&back);
    header.append(&slot);

    header.append(heading);
    page.append(&header);

    let (scroller, list) = scrolling_list();
    page.append(&scroller);

    (page, list, back, slot)
}

/// The scrolling list every screen built around one shares, wired the way
/// navigation here expects to find it.
pub(crate) fn scrolling_list() -> (gtk::ScrolledWindow, gtk::ListBox) {
    let list = gtk::ListBox::new();
    list.add_css_class("tp-menu");
    // Browse keeps exactly one row selected as focus moves, which is what
    // the boundary checks in wire_navigation rely on.
    list.set_selection_mode(gtk::SelectionMode::Browse);
    list.set_activate_on_single_click(true);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();
    // Tab has to land on the list itself. The rows cannot take focus, and a
    // ScrolledWindow will take it to scroll with the arrow keys, so without
    // this the stop is the scroller and every key goes to it instead.
    scroller.set_focusable(false);
    list.set_focusable(true);
    (scroller, list)
}

/// The mark on its own, for a screen that says the name some other way.
pub(crate) const APP_MARK: &[u8] = include_bytes!("../../data/branding/tineplayer.png");

/// The mark with the name beside it, for a header.
pub(crate) const HORIZONTAL_LOCKUP: &[u8] =
    include_bytes!("../../data/branding/lockup-horizontal.png");

/// The full logo at `width`, in [`crate::lockup`], which explains why it is
/// not a `GtkImage` like every other picture here.
pub(crate) fn lockup_image(bytes: &'static [u8], width: f64) -> crate::lockup::Lockup {
    crate::lockup::Lockup::new(bytes, width)
}

/// How much room the film's description may take, in interface units.
///
/// Interface units rather than pixels because that is the question actually
/// being asked. Everything on the page scales together, so what decides
/// whether the plot fits is not how many pixels tall the window is but how
/// many rows-worth of interface fit in it - and at 3x on a 1440px screen that
/// is a third of what it is at 1x on the same screen.
///
/// The reservation is what the page cannot do without: the choosers, the
/// footer that plays the film, and the margins around them. Whatever is left
/// over is what the description gets, and at 3x on a modest display that is
/// nothing - which is the right answer. A page that shows a plot summary and
/// no way to press play has its priorities backwards.
///
/// This is the plan's open question about `ui_scale` answered: no-scroll and
/// 3x cannot both hold, and what yields is the artwork and the prose.
/// A button face: a drawn mark, and the words beside it.
///
/// A box rather than a label with a mark in the text, and both halves are
/// centered on their own terms. The marks were glyphs to begin with, which
/// meant they took the label's font size and came out smaller than the type
/// they sat with; sizing them up through markup then made the whole *line*
/// taller, so the words sat on a baseline set by the mark and read as having
/// slipped downwards. An image beside a label has neither problem, and the
/// mark is drawn at whatever size suits rather than at whatever the text is.
pub(crate) fn marked_face(mark: gtk::Image, words: &str) -> gtk::Box {
    let face = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();

    mark.set_valign(gtk::Align::Center);
    face.append(&mark);

    if !words.is_empty() {
        let text = gtk::Label::new(Some(words));
        text.set_valign(gtk::Align::Center);
        face.append(&text);
    }
    face
}

/// How wide the media page is allowed to get, in interface units.
///
/// A ceiling rather than a shape. The reason to stop widening is that a line
/// of prose gets too long to read and a row's value drifts too far from its
/// label - both of which are about width alone, so nothing here consults the
/// height. Below this the page simply fills the window at any proportion.
///
/// **1920 is not an arbitrary round number.** The automatic interface size is
/// the display's height over 1080, so on any 16:9 screen shown fullscreen this
/// works out to `1920 * height / 1080`, which is `height * 16/9` - the screen's
/// own width. A television therefore fills edge to edge, which is what the
/// page is composed for and what the 16:9 rule this replaced did directly.
/// Anything wider than 16:9, or any window short of fullscreen, is where the
/// ceiling starts doing something, and what is left over goes to the backdrop
/// on either side.
///
/// Set at 1600 first, which quietly left a 1920px screen with 320px of
/// backdrop down the sides at fullscreen - the one case that most wants
/// filling.
pub(crate) const PAGE_MAX_UNITS: f64 = 1920.0;

/// How wide a dialog is allowed to get, in interface units.
///
/// The same 900 the notices page and the selector popovers already stop at,
/// and for the same reason: past about this much, a line of prose is longer
/// than the eye tracks back from. Shared by every panel that asks a question,
/// so two of them in a row are the same shape.
pub(crate) const DIALOG_MAX_UNITS: f64 = 900.0;

/// How much of the page's height the poster takes.
///
/// Wider than it was, and the width is the point: the poster and the column
/// beside it share one line, so a broader poster is what sets how wide the
/// summary runs. The extra depth on both sides is what fills a 16:9 screen
/// rather than leaving a band along the bottom.
pub(crate) const POSTER_SHARE: f64 = 0.58;

/// The padding `.tp-selector > contents` draws around a selector's list,
/// which its own width has to account for. Kept beside the stylesheet value it
/// mirrors - `panel_pad` - because the two have to agree.
pub(crate) const SELECTOR_PAD: f64 = 8.0;

/// How narrow a selector is allowed to get, in interface units.
///
/// A list of short entries - "None", "Stereo", a two-word device name - would
/// otherwise open as a sliver, which reads as something gone wrong rather than
/// as a deliberately small menu.
pub(crate) const SELECTOR_MIN_WIDTH: f64 = 300.0;

/// How wide a selector is allowed to get before its entries ellipsize.
pub(crate) const SELECTOR_MAX_WIDTH: f64 = 900.0;

/// How tall a selector is allowed to get before it scrolls instead.
///
/// Not a share of the window, deliberately: a popover that fills the screen is
/// the full-screen chooser this replaces. This is roughly a dozen rows, which
/// is enough for every device list and short enough that the page it belongs
/// to is still visible around it - which is the whole reason for a popover.
pub(crate) const SELECTOR_HEIGHT: f64 = 520.0;

/// Three lines of summary, in interface units, reserved whether the film has
/// a summary or not.
///
/// The one fixed height on the page, and the only one that earns it: a plot
/// runs from nothing to a paragraph while everything else here is one line or
/// absent, so it is the only thing that would move the rows underneath as you
/// step from one film to the next. A film with no summary gets the space as
/// blank rather than getting it back.
pub(crate) const PLOT_UNITS: f64 = 90.0;

/// What stands in for a poster when there is none, which is most of the time.
///
/// A PNG per theme rather than the SVG it was drawn from, for the reason
/// [`lockup_image`] gives: GStreamer's Windows distribution ships no gdk-pixbuf
/// loaders, so nothing there can decode an SVG at runtime. The two versions
/// carry the same ink as the fullscreen marks beside them.
pub(crate) fn video_file_image(size: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../../data/ui/video-file.png");

    let image = gtk::Image::new();
    if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_static(ICON)) {
        image.set_paintable(Some(&texture));
    }
    // Drawn well inside the frame rather than filling it: the mark is saying
    // there is no artwork, and one that reached the edges would read as
    // artwork.
    image.set_pixel_size(size.round().max(1.0) as i32);
    image.set_halign(gtk::Align::Center);
    image.set_valign(gtk::Align::Center);
    // Expands to centre itself in the frame. The request stops at the poster
    // column, which sets its own `hexpand` explicitly.
    image.set_hexpand(true);
    image.set_vexpand(true);
    // Decoration beside a title that already names the file.
    image.set_accessible_role(gtk::AccessibleRole::Presentation);
    image
}

/// Uppercased here rather than with the `text-transform` CSS property,
/// which needs a newer GTK than this project's baseline.
/// Whether a scrap of clipboard text is worth offering as something to open.
///
/// Deliberately shallow: it is looking for a mistake worth not making, not
/// deciding whether the thing exists. A sentence someone happened to copy is
/// rejected, an address or a path is offered, and being wrong costs a
/// selected field the next keystroke replaces.
pub(crate) fn looks_openable(text: &str) -> bool {
    if text.is_empty() || text.lines().count() > 1 {
        return false;
    }
    text.contains("://") || text.starts_with("\\\\") || std::path::Path::new(text).is_absolute()
}

pub(crate) fn heading_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(&text.to_uppercase()));
    label.add_css_class("tp-title");
    label
}

/// The four-corner mark for entering or leaving fullscreen.
///
/// The subtitle mark for the control bar.
///
/// One white version rather than a light and a dark one: unlike the menus,
/// the control strip draws its own dark background whatever the theme is, so
/// there is nothing for a second version to adapt to.
pub fn subtitles_image(scale: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../../data/ui/subtitles.png");
    marked_image(ICON, 26.0 * scale)
}

/// The mark on the button that opens one output's soundtracks.
///
/// A note rather than a speaker, which is the whole reason the soundtracks and
/// the levels are on separate buttons: two speakers side by side say nothing
/// about which is which, and this one is a choice about the film rather than
/// about how loud it is.
///
/// Bundled rather than taken from the icon theme, like every other mark on this
/// strip: GStreamer's Windows bundle ships no icon theme at all, and a missing
/// icon draws as a broken-image box.
pub fn soundtrack_image(scale: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../../data/ui/soundtrack.png");
    marked_image(ICON, 26.0 * scale)
}

/// The mark on the button that puts an output back in sync.
///
/// Drawn rather than taken from the icon theme, for the same reason the
/// fullscreen and subtitle marks are: nothing in the theme means "line these
/// up". `emblem-synchronizing-symbolic` comes closest and is in Adwaita, but
/// GStreamer's Windows bundle ships no icon theme at all - there is only the
/// set GTK compiles into itself - and a missing icon draws nothing rather
/// than failing, which is the worst way to find out.
///
/// One version rather than a light and a dark one, like the subtitle mark:
/// the control strip draws its own dark background whatever the theme is.
/// The size of the strip's icons, before scaling: the transport buttons, the
/// gear, and the buttons in the volume panel.
pub const ICON_PX: f64 = 24.0;

pub fn sync_image(scale: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../../data/ui/sync.png");
    marked_image(ICON, SYNC_MARK_PX * scale)
}

/// The sync mark's size before scaling, which is deliberately not [`ICON_PX`]
/// like everything else in that panel.
///
/// Larger, so that it *looks* the same size. The speaker above it is a themed
/// icon whose glyph fills its box, while this is a drawn mark with clear space
/// around it - so at the same nominal size the stopwatch came out visibly the
/// smaller of the two.
///
/// The number is arithmetic rather than taste: the mark's ink fills 83% of its
/// canvas, the speaker draws 29px, and 29 / 0.83 / 1.25 lands here. Eyeballing
/// the screenshot first gave 32, which would have overshot to 34px - so if the
/// artwork is ever redrawn with different margins, measure the ink rather than
/// nudging this by feel.
const SYNC_MARK_PX: f64 = 28.0;

/// The fullscreen mark, in the direction it will take you.
///
/// Drawn for this application rather than taken from the icon theme: the
/// bundled theme has 157 icons and none of them mean fullscreen. The nearest,
/// `window-maximize-symbolic`, is a small square that reads as "maximize".
///
/// Drawn twice in each direction, once in each theme's foreground color,
/// because an embedded image cannot be recoloured the way a symbolic icon is.
/// A single compromise gray read poorly against both.
///
/// **`dark` is about the surface, not about the theme.** The control strip
/// draws its own near-black background under either theme, so it asks for the
/// dark-theme mark always - see [`marked_image`].
pub fn fullscreen_image(fullscreen: bool, scale: f64) -> gtk::Image {
    const ENTER: &[u8] = include_bytes!("../../data/ui/fullscreen.png");
    const LEAVE: &[u8] = include_bytes!("../../data/ui/restore.png");

    let bytes = match fullscreen {
        true => LEAVE,
        false => ENTER,
    };
    marked_image(bytes, CORNER_MARK_PX * scale)
}

/// The gear, for the settings screen.
///
/// A pair like the fullscreen marks, and for the same reason: it sits on the
/// page under either theme. It used to be `emblem-system-symbolic` from the
/// icon theme, which GTK recolors from the foreground - including dimming it
/// when the window loses focus, while the drawn mark beside it did not, so the
/// two came apart every time the window went to the back.
/// `size` is in real pixels and is the caller's to decide, because the gear
/// appears at two sizes: beside the fullscreen mark on the media page, where
/// the two have to agree, and among the transport icons on the control strip,
/// where it has to agree with those instead.
pub fn settings_image(size: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../../data/ui/settings.png");

    marked_image(ICON, size)
}

/// The mark on every Back button: the one in a page header, and the one that
/// leaves playback.
///
/// A bundled mark rather than `go-previous-symbolic`, which is what these used
/// to be. Two reasons, and the second is the one that matters. A themed icon
/// takes its size from the theme, which knows nothing about `ui_scale`, so the
/// arrow stayed put while everything around it doubled on a television. And
/// the control strip already spends that same glyph on skipping back, so the
/// two would have sat on one bar meaning different things.
///
/// `size` is in real pixels and is the caller's to decide, because it appears
/// at two sizes: in a header, where it answers to the slot it sits in, and
/// among the transport icons, where it has to agree with those.
pub fn back_image(size: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../../data/ui/back.png");

    marked_image(ICON, size)
}

/// How large the back mark is drawn in a page header, before scaling. Set
/// against the slot it sits in rather than against the transport icons.
const BACK_MARK_PX: f64 = 22.0;

/// How large the two marks in the media page's corner are drawn, before
/// scaling. One number for both, so they cannot drift apart.
pub(crate) const CORNER_MARK_PX: f64 = 26.0;

/// The mark beside a name in the file browser, in interface units: the height
/// of the box a file mark is drawn into.
///
/// The marks are cropped to their ink before they are bundled, which is what
/// makes one number mean the same thing for all of them. Drawn as exported
/// they carried a wide empty margin - the page shape filled 54% of its canvas
/// across and 67% down - so a size set by eye against the icons that came
/// before was mostly padding, and the marks came out small however large the
/// number grew.
/// The operating system, written as people write it. `std::env::consts::OS`
/// answers in lowercase identifiers - "macos", "windows" - which read as a
/// build target rather than as a machine.
pub(crate) fn os_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    }
}

/// How much room the About text keeps inside its panel, in interface units.
pub(crate) const ABOUT_INSET: f64 = 18.0;

/// How wide the mark is drawn on the empty screen, in interface units.
///
/// The size the mark has been drawn at since the logo there was enlarged,
/// so taking the name out from under it changes what the screen says and not
/// how big the picture is. Half again the size it was before that, which was
/// judged too timid for the one screen whose whole job is to introduce the
/// application.
pub(crate) const EMPTY_MARK: f64 = 99.0;

/// How wide the horizontal lockup is drawn in the settings header, in
/// interface units.
///
/// The lockup is 4.5:1, so this is 48 units tall, which is deeper than the
/// header's own 38-unit footprint - the header grows to hold it rather than
/// the logo being kept down to a row sized for a back arrow.
pub(crate) const SETTINGS_LOCKUP: f64 = 218.0;

/// The corner radius of a panel, in interface units.
///
/// Named because two things have to agree about it: the stylesheet rounds
/// `.tp-menu-panel` by this much, and the logo above that panel is inset by
/// the same amount so it stands level with where the corner's curve begins
/// rather than with the edge it never quite reaches.
pub(crate) const PANEL_RADIUS: f64 = 16.0;

/// How tall the notices are allowed to grow before they scroll, as a share of
/// the window. A dialog is a thing on top of a screen, and one that reaches
/// the edges is a screen wearing a border.
pub(crate) const NOTICES_SHARE: f64 = 0.8;

/// How wide the notices dialog is allowed to get, in interface units. About
/// the length of line prose is comfortable to read.
pub(crate) const NOTICES_WIDTH: f64 = 900.0;

/// How wide the settings screen's column of categories is, in interface
/// units. Fixed rather than sized to its contents, so the pane beside it does
/// not move when the longest category name changes.
pub(crate) const CATEGORY_WIDTH: f64 = 260.0;

const ROW_MARK_PX: f64 = 34.0;

/// The same, for a folder in a listing. A little smaller: a folder is a wide
/// shape where a page is a tall one, so an equal box fills more of the line
/// with ink and puts the folders ahead of the files in a list that is mostly
/// files.
const FOLDER_MARK_PX: f64 = 29.0;

/// The folder on the button that opens the system browser, which is smaller
/// again: a mark beside a line of text rather than one standing on its own.
pub(crate) const BUTTON_FOLDER_PX: f64 = 24.0;

/// How wide the marks' column is, whichever mark is in it. Wide enough for the
/// broadest of them with a little air, so the names line up down the list.
const MARK_COLUMN_PX: f64 = 32.0;

/// The triangle on the play button, and the arrow on restart.
///
/// White under either theme, because both sit on the blue button rather than
/// on the page. The play mark is deliberately not the one the control strip
/// uses: that one is the theme's own transport icon, and this is the button a
/// whole page is pointing at.
pub fn play_image(scale: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../../data/ui/play.png");
    marked_image(ICON, PLAY_MARK_PX * scale)
}

pub fn restart_image(scale: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../../data/ui/restart.png");
    marked_image(ICON, PLAY_MARK_PX * scale)
}

/// How large the marks on the play and restart buttons are drawn, before
/// scaling. Bigger than the strip's icons: these are the one action the page
/// exists to offer, and they are read from across a room.
pub(crate) const PLAY_MARK_PX: f64 = 26.0;

/// An image from bytes compiled into the binary, at a size in real pixels.
///
/// The size is set here rather than in the stylesheet because `-gtk-icon-size`
/// sizes icon *names*, and every mark in this application is a paintable - so
/// the CSS that catches a themed icon passes silently over these. A pixel or
/// two out and a button is a different width, which in the volume panel moves
/// the start of a bar and leaves the two bars visibly different lengths.
pub(crate) fn marked_image(bytes: &'static [u8], size: f64) -> gtk::Image {
    let image = gtk::Image::new();
    match gdk::Texture::from_bytes(&glib::Bytes::from_static(bytes)) {
        Ok(texture) => image.set_paintable(Some(&texture)),
        // Said out loud: a mark that silently fails to appear looks like a
        // button with nothing on it, which is not a clue anyone can act on.
        Err(e) => eprintln!("Could not load an interface mark: {e}"),
    }
    image.set_pixel_size(size.round().max(1.0) as i32);
    image
}

/// Publishes the current row as the list's `active-descendant`.
///
/// **This is not what makes a list audible.** Rows take focus and the focus
/// moves with the selection, and a screen reader speaks on focus changes and
/// on nothing else. Verified 2026-08-05 against Windows UI Automation:
/// stepping down the settings list moves the focused element from one
/// `ListItem` to the next, each named with its full row text.
///
/// Kept because the relation is correct by the specification and costs
/// nothing, but nothing should be built on it announcing anything. Publishing
/// the current item as state alone was tried twice - selection on the rows,
/// then this relation - and both were silent in practice.
///
/// Hung off `row-selected` rather than off the places that select, because
/// there are many of those - arrow keys, the gamepad, page keys, a pointer,
/// and every screen that opens on a remembered row - and one signal catches
/// them all.
pub(crate) fn announce_selection(list: &gtk::ListBox) {
    list.connect_row_selected(|list, row| match row {
        Some(row) => {
            list.update_relation(&[gtk::accessible::Relation::ActiveDescendant(
                row.upcast_ref(),
            )]);
        }
        None => list.reset_relation(gtk::AccessibleRelation::ActiveDescendant),
    });
}

/// Appends a row to a list and gives it a name.
///
/// The name goes on the row GTK wraps around the widget, not on the labels
/// inside it, because GTK derives a name from a child label but not from a
/// grandchild. A row built as a box of two labels therefore had no name, and
/// a screen reader announced it as "3 of 6" and nothing more.
pub(crate) fn append_named(list: &gtk::ListBox, child: &impl IsA<gtk::Widget>, name: &str) {
    list.append(child);
    if let Some(row) = child.as_ref().parent().and_downcast::<gtk::ListBoxRow>() {
        name_it(&row, name);
        // The list is one stop in the tab order, not one per row. A folder of
        // two hundred files is otherwise two hundred presses between you and
        // the button below it, which is the difference between usable and
        // not for anyone who navigates by Tab.
        //
        // Rows stay focusable all the same, and the focus follows the
        // selection. Making them unfocusable was the obvious way to get one
        // tab stop and it silenced the screen reader completely: focus
        // arrived at the list and never moved again, so Narrator read the
        // list and its first row and then nothing, however far down somebody
        // travelled. Selection alone is not enough - checked against Windows
        // UI Automation, which showed `IsSelected` moving correctly from row
        // to row while the focused element stayed the list throughout, and a
        // screen reader speaks on focus.
        //
        // One tab stop comes from `move_focus_stop` instead, which finds the
        // stop containing the focus and steps to the next one, so a focused
        // row still counts as being on its list.
    }
}

/// Opens a folder in whatever the machine browses files with.
///
/// **Not `AppInfo::launch_default_for_uri`, which was tried first.** GIO
/// answers "No application is registered as handling this file" for a
/// `file://` directory on Windows - measured 2026-08-12, with the link
/// reaching this code and the launch failing every time. There is nothing to
/// register: the shell is what opens folders, and GIO's table of URI handlers
/// does not know that.
///
/// So each platform is asked in its own words. The exit status is deliberately
/// not read: `explorer.exe` reports failure on success often enough to be
/// famous for it, and there is nothing useful to do with the answer anyway.
pub(crate) fn show_folder(folder: &std::path::Path) {
    #[cfg(target_os = "windows")]
    let mut opener = {
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new("explorer");
        // No console window for a GUI application to flash up behind itself.
        command.creation_flags(0x0800_0000);
        command
    };
    #[cfg(target_os = "macos")]
    let mut opener = std::process::Command::new("open");
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let mut opener = std::process::Command::new("xdg-open");

    if let Err(e) = opener.arg(folder).spawn() {
        eprintln!("Could not open {}: {e}", folder.display());
    }
}

/// The explanation drawn under a settings row.
///
/// Never selectable and never focusable. It is not a control and not a value:
/// a caret landing in it, or an arrow key stopping on it, would be the
/// interface answering a question nobody asked.
pub(crate) fn row_note(text: &str, scale: f64) -> gtk::Label {
    let px = |base: f64| (base * scale).round() as i32;
    let label = gtk::Label::new(Some(text));
    label.add_css_class("tp-row-note");
    label.set_xalign(appearance::text_start());
    label.set_justify(appearance::text_justify());
    label.set_wrap(true);
    label.set_can_focus(false);
    // Lined up with the name above it, which sits inside the row's own
    // padding, and clear of the row below.
    label.set_margin_start(px(18.0));
    label.set_margin_end(px(18.0));
    label.set_margin_bottom(px(10.0));
    label
}

/// How a settings row reads aloud: the setting, then what it is set to.
pub(crate) fn row_name(label: &str, value: &str) -> String {
    if value.is_empty() {
        label.to_string()
    } else {
        format!("{label}, {value}")
    }
}

/// Gives a control a name for anyone who cannot see the picture on it. The
/// same reasoning as the copy in `controls`, which names the playback strip.
pub(crate) fn name_it(widget: &impl IsA<gtk::Accessible>, name: &str) {
    widget.update_property(&[gtk::accessible::Property::Label(name)]);
}

fn back_button(scale: f64) -> gtk::Button {
    // A mark rather than a text glyph: a "‹" character sits off the
    // vertical center because it's positioned by font metrics rather than
    // by the icon's own bounding box. Sized here rather than left to the
    // theme - see [`back_image`].
    let button = gtk::Button::new();
    button.set_child(Some(&back_image(BACK_MARK_PX * scale)));
    button.add_css_class("tp-back");
    name_it(&button, &tr!("Back"));
    button.set_valign(gtk::Align::Center);
    button
}

/// How a level reads in the settings menu. A silenced output says so rather
/// than showing the level it will return to, which is what the panel during
/// playback does too.
pub fn volume_label(level: f64, muted: bool) -> String {
    if muted {
        "Muted".to_string()
    } else {
        format!("{}%", (level * 100.0).round() as u32)
    }
}

/// The go-ahead button on a dialog: what it says, and whether pressing it
/// destroys something.
///
/// The two travel together because the second decides which button wears the
/// warning colour, and answering one without the other is what produced a red
/// Cancel sitting beside a plain Remove.
pub(crate) struct Confirm<'a> {
    pub(crate) label: &'a str,
    pub(crate) destructive: bool,
}

/// A panel over the screen that opened it: a heading, then whatever it has to
/// say, centered.
///
/// Named for the wizard it was written for. What is left of that is the shape:
/// a heading, some lines to read, and buttons - which is what a confirmation
/// and the sandbox instructions both are.
pub(crate) fn wizard_page(title: &str) -> gtk::Box {
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(20)
        .valign(gtk::Align::Center)
        .margin_top(40)
        .margin_bottom(40)
        .margin_start(56)
        .margin_end(56)
        .build();
    let heading = heading_label(title);
    heading.set_halign(gtk::Align::Center);
    // halign centers the label in the panel; justify centers the lines within
    // the label. Without it a heading that wraps, or one written across two
    // lines, sits centered as a block with its second line ragged left.
    heading.set_justify(gtk::Justification::Center);
    page.append(&heading);
    page
}

/// A line of explanation on a wizard panel. Selectable, so a command or a
/// path can be copied out with Ctrl+C, but never focusable: these are read,
/// not operated.
pub(crate) fn wizard_text(text: &str, command: bool) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text)
        .wrap(true)
        .wrap_mode(if command {
            gtk::pango::WrapMode::Char
        } else {
            gtk::pango::WrapMode::WordChar
        })
        .justify(gtk::Justification::Center)
        .halign(gtk::Align::Center)
        .css_classes([if command { "tp-path" } else { "tp-hint" }])
        .build();
    label.set_selectable(true);
    label.set_can_focus(false);
    label
}

/// A settings row carrying a switch rather than the word "On" or "Yes".
///
/// The switch is a readout, not a control: it cannot be clicked or focused,
/// and the row it sits in is what gets activated. That keeps one way of
/// working the menu - move to a row, press it - rather than a second target
/// inside the row that only a pointer could reach.
pub(crate) fn switch_row(label: &str, on: bool) -> (gtk::Box, gtk::Switch) {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(24)
        .build();
    row.add_css_class("tp-row");

    let name = gtk::Label::new(Some(label));
    name.set_xalign(appearance::text_start());
    name.set_justify(appearance::text_justify());
    name.set_hexpand(true);
    row.append(&name);

    let switch = gtk::Switch::new();
    switch.set_active(on);
    // A switch already reports whether it is on; without a name it reports
    // that about nothing in particular.
    name_it(&switch, label);
    switch.set_can_focus(false);
    switch.set_valign(gtk::Align::Center);
    row.append(&switch);

    (row, switch)
}

/// A settings row carrying a slider rather than a value and a chevron.
///
/// A level is a quantity, not a choice from a list, and a list of ten
/// percentages was a menu pretending to be a dial. Left and right move it
/// where they would otherwise do nothing on this screen, and the row keeps
/// the reading beside it so it can be set without looking at the bar.
/// A row with a bar, its reading, and for the ones that can be turned off, a
/// switch beyond it.
///
/// The switch rather than a value of its own: muted is not a quieter level
/// and an unapplied delay is not a shorter one, so both are a second thing
/// about the row, and the bar keeps saying what it will be when it is back
/// on.
pub(crate) fn slider_row(
    label: &str,
    width: i32,
    range: std::ops::RangeInclusive<f64>,
    now: f64,
    reading: &str,
    toggle: Option<bool>,
) -> (gtk::Box, gtk::Scale, gtk::Label, Option<gtk::Switch>) {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(24)
        .build();
    row.add_css_class("tp-row");

    // The label takes the slack instead of the slider, which keeps the bar
    // over on the right where every other row shows its value. A bar the
    // width of the screen also reads as far more precision than a level has.
    let name = gtk::Label::new(Some(label));
    name.set_xalign(appearance::text_start());
    name.set_justify(appearance::text_justify());
    name.set_hexpand(true);
    row.append(&name);

    let scale = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        *range.start(),
        *range.end(),
        1.0,
    );
    scale.set_draw_value(false);
    scale.set_size_request(width, -1);
    scale.set_can_focus(false);
    scale.set_value(now);
    scale.add_css_class("tp-progress");
    // Settings bars only. The same class draws the video timeline and the
    // bars in the volume panel, which sit over a picture rather than on a
    // page of rows and are not the ones that disappear into the background.
    scale.add_css_class("tp-bar");
    name_it(&scale, label);
    row.append(&scale);

    // Wide enough for the longest reading any slider shows, so the bar beside
    // it never shifts as the value changes. `set_width_chars` is a minimum
    // rather than a maximum, so a reading longer than this would still push
    // the bar - which is what made the sync slider jump under the pointer
    // while it was being dragged, since "In sync" and "1000 ms earlier" are
    // eight characters apart.
    //
    // The same width for every slider rather than one each. A per-row width
    // would leave the bars ending at different places down the column, which
    // is worse to look at than the whitespace a short reading leaves here.
    let value = gtk::Label::new(Some(reading));
    value.add_css_class("tp-value");
    value.set_xalign(appearance::text_end());
    value.set_justify(appearance::text_justify());
    value.set_width_chars(READING_CHARS);
    row.append(&value);

    // The wheel scrolls the list it is in, rather than moving the bar under
    // the pointer. A settings screen is a list first: passing over a slider
    // on the way down it should not change a setting, and the value that
    // changes is the one nobody was looking at.
    //
    // Taken in the capture phase so the bar never sees it, and passed on to
    // the scroller by hand, since stopping the event stops it reaching the
    // list as well.
    let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
    wheel.connect_scroll(|controller, _, down| {
        let Some(scroller) = controller
            .widget()
            .and_then(|widget| widget.ancestor(gtk::ScrolledWindow::static_type()))
            .and_downcast::<gtk::ScrolledWindow>()
        else {
            return glib::Propagation::Stop;
        };
        let adjustment = scroller.vadjustment();
        // A row at a time, near enough: the step increment on a list is the
        // height of what it holds, and a tenth of a page where it is not set.
        let step = if adjustment.step_increment() > 0.0 {
            adjustment.step_increment()
        } else {
            adjustment.page_size() / 10.0
        };
        let wanted = adjustment.value() + down * step;
        adjustment.set_value(wanted.clamp(
            adjustment.lower(),
            (adjustment.upper() - adjustment.page_size()).max(adjustment.lower()),
        ));
        glib::Propagation::Stop
    });
    scale.add_controller(wheel);

    let toggle = toggle.map(|on| {
        let switch = gtk::Switch::new();
        switch.set_active(on);
        name_it(&switch, label);
        switch.set_can_focus(false);
        switch.set_valign(gtk::Align::Center);
        row.append(&switch);
        // A bar that cannot be moved says so, rather than being moved to no
        // effect and leaving somebody to work out why nothing changed.
        scale.set_sensitive(on);
        value.set_sensitive(on);
        switch
    });

    (row, scale, value, toggle)
}

/// One piece of the notices page.
pub(crate) enum Notice {
    Heading(String),
    Text(String),
}

/// Turns THIRD-PARTY.md into something worth reading on a screen.
///
/// Not a Markdown renderer, and it does not need to be: the file is headings,
/// paragraphs and tables, and only the tables need doing anything to. A row of
/// pipes reads as punctuation rather than as a list, so the cells are joined
/// with a dash - `serde - 1.0.229 - MIT OR Apache-2.0` - and the rule under
/// each header is dropped, having nothing to say without the pipes around it.
///
/// Paragraphs are gathered rather than emitted line by line, so that text
/// wrapped at eighty columns in the file wraps to the window here instead.
pub(crate) fn notices_blocks(source: &str) -> Vec<Notice> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();

    let flush = |paragraph: &mut Vec<String>, blocks: &mut Vec<Notice>| {
        if !paragraph.is_empty() {
            blocks.push(Notice::Text(paragraph.join(" ")));
            paragraph.clear();
        }
    };

    for line in source.lines() {
        let line = line.trim();
        // The rule under a table header, which is pipes and dashes and no
        // words at all.
        if line.starts_with('|') && line.trim_matches(['|', '-', ':', ' ']).is_empty() {
            continue;
        }
        if let Some(heading) = line.strip_prefix("## ").or_else(|| line.strip_prefix("# ")) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Notice::Heading(heading.to_string()));
        } else if let Some(row) = line.strip_prefix('|') {
            // A table row stands alone rather than joining the paragraph
            // around it: two hundred crates read as a list, not as prose.
            flush(&mut paragraph, &mut blocks);
            let cells: Vec<&str> = row
                .trim_end_matches('|')
                .split('|')
                .map(str::trim)
                .filter(|cell| !cell.is_empty())
                .collect();
            blocks.push(Notice::Text(cells.join("  -  ")));
        } else if line.is_empty() {
            flush(&mut paragraph, &mut blocks);
        } else {
            // Markdown decoration that would otherwise be read aloud as
            // punctuation, and the note marker, which is a label for a
            // renderer rather than words for a reader.
            let text = line
                .trim_start_matches("> ")
                .trim_start_matches('>')
                .trim_start_matches("- ")
                .replace("**", "")
                .replace('`', "");
            if text.trim() == "[!NOTE]" {
                continue;
            }
            paragraph.push(text.trim().to_string());
        }
    }
    flush(&mut paragraph, &mut blocks);
    blocks
}

/// A heading within a page of prose. Named rather than styled inline so the
/// About page reads as a document rather than as a form.
pub(crate) fn about_heading(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("tp-about-heading");
    label.set_xalign(appearance::text_start());
    label.set_justify(appearance::text_justify());
    label.set_wrap(true);
    label.set_selectable(true);
    label.set_can_focus(false);
    label
}

/// Where the address sits in relation to the sentence introducing it.
/// A line ending in a link that opens in the machine's browser. The address
/// is shown as written rather than hidden behind words, since on a screen
/// nobody can click there is still a use in being able to read it out.
///
/// Always on the same line as the sentence introducing it. There was a second
/// arrangement that put a long address on a line of its own, because one read
/// character by character is hard to pick back out of a wrapped paragraph -
/// and the only long address has gone, the notices it pointed at now being a
/// row directly below rather than a page on the web.
pub(crate) fn about_link(lead: &str, href: &str, shown: &str) -> gtk::Label {
    let label = about_text("");
    label.set_markup(&format!(
        "{} <a href=\"{}\">{}</a>",
        glib::markup_escape_text(lead),
        glib::markup_escape_text(href),
        glib::markup_escape_text(shown),
    ));
    label
}

pub(crate) fn about_text(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("tp-about");
    label.set_xalign(appearance::text_start());
    label.set_justify(appearance::text_justify());
    label.set_wrap(true);
    // Selectable so a path or a version can be copied out rather than
    // transcribed, but never focusable: GTK gives a selectable label focus by
    // default, which would put a caret in the middle of a page navigated by
    // arrow keys.
    label.set_selectable(true);
    label.set_can_focus(false);
    // Long enough to read as paragraphs, short enough that the eye finds the
    // next line: a line the width of a television is unreadable.
    label.set_max_width_chars(72);
    label
}

/// Puts a list on a row, and scrolls it there once there is a page to scroll.
///
/// Focus alone is not enough. A screen is built and handed to the window in
/// one go, so at the moment a row is focused nothing has been laid out yet:
/// the scroller has no height, the row has no position, and the scroll that
/// would have followed the focus has nowhere to go. Coming back to a screen
/// therefore landed at the top of it, however far down you had been.
///
/// So the scroll is done by hand, and not until the row has been mapped -
/// which is the point at which it knows where it is.
pub(crate) fn settle_on(row: &gtk::ListBoxRow) {
    let ticket = claim_settling();
    // The row itself, so a screen reader has a focus change to announce.
    row.grab_focus();
    // Setting the window's child maps the new page there and then, so by the
    // time a screen picks its row the row is usually mapped already and
    // waiting for the signal would be waiting forever. Only the first screen
    // of a session arrives unmapped, because the window itself is not up yet.
    if row.is_mapped() {
        after_layout(row, ticket);
    } else {
        row.connect_map(move |row| after_layout(row, ticket));
    }
}

thread_local! {
    /// Which settling is the current one. See [`claim_settling`].
    static SETTLING: Cell<u64> = const { Cell::new(0) };
}

/// Claims the right to be the row the deferred work below settles on, and
/// supersedes whatever claimed it before.
///
/// Settling a row is not finished when [`settle_on`] returns: where the row is
/// and how far the scroller can travel are only known after a layout pass, so
/// the last of it waits for an idle. Nothing about that idle was tied to the
/// row still being the one wanted, and holding an arrow key queues one per
/// press - so the earlier ones came due against rows already left behind, and
/// scrolled them back into view. Arrowing quickly down a list threw the page
/// back to wherever the cursor had been a few presses ago.
///
/// A ticket rather than cancelling the pending idle: several things settle
/// rows - the arrow keys, a screen being built, a popover opening over one -
/// and none of them knows about the others. Each takes the next number, and
/// deferred work runs only while its number is still the current one, so the
/// most recent claim always wins without anybody having to be told.
///
/// This does not replace [`focus_is_outside`], which covers what a ticket
/// cannot: moving from the top row up to a header button focuses the button
/// without settling anything, so no new ticket is taken and only the focus
/// check sees that the row is no longer where the viewer is.
pub(crate) fn claim_settling() -> u64 {
    SETTLING.with(|settling| {
        let ticket = settling.get().wrapping_add(1);
        settling.set(ticket);
        ticket
    })
}

/// Whether this settling is still the one in force.
fn settling_is_current(ticket: u64) -> bool {
    SETTLING.with(|settling| settling.get() == ticket)
}

/// Runs once the page has been through a layout pass, which is when a row
/// finally knows where it is and the scroller knows how much of it there is
/// to move.
fn after_layout(row: &gtk::ListBoxRow, ticket: u64) {
    let row = row.clone();
    glib::idle_add_local_once(move || {
        // Only while this is still the row being settled on. Anything settled
        // since - the next row under a held arrow key, another screen, a
        // popover opening - has taken a later ticket and this one is stale.
        if !settling_is_current(ticket) {
            return;
        }
        // Only if the focus is still in this list. The grab below is a second
        // attempt, for the one case where the first one was too early to take
        // - and a second attempt that runs unconditionally is a second attempt
        // at stealing the focus back from wherever it has since gone.
        //
        // Arrowing up off the top row twice in quick succession did exactly
        // that: the first press selected the top row and queued this, the
        // second moved out to the Play button, and then this fired and pulled
        // the focus back down to the row. Slowly it was fine, because the idle
        // had already run before the second press arrived - which is the shape
        // of every bug that only happens when you are not being careful.
        if focus_is_outside(&row) {
            return;
        }
        // Only if it has not already taken. A focus grab inside a scroller
        // makes GTK scroll the row into view, so repeating one that already
        // succeeded sets a second scroll going against the one below.
        if !row.has_focus() {
            row.grab_focus();
        }
        show_row(&row);
    });
}

/// Whether the focus has left the list this row belongs to.
///
/// Nothing focused at all counts as inside: that is the state on the very
/// first screen of a session, before anything has taken focus, and it is
/// precisely when the deferred grab is needed.
fn focus_is_outside(row: &gtk::ListBoxRow) -> bool {
    let (Some(root), Some(list)) = (row.root(), row.parent()) else {
        return false;
    };
    match root.focus() {
        Some(focused) => focused != list && !focused.is_ancestor(&list),
        None => false,
    }
}

/// Moves the scroller so a row is fully on screen, by the smallest amount
/// that does it.
///
/// The minimum on purpose, and it used to place the row a third of the way
/// down the frame instead - which looks better in isolation and is the wrong
/// rule here, because this is not the only thing scrolling. Focusing a row
/// inside a scroller makes GTK bring it into view too, by the smallest amount.
/// Two rules that disagree about where a row belongs produce whichever answer
/// ran last: arrowing down kept the row at the bottom edge on the presses
/// where GTK's scroll had already satisfied this one, and threw the row up
/// near the top on the presses where it had not. Nothing about the input
/// differed, so it read as random.
///
/// Agreeing with GTK is what makes it predictable, and it is also the better
/// behaviour while arrowing: the row stays where it is and the list moves one
/// row under it, rather than the page jumping every time the edge is reached.
fn show_row(row: &gtk::ListBoxRow) {
    let Some(list) = row.parent() else { return };
    let mut ancestor = list.parent();
    let scroller = loop {
        match ancestor {
            Some(widget) => match widget.downcast::<gtk::ScrolledWindow>() {
                Ok(scroller) => break scroller,
                Err(widget) => ancestor = widget.parent(),
            },
            None => return,
        }
    };

    // The row's own allocation inside the list, which is where it sits in the
    // content and does not move when the content is scrolled.
    //
    // Asked of the widget tree with `translate_coordinates` before, which
    // looks equivalent and is not. The step above this one grabs the row's
    // focus, and GTK answers a focus grab inside a scroller by scrolling the
    // row into view itself - moving the adjustment and re-allocating the list
    // underneath us. `translate_coordinates` then reported whichever
    // allocation happened to be current: the row's place in the list on one
    // press, its place on screen on the next.
    //
    // On screen it is always the same place, hard against the bottom edge, so
    // every other press computed the same destination near the top of the list
    // and jumped there - and the two answers diverged further the further down
    // the list you had gone.
    let top = f64::from(row.allocation().y());
    let adjustment = scroller.vadjustment();
    let page = adjustment.page_size();
    // Already on screen: leave it where it is rather than jumping the page
    // about under someone who can see the row perfectly well.
    let value = adjustment.value();
    let bottom = top + f64::from(row.height());
    let wanted = if top < value {
        // Off the top: bring its top edge to the top of the frame.
        top
    } else if bottom > value + page {
        // Off the bottom: bring its bottom edge to the bottom of the frame.
        bottom - page
    } else {
        return;
    };
    adjustment.set_value(wanted.clamp(adjustment.lower(), (adjustment.upper() - page).max(0.0)));
}

/// Where a stored language code sits in the offered list.
pub(crate) fn language_position(code: Option<&str>) -> Option<usize> {
    let code = code?;
    crate::languages::LANGUAGES
        .iter()
        .position(|(stored, _, _, _)| *stored == code)
}

/// As far down the About page as it goes, which is the top of the last
/// screenful rather than the bottom of the text.
pub(crate) fn about_bottom(adjustment: &gtk::Adjustment) -> f64 {
    (adjustment.upper() - adjustment.page_size()).max(adjustment.lower())
}

/// Binds an action to each of `keys` under every modifier this platform
/// answers a shortcut on.
///
/// `<Primary>` everywhere, which is Control on all three platforms, plus
/// Command on macOS - where `<Primary>` is emphatically not it. See
/// `install_accelerators` for how that was measured.
pub(crate) fn bind_accels(gtk_app: &gtk::Application, action: &str, keys: &[&str]) {
    let mut accels = Vec::new();
    for key in keys {
        accels.push(format!("<Primary>{key}"));
        if cfg!(target_os = "macos") {
            accels.push(format!("<Meta>{key}"));
        }
    }
    let accels: Vec<&str> = accels.iter().map(String::as_str).collect();
    gtk_app.set_accels_for_action(action, &accels);
}

/// The modifiers a shortcut may be pressed with here, as one mask to test a
/// key event against.
///
/// `<Primary>` is asked of GTK rather than written out per platform, so the
/// keys matched by hand cannot drift from the ones bound as accelerators.
/// Command is added on macOS for the same reason it is bound there, and it is
/// why this is tested with `intersects` rather than `contains`: the mask holds
/// two modifiers on that platform and either one alone means yes.
pub(crate) fn primary_mask() -> gdk::ModifierType {
    let mut mask = gtk::accelerator_parse("<Primary>a")
        .map(|(_, mask)| mask)
        .unwrap_or(gdk::ModifierType::CONTROL_MASK);
    if cfg!(target_os = "macos") {
        mask |= gdk::ModifierType::META_MASK;
    }
    mask
}

pub(crate) fn last_row_index(list: &gtk::ListBox) -> i32 {
    let mut last = 0;
    while list.row_at_index(last + 1).is_some() {
        last += 1;
    }
    last
}

/// One row of a soundtrack list. See [`crate::label`] for the shape, which
/// subtitles share.
pub(crate) fn describe_audio_track(track: &AudioTrack) -> String {
    crate::label::line(
        &crate::label::Parts {
            language: &track.language,
            technical: format!("{} {}ch", track.codec, track.channels),
            kind: track.kind(),
            title: &track.title,
        },
        crate::label::Naming::Native,
        // A track that will not say what it is still has to be pointable at,
        // and its number is the one thing it always has.
        &tr!("Track {number}").replace("{number}", &(track.index + 1).to_string()),
    )
}

/// A stored alignment as a statement rather than as a signed number.
///
/// Which way the audio runs is the whole of what it says, and "+830ms" does
/// not say it. This is read by someone checking a correction they cannot see
/// the effect of, so it has to be unambiguous without a convention to look up.
pub(crate) fn describe_lateness(millis: f64) -> String {
    let rounded = millis.round();
    if rounded > 0.0 {
        // Rounded before it is handed over, not inside the placeholder:
        // `fill` matches `{name}` exactly and has no format specifiers,
        // so `{ms:.0}` would have gone to screen as written.
        tr!("Audio {ms}ms late", ms = format!("{rounded:.0}")).into_owned()
    } else if rounded < 0.0 {
        tr!("Audio {ms}ms early", ms = format!("{:.0}", -rounded)).into_owned()
    } else {
        tr!("In sync").into_owned()
    }
}

/// A menu row: what the setting is on the left, its current value and a
/// chevron on the right.
/// The heading that opens a group of rows: which output the three rows under
/// it belong to.
///
/// A `GtkListBox` header rather than a row, which is what makes it
/// unselectable for free - headers sit outside the selection model and outside
/// the focus chain, so the arrow keys walk past without being told to.
///
/// Capitals with a little tracking, in the manner of a section label rather
/// than a title: it has to be legible enough to group what is under it and
/// quiet enough that the rows stay the thing being read. The tracking is a
/// Pango attribute rather than CSS `letter-spacing`, which GTK's stylesheet
/// parser accepts and does not apply.
/// What a group heading says under itself, for the groups that say anything.
///
/// Only Kodi's do. What belongs here is what is true of a whole installation
/// rather than of one setting under it - which file it is, and either why it
/// cannot be used or the thing every one of them shares: Kodi reads that file
/// once, at startup.
pub(crate) struct GroupNote {
    pub(crate) sentence: String,
    /// A folder the note offers to open. Offered rather than printed: a path
    /// read off a television is a path nobody is going to type.
    pub(crate) folder: Option<std::path::PathBuf>,
}

/// A group heading, and the line under it for the groups that have one.
///
/// The note is a `GtkLabel` styled like a row's own note and indented to the
/// same `pad_h` the heading is, so the three line up in one column.
pub(crate) fn group_header(
    title: &str,
    note: Option<&GroupNote>,
    scale: f64,
    first: bool,
) -> gtk::Widget {
    let heading = group_heading(title, scale, first);
    let Some(note) = note else {
        return heading.upcast();
    };

    let stack = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    stack.append(&heading);

    let text = row_note(&note.sentence, scale);
    if let Some(folder) = note.folder.clone() {
        // On the same line as the sentence it belongs to, the way Clear Saved
        // Playback Data offers its own folder.
        text.set_markup(&format!(
            "{}  <a href=\"{}\">{}</a>",
            glib::markup_escape_text(&note.sentence),
            glib::markup_escape_text(&gtk::gio::File::for_path(&folder).uri()),
            glib::markup_escape_text(&tr!("Open File Location")),
        ));
        // Reported rather than swallowed: a link that does nothing looks like
        // a link that was pressed wrongly.
        text.connect_activate_link(move |_, _| {
            show_folder(&folder);
            glib::Propagation::Stop
        });
    }
    stack.append(&text);
    stack.upcast()
}

pub fn group_heading(title: &str, scale: f64, first: bool) -> gtk::Label {
    let heading = gtk::Label::new(Some(title));
    heading.set_xalign(appearance::text_start());
    heading.set_justify(appearance::text_justify());
    heading.add_css_class("tp-group");
    // Nothing above the first heading. It opens the list rather than dividing
    // it, and the buttons already sit above with room of their own.
    if first {
        heading.add_css_class("tp-group-first");
    }
    let attributes = gtk::pango::AttrList::new();
    attributes.insert(gtk::pango::AttrInt::new_letter_spacing(
        (1.5 * scale * gtk::pango::SCALE as f64) as i32,
    ));
    heading.set_attributes(Some(&attributes));
    heading
}

/// A heading's capitals turned back into words, for a screen reader.
///
/// "FIRST OUTPUT" read literally is a risk of being spelled out a letter at a
/// time, which is a real behaviour of several readers on all-capital text.
pub fn title_case(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &characters.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn menu_row(label: &str, value: &str, enabled: bool) -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(24)
        .build();
    row.add_css_class("tp-row");

    let name = gtk::Label::new(Some(label));
    name.set_xalign(appearance::text_start());
    name.set_justify(appearance::text_justify());
    row.append(&name);

    let value_label = gtk::Label::new(Some(value));
    value_label.add_css_class("tp-value");
    value_label.set_hexpand(true);
    value_label.set_xalign(appearance::text_end());
    value_label.set_justify(appearance::text_justify());
    value_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&value_label);

    let chevron = gtk::Label::new(Some("›"));
    chevron.add_css_class("tp-chevron");
    row.append(&chevron);

    row.set_sensitive(enabled);
    row
}

/// What a browsing screen is for: opening a video, or choosing a folder.
///
/// The two screens differ in what they list, what the footer holds and what a
/// row does. Everything else - the trail, the places column, the system
/// browser - is the same, and used to be written twice.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Browse {
    Videos,
    /// A separate soundtrack for the video already chosen: the same screen,
    /// listing audio files instead.
    Audio,
    /// A subtitle file from somewhere other than beside the video.
    Subtitles,
    Folders,
}

impl Browse {
    /// Whether only folders are worth showing. A folder is being chosen here,
    /// so the files inside it would be a list of things that cannot be picked.
    pub(crate) fn folders_only(self) -> bool {
        self == Browse::Folders
    }

    pub(crate) fn wants(self) -> crate::browser::Kind {
        match self {
            Browse::Audio => crate::browser::Kind::Audio,
            Browse::Subtitles => crate::browser::Kind::Subtitle,
            _ => crate::browser::Kind::Video,
        }
    }
}

/// The parts of a browsing screen its caller still has to finish.
pub(crate) struct BrowserPage {
    pub(crate) page: gtk::Box,
    pub(crate) list: gtk::ListBox,
    pub(crate) crumbs: Vec<gtk::Button>,
    pub(crate) browse: gtk::Button,
    pub(crate) open: gtk::Button,
    pub(crate) cancel: gtk::Button,
}

/// One row of a listing: what it says, what it is drawn with, where it goes,
/// and how it reads aloud. A path of `None` is the way up.
#[derive(Clone)]
pub(crate) struct BrowserEntry {
    /// Whether the Open button acts on this row: a file, rather than a folder,
    /// the way up, or a notice.
    pub(crate) openable: bool,
    pub(crate) label: String,
    pub(crate) icon: RowIcon,
    pub(crate) path: Option<std::path::PathBuf>,
    pub(crate) spoken: String,
    /// Something to read rather than somewhere to go: the line saying a
    /// folder holds nothing worth listing.
    pub(crate) notice: bool,
}

/// What sits behind a modal opened before there is a screen to sit behind it.
///
/// Blank on purpose. The alternative - building a menu page to stand in for
/// the real one - draws a screen nobody navigated to, which is worse than an
/// empty background because it looks like somewhere you could go back to.
pub(crate) fn empty_backdrop() -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, 0)
}

/// Fills a listing, and leaves the notice as a line of text.
///
/// A notice drawn like an entry invites being chosen, and choosing it walked
/// back up a level - which reads as a broken listing rather than as an empty
/// folder. Centred, dimmer, without an icon, and passed over by the cursor.
pub(crate) fn fill_browser_list(list: &gtk::ListBox, entries: &[BrowserEntry], scale: f64) {
    for entry in entries {
        if entry.notice {
            let label = gtk::Label::new(Some(&entry.label));
            label.add_css_class("tp-row");
            label.add_css_class("tp-hint");
            label.set_xalign(0.5);
            append_named(list, &label, &entry.spoken);
            if let Some(row) = label.parent().and_downcast::<gtk::ListBoxRow>() {
                row.set_selectable(false);
                row.set_activatable(false);
            }
        } else {
            append_named(
                list,
                &browser_row(entry.icon, &entry.label, scale),
                &entry.spoken,
            );
        }
    }
}

/// Opens a system dialog where the built-in browser already is.
///
/// Best effort: a folder that has since been unplugged or removed leaves the
/// dialog wherever it would have opened anyway, which is better than refusing
/// to open at all.
pub(crate) fn open_at(chooser: &gtk::FileChooserNative, start: &std::path::Path) {
    if start.is_dir() {
        let _ = chooser.set_current_folder(Some(&gtk::gio::File::for_path(start)));
    }
}

/// What a folder shows in a given mode: the way up, then what is inside.
pub(crate) fn browser_entries(directory: &std::path::Path, mode: Browse) -> Vec<BrowserEntry> {
    let mut entries = Vec::new();
    if let Some(parent) = directory.parent() {
        // Two dots rather than the word: it is what a file listing has always
        // called the folder above, and it needs no translating. Read aloud it
        // is punctuation and says nothing, so the spoken name says where it
        // goes instead.
        entries.push(BrowserEntry {
            openable: false,
            label: "..".to_string(),
            icon: RowIcon::Folder,
            path: None,
            spoken: match parent.file_name() {
                Some(name) => tr!("Up to {folder}", folder = name.to_string_lossy()).into_owned(),
                None => tr!("Up to the list of drives").into_owned(),
            },
            notice: false,
        });
    }
    for entry in crate::browser::read(directory, mode.wants()) {
        if mode.folders_only() && !entry.is_dir {
            continue;
        }
        // Which mark a file gets follows what this screen is for: the same
        // file is a video when a video is being chosen and a soundtrack when
        // one is. Nothing here inspects the file itself, which would mean
        // opening every one in the folder to draw a list.
        let icon = match (entry.is_dir, mode) {
            (true, _) => RowIcon::Folder,
            (false, Browse::Audio) => RowIcon::Audio,
            (false, Browse::Subtitles) => RowIcon::Subtitle,
            (false, _) => RowIcon::Video,
        };
        entries.push(BrowserEntry {
            openable: !entry.is_dir,
            label: entry.label.clone(),
            icon,
            path: Some(entry.path),
            spoken: entry.label,
            notice: false,
        });
    }
    // Only where the listing is what you came for. A folder with nothing to
    // play in it is worth saying, since the alternative reads as a folder
    // that failed to load; a folder with no folders under it is not empty at
    // all - it is full of files this screen has no reason to show, and
    // calling it empty would be wrong.
    //
    // Counting the way up as nothing, since it fills the list on its own and
    // is why this never appeared before.
    if mode == Browse::Videos && entries.iter().all(|entry| entry.path.is_none()) {
        entries.push(BrowserEntry {
            openable: false,
            label: tr!("Nothing here").into_owned(),
            icon: RowIcon::None,
            path: None,
            spoken: tr!("Nothing here").into_owned(),
            notice: true,
        });
    }
    entries
}

/// What a browser row draws beside its name.
///
/// The file marks are bundled rather than named from the desktop's icon set.
/// The set is what a row used to ask for, and the theme decides what turns up:
/// a generic video icon is absent from the Pi's theme entirely and fell back
/// to the missing-image glyph, which reads as a warning about the file. These
/// three are the same on every machine.
///
/// The folder is bundled with them. It could have stayed the theme's - a
/// folder is the one icon every theme has - but then one mark in a column of
/// four would be drawn in somebody else's hand, and which one would depend on
/// the machine.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum RowIcon {
    Folder,
    Video,
    Audio,
    Subtitle,
    /// A notice rather than a file - "Nothing here" - which draws no mark.
    None,
}

impl RowIcon {
    /// The mark at the size a listing draws it.
    pub(crate) fn image(self, scale: f64) -> gtk::Image {
        let size = match self {
            Self::Folder => FOLDER_MARK_PX,
            _ => ROW_MARK_PX,
        };
        self.image_at(size, scale)
    }

    /// The mark at a size of the caller's choosing, for the places that are
    /// not a row in a listing.
    pub(crate) fn image_at(self, size: f64, scale: f64) -> gtk::Image {
        const VIDEO: &[u8] = include_bytes!("../../data/ui/file-video.png");
        const AUDIO: &[u8] = include_bytes!("../../data/ui/file-audio.png");
        const SUBTITLE: &[u8] = include_bytes!("../../data/ui/file-subtitle.png");
        const FOLDER: &[u8] = include_bytes!("../../data/ui/folder.png");

        let bytes = match self {
            Self::Video => VIDEO,
            Self::Audio => AUDIO,
            Self::Subtitle => SUBTITLE,
            Self::Folder => FOLDER,
            Self::None => return gtk::Image::new(),
        };
        marked_image(bytes, size * scale)
    }
}

/// A browser row: a mark, then the name.
///
/// Icons rather than emoji, because emoji depend on a color font being
/// installed. The Pi has none, so a folder character rendered as an empty box
/// with the codepoint inside it.
fn browser_row(icon: RowIcon, text: &str, scale: f64) -> gtk::Box {
    // The padding goes on the row rather than the label, so it applies
    // before the icon as well as around the text.
    //
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .css_classes(["tp-row"])
        .build();

    let image = icon.image(scale);
    image.add_css_class("tp-row-icon");
    // A column of its own, the same width whatever is drawn in it, so every
    // name in the list starts at the same place. The marks are cropped to
    // their ink and no two are the same shape - a page is tall and narrow, a
    // folder wide - so left to size themselves the folder rows and the file
    // rows put their names a couple of pixels apart, which is the sort of
    // thing that reads as sloppiness without being obvious enough to name.
    image.set_size_request((MARK_COLUMN_PX * scale).round() as i32, -1);
    image.set_halign(gtk::Align::Center);
    row.append(&image);

    let label = gtk::Label::new(Some(text));
    label.set_xalign(appearance::text_start());
    label.set_justify(appearance::text_justify());
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&label);
    row
}

/// Lines a selector up with the end of the row that opened it - the edge the
/// row's *value* sits against - leaving the vertical placement to GTK.
///
/// GTK positions a popover by centering it on a rectangle you nominate, in the
/// parent's coordinates. Here that is a one-pixel sliver half a popover's
/// width in from the value's edge, so centering the popover on it lands the
/// two edges together. The entries inside are aligned the same way, because
/// they are alternatives to that value, and a centered popover would sit just
/// off it - close enough to read as a mistake rather than a margin.
///
/// **Widget coordinates do not mirror**, whatever the text direction: x is
/// measured from the left in every language, so the sliver has to be put on
/// the other side by hand. Missing that left the chooser opening over the
/// row's *name* in a right-to-left layout rather than over its value - the one
/// thing the direction audit did not catch, because it is arithmetic rather
/// than an alignment or a stylesheet rule.
///
/// The rectangle spans the row's full height, which is GTK's own default and
/// gives its ordinary vertical behaviour: below the row when there is room,
/// flipped above it when there is not.
///
/// Aligning an edge of the popover to an edge of the row was tried and taken
/// out again. It is possible - a zero-height rectangle at `y` puts the
/// popover's near edge on that line - but it requires predicting which way GTK
/// will open, and the popover then covers the row it belongs to, which leaves
/// a choice sitting under the pointer where the row used to be. Clicking again
/// to dismiss picks that choice instead. macOS avoids this by aligning the
/// *selected* entry to the row rather than the first one, so a second click
/// picks what was already set; without that, overlapping is worse than not.
///
/// **The width cannot come from measuring the popover.** A popover is a
/// `GtkNative`: it takes no room in the widget that parents it, so measuring
/// it as a child answers zero however wide it will actually open. That zero is
/// what left an earlier attempt at this centered. The number has to come from
/// what is inside it, plus the padding the stylesheet puts around that.
pub(crate) fn aim_at_value(popover: &gtk::Popover, anchor: &gtk::ListBoxRow, width: i32) {
    if width <= 0 || anchor.width() <= 0 {
        return;
    }
    let center = match appearance::rtl() {
        true => width / 2,
        false => anchor.width() - width / 2,
    };
    popover.set_pointing_to(Some(&gdk::Rectangle::new(center, 0, 1, anchor.height())));
}

pub(crate) fn chooser_row(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("tp-row");
    label.set_xalign(appearance::text_start());
    label.set_justify(appearance::text_justify());
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label
}

/// GTK rings the system bell when a keyboard move can't go anywhere - at
/// the ends of a list, which happens constantly when navigating by
/// arrow key or D-pad. The application provides its own click instead.
pub(crate) fn suppress_error_bell() {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_error_bell(false);
        // Clicking the timeline jumps to that point. Left to the platform
        // default this differs by system: on macOS a click on the trough
        // steps toward the pointer instead of going there, which reads as a
        // seek that ignored where you clicked.
        settings.set_gtk_primary_button_warps_slider(true);
        // Holding the timeline still for a moment otherwise puts GtkRange into
        // its fine-adjustment mode: the trough grows and the slider starts
        // moving a fraction of the distance the pointer does. That is a useful
        // affordance for choosing an exact value in a settings dialog, and a
        // baffling one on a video timeline, where it looks like the playhead
        // has come unstuck from the mouse. Nothing here wants a long press, so
        // the threshold is put beyond reach rather than the behavior fought.
        // An hour, not u32::MAX: the property has a range and refuses
        // anything past it, which panics on the way up.
        settings.set_gtk_long_press_time(60 * 60 * 1000);
    }
}

/// Sizes are set here rather than left to the theme because the interface
/// is meant to be read from across a room. Everything scales from one
/// factor so it can be dialled down for close-range use.
/// Starting window size, in the same units as the interface inside it.
///
/// A fixed size would mean a 2x menu opening into a 1x frame, which is how a
/// 4K display ends up with a window too small for its own contents. Capped to
/// most of the monitor so a large scale on a modest screen still opens
/// something that fits, panels and decoration included.
pub(crate) fn default_window_size(
    scale: f64,
    monitor: Option<&gdk::Monitor>,
    saved: (Option<i32>, Option<i32>),
) -> (i32, i32) {
    // Sixteen by nine, and a good deal larger than it was. The old size was
    // 1100x700 - close to 11:7, and so a shape no film is - which left the
    // media page holding a column of empty air down the sides of its artwork
    // before anybody had touched a window edge.
    const BASE_WIDTH: f64 = 1600.0;
    const BASE_HEIGHT: f64 = 900.0;
    const MAX_FRACTION: f64 = 0.9;

    // Where it was left, if it was left anywhere. Held to the same fraction of
    // the screen as the default below: a size remembered from a larger monitor
    // would otherwise open off the edge of a smaller one.
    let (mut width, mut height) = match saved {
        (Some(width), Some(height)) if width > 0 && height > 0 => (width as f64, height as f64),
        _ => (BASE_WIDTH * scale, BASE_HEIGHT * scale),
    };
    if let Some(monitor) = monitor {
        let geometry = monitor.geometry();
        width = width.min(geometry.width() as f64 * MAX_FRACTION);
        height = height.min(geometry.height() as f64 * MAX_FRACTION);
    }
    (width.round() as i32, height.round() as i32)
}

#[cfg(test)]
mod notices {
    use super::*;

    /// The real file, since that is what ships and what the transform has to
    /// cope with. A table that still has its pipes in it is the failure this
    /// is watching for: it reads as punctuation rather than as a list.
    #[test]
    fn the_shipped_notices_read_as_text() {
        let blocks = notices_blocks(include_str!("../../THIRD-PARTY.md"));
        assert!(!blocks.is_empty(), "nothing was produced");

        let mut headings = Vec::new();
        for block in &blocks {
            match block {
                Notice::Heading(text) => headings.push(text.as_str()),
                Notice::Text(text) => {
                    assert!(!text.contains('|'), "table pipe left in: {text:?}");
                    assert!(!text.contains("**"), "bold marker left in: {text:?}");
                    assert!(!text.starts_with('>'), "quote marker left in: {text:?}");
                    assert!(text.trim() != "[!NOTE]", "note marker left in");
                }
            }
        }

        for wanted in ["Fonts", "Native libraries", "Rust dependencies"] {
            assert!(
                headings.contains(&wanted),
                "no {wanted:?} heading in {headings:?}"
            );
        }
    }

    /// A crate row keeps all three of its cells, joined rather than dropped.
    #[test]
    fn a_crate_row_keeps_its_columns() {
        let blocks = notices_blocks(
            "## Rust dependencies\n\n| Crate | Version | License |\n|---|---|---|\n| serde | 1.0.229 | MIT OR Apache-2.0 |\n",
        );
        let rows: Vec<&String> = blocks
            .iter()
            .filter_map(|block| match block {
                Notice::Text(text) => Some(text),
                _ => None,
            })
            .collect();
        assert!(
            rows.iter().any(|row| row.contains("serde")
                && row.contains("1.0.229")
                && row.contains("MIT OR Apache-2.0")),
            "the crate row lost a column: {rows:?}"
        );
        // The rule under the header carries no words and should be gone.
        assert!(!rows.iter().any(|row| row.contains("---")), "{rows:?}");
    }

    /// Paragraphs wrapped in the file wrap to the window instead.
    #[test]
    fn wrapped_prose_is_rejoined() {
        let blocks = notices_blocks("one line\nand its continuation\n\na second paragraph\n");
        let texts: Vec<&String> = blocks
            .iter()
            .filter_map(|block| match block {
                Notice::Text(text) => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert_eq!(texts[0], "one line and its continuation");
    }
}

#[cfg(test)]
mod poster_shape {
    /// The rule without the texture, so it can be checked without a display -
    /// the same reason `artwork::fitted` is written to be testable.
    fn height_for(picture: (f64, f64), slot: (f64, f64)) -> f64 {
        let (width, height) = slot;
        let aspect = picture.0 / picture.1;
        match aspect > (width / height) * 1.15 {
            true => (width / aspect).min(height),
            false => height,
        }
    }

    /// A real poster fills the slot exactly, which is the case that must not
    /// change: every library's film artwork is two by three.
    #[test]
    fn a_poster_fills_the_slot() {
        assert_eq!(height_for((1000.0, 1500.0), (300.0, 450.0)), 450.0);
        // And one a few pixels out is still treated as a poster, cropped by
        // the frame rather than reshaping it.
        assert_eq!(height_for((1000.0, 1490.0), (300.0, 450.0)), 450.0);
    }

    /// An episode still is 16:9, and gets the slot's width and its own height
    /// rather than being scaled up until its sides fall off the frame.
    #[test]
    fn a_wide_still_keeps_its_width_and_loses_height() {
        let tall = height_for((1920.0, 1080.0), (300.0, 450.0));
        assert!((tall - 168.75).abs() < 0.01, "{tall}");
        // Shorter than the slot, never taller, so the rows beside it stay put.
        assert!(tall < 450.0);
    }

    /// Taller than two by three - some libraries carry 1000x1500 and some
    /// carry narrower scans - still fills the slot, and is cropped by it.
    #[test]
    fn a_narrow_picture_still_fills_the_slot() {
        assert_eq!(height_for((1000.0, 2000.0), (300.0, 450.0)), 450.0);
    }
}

/// How tall the series' small frame should be for the picture in it.
///
/// **Whatever the picture needs, so none of it is cropped.** This one is a
/// reference rather than a composition - it says which programme this is - and
/// a poster with its title lettering cut off the bottom fails at exactly that.
/// The frame above it crops on purpose, because it is a fixed slot in a layout;
/// this is not.
///
/// Two by three until a picture turns up, which is what most posters are, so
/// the space reserved is about right before the fetch lands.
pub(crate) fn series_frame_height(texture: Option<&gdk::Texture>, width: f64) -> f64 {
    let Some(texture) = texture.filter(|texture| texture.width() > 0 && texture.height() > 0)
    else {
        return width * 3.0 / 2.0;
    };
    width * f64::from(texture.height()) / f64::from(texture.width())
}

/// The series' poster as a widget, filling its small frame.
///
/// Expanding both ways for the reason the main poster does: the widget draws a
/// texture and measures as nothing, so without it the frame allocates no room
/// at all and the picture simply does not appear.
pub(crate) fn series_picture(texture: gdk::Texture) -> crate::artwork::Artwork {
    let picture = crate::artwork::Artwork::poster();
    picture.set_texture(Some(texture));
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture
}

/// How tall the poster frame should be for the picture going into it.
///
/// The slot's full height for a poster, and only what it needs for anything
/// wider - an episode's Primary image is a 16:9 still, and cropping that into a
/// two-by-three slot scales it to fill the height and throws the sides away,
/// which is most of the picture. Reported on 2026-08-16.
///
/// **The width is never touched**, whatever the shape. It is what the column is
/// sized to, so letting it vary would shift the page beside it every time a
/// different kind of item was loaded.
pub(crate) fn poster_frame_height(texture: Option<&gdk::Texture>, width: f64, height: f64) -> f64 {
    let Some(texture) = texture.filter(|texture| texture.width() > 0 && texture.height() > 0)
    else {
        return height;
    };
    let aspect = f64::from(texture.width()) / f64::from(texture.height());
    // **With room to spare, so a near-miss is still cropped.** Libraries carry
    // posters a few pixels off two by three, and reshaping for those would let
    // the frame's height wobble from one film to the next for no reason anyone
    // could name. The two real cases are nowhere near each other - a poster is
    // about 0.67 and an episode still is 1.78 - so this only has to be wide
    // enough to tell a bad scan from a different kind of picture.
    const CLEARLY_WIDER: f64 = 1.15;
    match aspect > (width / height) * CLEARLY_WIDER {
        // Wider than the slot: keep the width, take less height.
        true => (width / aspect).min(height),
        false => height,
    }
}

/// Brings a widget up from nothing over a quarter of a second.
///
/// Artwork is loaded after the page is already up, so without this it appears
/// at full strength between one frame and the next - which reads as a fault
/// rather than as something finishing loading.
///
/// Opacity rather than a `Revealer`, which is what the controls use for their
/// panels: a revealer that is not revealed takes no space, and a poster
/// collapsing its frame and then pushing it back open would be a worse jolt
/// than the one being fixed. Opacity never touches the layout.
///
/// Driven by the frame clock rather than a timer, so it runs at whatever rate
/// the screen is actually drawing and finishes on a frame rather than between
/// two.
pub(crate) fn fade_in(widget: &impl IsA<gtk::Widget>) {
    const OVER: f64 = 0.5;

    let widget = widget.clone().upcast::<gtk::Widget>();
    widget.set_opacity(0.0);
    let started = std::time::Instant::now();
    widget.add_tick_callback(move |widget, _| {
        let progress = (started.elapsed().as_secs_f64() / OVER).clamp(0.0, 1.0);
        // Eased out, so it arrives softly rather than stopping dead.
        widget.set_opacity(1.0 - (1.0 - progress).powi(3));
        match progress >= 1.0 {
            true => glib::ControlFlow::Break,
            false => glib::ControlFlow::Continue,
        }
    });
}
