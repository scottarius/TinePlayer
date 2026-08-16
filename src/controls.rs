//! The strip of playback controls laid over the video: where you are, how
//! long the file is, whether it is running, and the means to change all three.
//!
//! The transport buttons take keyboard focus, and the focus follows whichever
//! one is highlighted. That was not always so: they were deliberately
//! unfocusable, on the grounds that a controller drives them through the same
//! actions the keyboard does and a focus state is one more thing to get wrong.
//!
//! What that cost was the screen reader. A screen reader speaks on focus
//! changes, so a highlight kept in a `Cell` and a CSS class is silence.
//! Checked against Windows UI Automation rather than guessed at: with the
//! marks published as accessible state and no focus movement, the focused
//! element stayed the window while the highlight travelled the row.
//!
//! The position scale is still unfocusable, because its own arrow bindings
//! would nudge the playhead instead of seeking, so the bar takes the focus for
//! that row and renames itself. Space is caught in the capture phase for the
//! same class of reason - see the controller beside the button row.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gstreamer as gst;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;

use crate::player::Playback;
use crate::tr;

/// Which part of the strip a controller is driving.
///
/// Left and right mean two different things - seek, or move between buttons -
/// and which one depends on this. Splitting the strip into a timeline row and
/// a button row is what makes that unambiguous: the meaning belongs to the row
/// rather than to a mode the viewer has to remember being in.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// Not being driven. The strip behaves as it always has: it appears on
    /// input and hides again, and left and right seek.
    None,
    Buttons,
    Timeline,
    /// The levels menu is open: every output's level and sync, and the main
    /// level over all of them. Up and down step through those rows, left and
    /// right move whichever one is marked.
    ///
    /// Every row in it is a bar, which is what keeps left and right meaning one
    /// thing throughout. Choosing a soundtrack is [`Row::Audio`], opened from a
    /// button of its own rather than from in here.
    Volume,
    /// A soundtrack chooser is open, for the output in `output`. Up and down
    /// move through the list, and left and right do nothing: there is nowhere
    /// sideways to go, and seeking the film out from under an open list would
    /// be worse than doing nothing.
    Audio,
    /// The subtitle chooser is open, and behaves exactly as a soundtrack one
    /// does. Up and down move through the list, and left and right do nothing.
    Subtitles,
}

/// Which button a press is being held on.
///
/// Two of them mean something different held than tapped, and only one can be
/// held at a time - so this says which, rather than each having a hold of its
/// own to keep in step.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// Held to silence this one output, tapped to open its soundtracks. The
    /// gesture stayed with the icon that belongs to one output when the levels
    /// moved out into a menu of their own, so silencing one of two people's
    /// audio is still a press and a wait rather than a trip into a menu.
    Output(usize),
    /// Held to silence everything at once, tapped to open the levels. The same
    /// thing the keyboard's `M` does, given to the one control that governs
    /// both outputs - and the first way a pointer has ever had to reach it.
    Main,
    /// Held to show or hide the subtitle already chosen, tapped to open the
    /// list and choose a different one. The deliberate act is the one that
    /// offers a choice; the shortcut is the one that does not look away from
    /// the film.
    Subtitles,
}

/// Told which output changed, where its level now stands as a fraction of
/// full, whether it is silenced, and whether the change is worth keeping.
/// Silencing everything at once is not: it lasts the session, the way the
/// subtitle toggle does, and a film that started silent because of a door
/// knocked on last week would be a bug rather than a memory.
type VolumeHandler = Box<dyn Fn(&str, f64, bool, bool)>;

/// Told that everything has been silenced at once, or let go again. Separate
/// from [`VolumeHandler`] because it belongs to no output and changes none of
/// them: what each output is set to is untouched underneath it.
type HushHandler = Box<dyn Fn(bool)>;

/// Told where the main level now stands, as a fraction of full. Separate from
/// [`VolumeHandler`] because it belongs to no output: what it changes is what
/// every output's own level is a fraction *of*, so the answer is worked out
/// once, above, rather than reported per output from in here.
type MainHandler = Box<dyn Fn(f64)>;

/// Told which output was shifted and by how many milliseconds. Separate from
/// [`VolumeHandler`] because a delay is always worth keeping: unlike silencing
/// everything at once, it describes the equipment rather than the moment.
type SyncHandler = Box<dyn Fn(&str, f64, bool)>;

/// Told which row of the subtitle chooser was picked, counted as the rows were
/// handed over in [`Controls::set_subtitle_entries`]. A position rather than
/// the choice itself, because what a row stands for is the application's to
/// know: the strip is given a list of words and gives back which one.
type SubtitleHandler = Box<dyn Fn(usize)>;

/// Told which output was put onto which row of its soundtrack list, counted as
/// the rows were handed over in [`Controls::set_audio_entries`]. A position
/// rather than the choice itself, for the same reason as [`SubtitleHandler`]:
/// what a row stands for is the application's to know.
type AudioHandler = Box<dyn Fn(&str, usize)>;

/// Wraps a list so it scrolls rather than running off the top of the window.
///
/// `propagate_natural_height` is what makes it size to its contents when they
/// fit, so a list of three does not sit in a tall empty box; the cap that
/// stops it growing past the window is set when the list is opened, since only
/// then is the window's height known. See [`Controls::cap_list_height`].
///
/// No horizontal scrolling: rows are already cut to fit rather than allowed to
/// stretch the panel, so there is never anything off to the side to reach.
fn scroller(list: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .child(list)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_height(true)
        .build()
}

/// Brings a row into view, so that moving onto one that is off the end of a
/// scrolled list actually shows it.
///
/// Without this the cursor moves invisibly: the mark lands on a row nobody can
/// see, and the list looks like it has stopped responding at whatever row
/// happened to be last on screen.
fn reveal_row(scroll: &gtk::ScrolledWindow, row: &impl IsA<gtk::Widget>) {
    let Some(list) = scroll.child() else { return };
    let Some(bounds) = row.as_ref().compute_bounds(&list) else {
        return;
    };
    let adjustment = scroll.vadjustment();
    let (top, bottom) = (bounds.y() as f64, (bounds.y() + bounds.height()) as f64);
    let seen = adjustment.value();
    let page = adjustment.page_size();
    if top < seen {
        adjustment.set_value(top);
    } else if bottom > seen + page {
        adjustment.set_value(bottom - page);
    }
}

/// The icon for a level: staged arcs on the way up, and the barred speaker
/// when there is nothing to hear.
///
/// How loud rather than merely whether, so an output can be read at a glance
/// from across a room - which is the whole reason the strip's icons are drawn
/// this way and not as a single fixed speaker.
fn volume_icon_name(level: f64, muted: bool) -> &'static str {
    if muted || level <= 0.0 {
        "audio-volume-muted-symbolic"
    } else if level < 0.34 {
        "audio-volume-low-symbolic"
    } else if level < 0.67 {
        "audio-volume-medium-symbolic"
    } else {
        "audio-volume-high-symbolic"
    }
}

/// Marks or unmarks a row, which is how both lists and the two controls under
/// them say where the cursor is.
fn mark(widget: &impl IsA<gtk::Widget>, on: bool) {
    if on {
        widget.add_css_class("tp-selected");
    } else {
        widget.remove_css_class("tp-selected");
    }
}

/// Which of an output's two controls is being driven.
///
/// The panel is navigated by this pair rather than by output alone: up and
/// down step through every control of every output in turn, so that grouping
/// the two under a device heading reads the way it behaves.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    Volume,
    Sync,
}

/// One row of the levels menu, in the order they are drawn.
///
/// The menu is navigated by this rather than by an output plus a control,
/// because the main level belongs to no output and would have needed a flag
/// beside them saying to ignore both - which is the shape that has already gone
/// wrong once in this file.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    /// One output's own bar: which output, and which of its two.
    Output(usize, Control),
    /// The main level at the foot, over all of them.
    Main,
}

/// One output's menu: its soundtracks, its name, and its two controls.
struct Output {
    role: &'static str,
    /// This output's own button on the strip, which opens its soundtracks.
    ///
    /// It carries no level any more. One speaker governs the sound now, and a
    /// button that opens a list of soundtracks should not also be reporting how
    /// loud something is.
    button: gtk::Button,
    /// The device's name and this output's two bars, inside the levels menu.
    /// Shown whenever the output is in use, alongside the other one rather
    /// than instead of it.
    group: gtk::Box,
    /// The soundtracks on offer, in the chooser this output's button opens.
    /// Filled per video, since what is on offer belongs to the file rather
    /// than to the strip.
    tracks: gtk::Box,
    track_rows: RefCell<Vec<gtk::Label>>,
    /// Which row the cursor is on, and which row is the soundtrack actually
    /// playing. They part company the moment anybody moves, which is why they
    /// are marked separately - the same as the subtitle chooser.
    track_at: Cell<usize>,
    track_current: Cell<Option<usize>>,
    /// Whether this output is in use at all. Distinct from whether its menu
    /// is open: an output set to None has a button that does nothing, and
    /// hiding the group for the one reason must not be confused with hiding
    /// it for the other.
    in_use: Cell<bool>,
    row: gtk::Box,
    mute: gtk::Button,
    level: gtk::Scale,
    level_reading: gtk::Label,
    /// The row and slider for how far this output is shifted in time, with
    /// the button that turns the shift on and off.
    sync_row: gtk::Box,
    sync: gtk::Scale,
    sync_reading: gtk::Label,
    sync_toggle: gtk::Button,
    /// Whether the shift is being applied. Off keeps the slider where it is:
    /// the value is what somebody found by ear, and losing it to hear the
    /// difference for a moment would be the opposite of useful.
    sync_on: Cell<bool>,
    /// Whether this output is silenced in its own right, which is the only
    /// thing that ever changes it. Silencing everything at once lies over the
    /// top rather than writing here - see [`Controls::toggle_hush`].
    muted: Cell<bool>,
}

impl Output {
    /// The focusable row for one of this output's controls.
    fn row_for(&self, control: Control) -> &gtk::Box {
        match control {
            Control::Volume => &self.row,
            Control::Sync => &self.sync_row,
        }
    }
}

/// How long the strip stays up after the last input. Long enough to read a
/// timestamp after a seek, short enough not to sit over the picture.
const LINGER: Duration = Duration::from_secs(3);

/// The same, while a controller is holding one of the rows. Someone moving
/// through the buttons needs longer than someone who just glanced at the
/// clock, but it still goes away on its own: a strip that stayed up forever
/// because a button was highlighted would be worse than one that hides.
const LINGER_HELD: Duration = Duration::from_secs(12);

/// How far a pointer has to travel before it counts as having moved, in
/// logical pixels. Small enough that reaching for a control registers at once,
/// large enough to ignore the drift a still pointer reports.
const MOVEMENT: f64 = 4.0;

/// Play's place in the button order, which is where a controller starts every
/// time it takes hold of the row.
const PLAY: usize = 3;

/// Where the first output's button sits in the same order. One button follows
/// per output, and fullscreen comes after them all. Each output's menu keeps
/// its own button highlighted while it is open, so it is clear where closing
/// the menu goes back to.
const FIRST_OUTPUT: usize = 6;

/// Subtitles' place in the same order, and for the same reason: the chooser
/// keeps it highlighted while it is open.
const SUBTITLES: usize = 5;

/// How long an output's button has to be held for it to mean "silence this
/// output" rather than "show me its menu". Long enough not to fire under an
/// ordinary press, short enough to be a deliberate gesture rather than a wait.
pub const HOLD: Duration = Duration::from_millis(600);

/// How much of the window the key list may take before it scrolls inside
/// itself. A ceiling only: a list shorter than this is drawn at its own
/// height, so the panel is the size of what is in it.
const SHORTCUTS_SHARE: f64 = 0.9;

/// How far one press moves a level. Twenty steps across the range: coarse
/// enough to cross it in a second of held input, fine enough to settle on a
/// level rather than overshoot it.
const VOLUME_STEP: f64 = 0.05;

/// How wide the panel is, before scaling. A minimum rather than a fixed
/// width, but with the device names now cut to fit rather than stretching it,
/// it is what the panel actually comes out at.
const PANEL_WIDTH: f64 = 420.0;

/// The least the subtitle chooser is ever drawn at, before scaling. A minimum
/// only: it grows to whatever the longest language name needs, unlike the
/// volume panel, which is a fixed width because the device names in it run
/// long enough to be cut. Without a floor a list of "Off" and "en" would come
/// out a box barely wider than a word.
const SUBTITLE_WIDTH: f64 = 260.0;

/// How wide a row is allowed to grow, in characters, before it is cut instead.
/// A language name is short; a subtitle file somebody picked by hand is not,
/// and one of those would otherwise take the panel across the picture.
const SUBTITLE_CHARS: i32 = 32;

/// The least a scrolled list is ever drawn at, before scaling. A window too
/// short to hold a list properly should still show a row or two and scroll,
/// rather than collapsing to nothing and reading as a list that failed to
/// appear.
const MIN_LIST_HEIGHT: f64 = 120.0;

/// Between a row's button, its bar and its reading. The same for both rows,
/// because different spacing gives the two bars different lengths and they
/// sit one above the other.
const ROW_SPACING: i32 = 12;

/// How far one press shifts an output, in milliseconds. The same step the
/// settings slider uses, which is about the smallest change that can be told
/// apart against a picture.
const SYNC_STEP: f64 = 10.0;

/// The reading beside a bar in the panel.
///
/// One width for both rows, so the bars above and below each other start and
/// end in the same place, and the same width the settings sliders use.
fn reading_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("tp-hint");
    label.set_xalign(1.0);
    label.set_width_chars(crate::app::READING_CHARS);
    label
}

/// One line of the subtitle chooser.
///
/// A label rather than the box the volume panel's rows are, because that is
/// all a row here holds: there is no button and no bar beside it, only the
/// name of a subtitle. Focusable all the same, so the mark the strip draws has
/// something for a screen reader to be pointed at.
fn subtitle_row(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("tp-subtitle-row");
    label.set_xalign(0.0);
    label.set_max_width_chars(SUBTITLE_CHARS);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_focusable(true);
    label
}

/// Gives a control a name for anyone who cannot see the picture on it.
///
/// Every button in the strip is an icon and nothing else, which a screen
/// reader announces as "button" and no more. The name is set here rather than
/// as a tooltip: a tooltip is for a pointer hovering, and this interface is
/// built to be driven without one.
fn name_it(widget: &impl IsA<gtk::Accessible>, name: &str) {
    widget.update_property(&[gtk::accessible::Property::Label(name)]);
}

pub struct Controls {
    root: gtk::Overlay,
    strip: gtk::Revealer,
    /// The bar the whole strip sits in.
    ///
    /// It carries the `active-descendant` relation, and it takes the focus
    /// itself for the timeline row, where the scale must not have it. Renamed
    /// as that happens, so arriving there is announced as "Playback position"
    /// rather than as the strip in general.
    holder: gtk::Box,
    icon: gtk::Image,
    play: gtk::Button,
    stop: gtk::Button,
    skip_back: gtk::Button,
    skip_forward: gtk::Button,
    /// Leaves playback for the page the film was started from. Present
    /// whatever launched us; [`Self::stop`] beside it is not.
    back: gtk::Button,
    elapsed: gtk::Label,
    duration: gtk::Label,
    /// Whether the right-hand readout counts down instead of naming the
    /// length. Starts off for every video: how long something is is the
    /// question you have before you start, and how much is left is the one
    /// you ask part way through.
    remaining: Rc<Cell<bool>>,
    position: gtk::Scale,
    /// Insensitive until something tells it a subtitle track exists, since
    /// most files reach playback with none selected.
    subtitles: gtk::Button,
    fullscreen: gtk::Button,
    /// Set while the readout is being written, so the scale's own change
    /// signal is not mistaken for someone dragging it.
    updating: Cell<bool>,
    /// Bumped every time the strip is shown. A pending hide captures the
    /// value it was scheduled under and does nothing if it no longer matches,
    /// which is what stops repeated seeks from hiding the strip three seconds
    /// after the *first* one.
    ///
    /// Preferred over canceling the timer by id: a source that has already
    /// fired cannot be removed, and trying logs a GLib critical.
    generation: Rc<Cell<u64>>,
    /// The buttons in the order they are drawn, which is the order a
    /// controller moves through them.
    order: Vec<gtk::Button>,
    /// The button row, held back on its own when the strip is only showing
    /// where playback has reached.
    buttons: gtk::Revealer,
    /// Which of them is highlighted, when the button row is being driven.
    ///
    /// Still kept here rather than read back from GTK, because stepping over
    /// an insensitive button is far easier against an index than against the
    /// focus chain. The focus is moved to match, which is what a screen reader
    /// follows, but this remains the thing that decides where it goes.
    focused: Cell<usize>,
    row: Cell<Row>,
    /// Told about every level and mute change, whichever way it was made.
    on_volume: RefCell<Option<VolumeHandler>>,
    /// The same for a change to how far an output is shifted in time.
    on_sync: RefCell<Option<SyncHandler>>,
    /// Told which row of the chooser was picked.
    on_subtitle: RefCell<Option<SubtitleHandler>>,
    /// Told which output was put onto which of its soundtrack rows.
    on_audio: RefCell<Option<AudioHandler>>,
    /// Told to show or hide whatever is already chosen, which is what holding
    /// the subtitle button means. Kept as a handler rather than wired to the
    /// button's click, because the click now opens the list instead.
    on_toggle_subtitles: RefCell<Option<Box<dyn Fn()>>>,
    /// The subtitle chooser, the list of rows inside it, and where in that
    /// list the strip is.
    subtitle_panel: gtk::Revealer,
    subtitle_list: gtk::Box,
    /// What that list scrolls inside, for a film with more subtitles than fit.
    subtitle_scroll: gtk::ScrolledWindow,
    /// Rebuilt for every video, since what is on offer is a property of the
    /// file rather than of the strip.
    subtitle_rows: RefCell<Vec<gtk::Label>>,
    subtitle_at: Cell<usize>,
    /// Which row is the subtitle in force, marked apart from where the cursor
    /// is: the two part company the moment anybody moves, which is the whole
    /// reason for marking them separately. `None` while the list is empty.
    subtitle_current: Cell<Option<usize>>,
    /// The levels menu.
    panel: gtk::Revealer,
    /// The soundtrack chooser, and the one scroller every output's list opens
    /// inside. Which list that is, is `output`.
    audio_panel: gtk::Revealer,
    audio_scroll: gtk::ScrolledWindow,
    outputs: Vec<Output>,
    /// Which output's soundtracks are open, which is the only thing the strip
    /// still keeps an output index for: the levels menu shows them all at once.
    output: Cell<usize>,
    /// Where the cursor is in the levels menu, counted over every row of it in
    /// turn - each output's level and sync, then the main level last.
    ///
    /// One number rather than an output plus a control plus a flag saying which
    /// of the two it means. That arrangement is what put a stale answer behind
    /// `control`, so that Enter on a soundtrack muted the output; there is
    /// nothing here to be stale against.
    level_at: Cell<usize>,
    /// The key list over the picture, and what scrolls it. Shown by F1 and by
    /// the pad's Select, and dismissed by anything that means "back".
    shortcuts: gtk::Revealer,
    shortcuts_scroll: gtk::ScrolledWindow,
    /// The row at the foot of the menu that governs both outputs, and where its
    /// level stands. The mute beside it is the blanket silence below, which
    /// this row is the first way a pointer has had of reaching.
    main_row: gtk::Box,
    main_mute: gtk::Button,
    main: gtk::Scale,
    main_reading: gtk::Label,
    /// Told when the main level moves, so every output can be pushed again at
    /// the level it now works out to.
    on_main: RefCell<Option<MainHandler>>,
    /// Told when everything is silenced at once, or let go.
    on_hush: RefCell<Option<HushHandler>>,
    /// The speaker on the strip and the mark inside it, which stages with what
    /// is actually being heard rather than with the main level's own value.
    volume_button: gtk::Button,
    volume_icon: gtk::Image,
    /// Whether everything is silenced at once. Held here rather than in the
    /// configuration on purpose - see [`VolumeHandler`].
    hushed: Cell<bool>,
    /// Bumped to cancel a hold that has not fired yet, the same way the hide
    /// timer is canceled.
    hold: Cell<u64>,
    /// Whether the button is being held at all, so that a keyboard repeating
    /// its press does not restart the hold and stop it ever finishing.
    holding: Cell<bool>,
    /// Whether the hold did something, so that letting go does not then do
    /// the ordinary thing on top of it.
    held: Cell<bool>,
    /// Set when a pointer's hold has already acted, so the click that follows
    /// the release is ignored.
    swallow_click: Cell<bool>,
    /// Whether the selected output is marked. A pointer needs no mark - it
    /// points at what it is about to change - and marking one under a mouse
    /// user says something is held when nothing is. It comes on the moment a
    /// direction is pressed.
    selected: Cell<bool>,
    /// Kept so the fullscreen mark can be redrawn when the state changes.
    ///
    /// The theme is deliberately not kept beside it. Every mark on this strip
    /// is chosen for the strip's own near-black background rather than for the
    /// window's theme, so there is nothing here for a theme to decide.
    scale: f64,
    fullscreen_state: RefCell<bool>,
}

impl Controls {
    pub fn new(
        video: &gtk::Picture,
        scale: f64,
        fullscreen_now: bool,
        lock_fullscreen: bool,
        outputs: &[(&'static str, String)],
        // Whether something else chose this video and is waiting for the
        // playback, which is the only case where Stop means anything Back
        // does not.
        external: bool,
    ) -> Rc<Self> {
        // Pause, because playback begins playing. The readout corrects this
        // on its first tick anyway, but half a second of the wrong icon is
        // half a second of it looking stopped.
        let icon = gtk::Image::from_icon_name("media-playback-pause-symbolic");
        icon.add_css_class("tp-transport");
        icon.add_css_class("tp-transport-main");
        let play = gtk::Button::new();
        play.set_child(Some(&icon));
        play.add_css_class("tp-transport-button");
        name_it(&play, &tr!("Play or pause"));

        // go-* rather than media-seek-*: the seek glyphs are absent from the
        // GTK that ships with GStreamer on Windows, and a missing icon draws
        // as a broken-image box. These are plain arrows, which is less
        // expressive than a skip glyph but present everywhere.
        let back_icon = gtk::Image::from_icon_name("go-previous-symbolic");
        back_icon.add_css_class("tp-transport");
        let skip_back = gtk::Button::new();
        skip_back.set_child(Some(&back_icon));
        skip_back.add_css_class("tp-transport-button");
        name_it(&skip_back, &tr!("Skip back"));

        let forward_icon = gtk::Image::from_icon_name("go-next-symbolic");
        forward_icon.add_css_class("tp-transport");
        let skip_forward = gtk::Button::new();
        skip_forward.set_child(Some(&forward_icon));
        skip_forward.add_css_class("tp-transport-button");
        name_it(&skip_forward, &tr!("Skip forward"));

        // Beside play, because they are the same kind of thing: what playback
        // is doing right now.
        let stop_icon = gtk::Image::from_icon_name("media-playback-stop-symbolic");
        stop_icon.add_css_class("tp-transport");
        let stop = gtk::Button::new();
        stop.set_child(Some(&stop_icon));
        stop.add_css_class("tp-transport-button");
        name_it(&stop, "Stop");

        let elapsed = gtk::Label::new(Some("0:00"));
        elapsed.add_css_class("tp-time");
        let duration = gtk::Label::new(Some("0:00"));
        duration.add_css_class("tp-time");
        // Clicking it swaps between the length and what is left. The readout
        // refreshes on the next tick a tenth of a second later, which is
        // faster than the change can be seen.
        let remaining = Rc::new(Cell::new(false));
        {
            let remaining = remaining.clone();
            let gesture = gtk::GestureClick::new();
            gesture.connect_released(move |_, _, _, _| {
                remaining.set(!remaining.get());
            });
            duration.add_controller(gesture);
        }

        // A scale rather than a progress bar: with its value hidden it looks
        // much the same, and it can be clicked and dragged to seek.
        let position = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.001);
        position.set_draw_value(false);
        position.set_hexpand(true);
        position.set_can_focus(false);
        position.add_css_class("tp-progress");
        name_it(&position, "Position");

        let fullscreen = gtk::Button::new();
        fullscreen.set_child(Some(&crate::app::fullscreen_image(fullscreen_now, scale)));
        fullscreen.add_css_class("tp-transport-button");
        name_it(&fullscreen, &tr!("Toggle fullscreen"));
        // Hidden rather than dimmed when fullscreen is fixed for this run:
        // there is nothing to be waiting for, so nothing to grey out.
        fullscreen.set_visible(!lock_fullscreen);

        // A bundled image rather than a themed icon name: no subtitle glyph
        // ships with GTK on Windows, and a missing icon draws as a
        // broken-image box.
        let subtitles = gtk::Button::new();
        subtitles.set_child(Some(&crate::app::subtitles_image(scale)));
        subtitles.add_css_class("tp-transport-button");
        subtitles.add_css_class("tp-subtitles-button");
        name_it(&subtitles, &tr!("Show or hide subtitles"));
        subtitles.set_sensitive(false);

        // One speaker for the sound, where there used to be one per output.
        //
        // Two of them had to be numbered, because side by side two speakers say
        // which is louder rather than which is which. With one there is nothing
        // to tell apart: it means the sound, all of it, and the numbers move on
        // to the soundtrack icons where they answer a real question.
        let volume_icon = gtk::Image::from_icon_name("audio-volume-high-symbolic");
        volume_icon.add_css_class("tp-transport");
        let volume_button = gtk::Button::new();
        volume_button.set_child(Some(&volume_icon));
        volume_button.add_css_class("tp-transport-button");
        name_it(&volume_button, "Volume");

        // No spacing of its own. Each group already pads itself and every join
        // between two of them carries a divider with margins either side, so a
        // gap here was a fourth helping stacked on top of those three - which
        // is what left each block floating well clear of the rule below it.
        let panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(0)
            .build();
        panel.add_css_class("tp-volume-panel");
        let mut rows = Vec::new();
        for (index, (role, name)) in outputs.iter().enumerate() {
            let (role, name) = (*role, name);
            // One button per output rather than one for both. Which output a
            // press is about is otherwise unanswerable without opening
            // something first, and on a television that is a press spent
            // finding out where you are.
            // A bundled image rather than a themed icon name, the same as the
            // subtitle mark and for the same reason: nothing in the icon theme
            // means "soundtrack", and GStreamer's Windows bundle ships no icon
            // theme at all, where a missing icon draws as a broken-image box.
            let icon = crate::app::soundtrack_image(scale);
            icon.add_css_class("tp-transport");
            // Numbered, which is the whole reason a badge is wanted here rather
            // than on the speaker beside it: two soundtrack icons side by side
            // say nothing about which output each belongs to. The speaker needs
            // no number, because there is one of it and it governs both.
            //
            // Over the icon rather than beside it: the row of buttons is evenly
            // spaced, and a pair that were wider than the rest would pull the
            // whole row off center.
            let badge = gtk::Label::new(Some(&(index + 1).to_string()));
            badge.add_css_class("tp-output-badge");
            badge.set_halign(gtk::Align::Start);
            badge.set_valign(gtk::Align::Start);
            let stack = gtk::Overlay::new();
            stack.set_child(Some(&icon));
            stack.add_overlay(&badge);
            let button = gtk::Button::new();
            button.set_child(Some(&stack));
            button.add_css_class("tp-transport-button");
            // The number is in the name too. A screen reader gets "1" as a
            // label over an icon it cannot describe otherwise, and "Soundtrack"
            // twice over would name neither.
            name_it(
                &button,
                &format!("Soundtrack, output {}, {name}", index + 1),
            );

            // Above the controls, and the same list the subtitle chooser uses,
            // because it is the same act: a list of choices read from a sofa
            // while a film runs.
            let tracks = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .build();
            // Cut rather than allowed to stretch the panel. Device names run
            // long - "Headphones (2- Arctis Nova Pro Wireless)" - and a panel
            // sized to the longest one would be most of the screen.
            let label = gtk::Label::builder()
                .label(name)
                .halign(gtk::Align::Start)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["tp-hint"])
                .build();

            let mute = gtk::Button::from_icon_name("audio-volume-high-symbolic");
            mute.add_css_class("tp-transport-button");
            mute.set_can_focus(false);
            // Named by device, since which output is being silenced is the
            // whole question in a player with two of them.
            name_it(&mute, &format!("Mute {name}"));

            let level = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
            level.set_draw_value(false);
            level.set_hexpand(true);
            level.set_can_focus(false);
            level.add_css_class("tp-progress");
            name_it(&level, &format!("Volume, {name}"));

            let level_reading = reading_label(&crate::app::volume_label(1.0, false));

            // The device names itself once, above its controls, rather than
            // each control repeating it. The heading is not focusable, so it
            // is never stepped onto and never announced on its own - which is
            // why each row below carries the device in its own name.
            let row = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(ROW_SPACING)
                .build();
            row.set_focusable(true);
            name_it(&row, &format!("Volume, {name}"));
            row.append(&mute);
            row.append(&level);
            row.append(&level_reading);

            let sync = gtk::Scale::with_range(
                gtk::Orientation::Horizontal,
                -crate::config::MAX_OFFSET_MS,
                crate::config::MAX_OFFSET_MS,
                1.0,
            );
            sync.set_draw_value(false);
            sync.set_hexpand(true);
            sync.set_can_focus(false);
            sync.add_css_class("tp-progress");
            name_it(&sync, &tr!("Audio sync, {name}", name = name));

            // The same shape as the row above: a button the width of the
            // mute one, then the bar. Pressing it puts the output back in
            // sync, which is the one value somebody is ever certain they
            // want and the one that stepping in tens cannot reach from an
            // arbitrary place a pointer left it in.
            let sync_toggle = gtk::Button::new();
            sync_toggle.set_child(Some(&crate::app::sync_image(scale)));
            sync_toggle.add_css_class("tp-transport-button");
            sync_toggle.set_can_focus(false);
            name_it(&sync_toggle, &tr!("Use audio sync, {name}", name = name));

            let sync_reading = reading_label(&crate::app::offset_label(0.0));

            // The same spacing as the row above, not just the same widgets:
            // the two bars sit one under the other, and gaps of different
            // sizes either side leave them visibly different lengths.
            let sync_row = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(ROW_SPACING)
                .build();
            sync_row.set_focusable(true);
            name_it(&sync_row, &tr!("Audio sync, {name}", name = name));
            sync_row.append(&sync_toggle);
            sync_row.append(&sync);
            sync_row.append(&sync_reading);

            // The device names itself, then its two bars. The soundtracks are
            // not here any more: they are a choice about the film, where this
            // menu is about how loud things are and when.
            let group = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(4)
                .css_classes(["tp-menu-group"])
                .build();
            // Above every group but the first, so two outputs read as two
            // blocks rather than one long column of bars. Inside the group, so
            // that hiding an output takes its divider with it and never leaves
            // a rule with nothing under it.
            if index > 0 {
                let divider = gtk::Separator::new(gtk::Orientation::Horizontal);
                divider.add_css_class("tp-menu-divider");
                group.append(&divider);
            }
            group.append(&label);
            group.append(&row);
            group.append(&sync_row);
            // Shown or hidden by whether the output is in use, which playback
            // says through `set_levels`. Both are shown at once when both are
            // in use: this is one menu about the sound, not a menu per output.
            group.set_visible(false);
            panel.append(&group);

            rows.push(Output {
                role,
                button,
                group,
                tracks,
                track_rows: RefCell::new(Vec::new()),
                track_at: Cell::new(0),
                track_current: Cell::new(None),
                in_use: Cell::new(index == 0),
                row,
                mute,
                level,
                level_reading,
                sync_row,
                sync,
                sync_reading,
                sync_toggle,
                sync_on: Cell::new(false),
                muted: Cell::new(false),
            });
        }

        // The main level, at the foot of the menu and outside every group.
        //
        // At the foot because the menu rises out of the bar, so the last row is
        // the one nearest the button and the first a press upward reaches. At
        // the foot *outside* the groups because it governs both of them: built
        // into one it would be drawn twice, and read as belonging to whichever
        // output it was sitting under.
        //
        // Always drawn, even with one output in use. It goes on multiplying
        // whether it is on screen or not, so a main level left at three
        // quarters with nothing to say so would make every other number in this
        // menu a lie - and an output can be turned off mid-film, which would
        // take the control away while its value carried on applying.
        // Built exactly as an output's group is - the same box, the same
        // spacing, the divider inside it - so the row lines up with the ones
        // above rather than merely resembling them. The panel pads its own
        // direct children, so a row appended straight to it came out inset from
        // every other bar in the menu. Reported by Scott, 2026-08-14.
        let main_group = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .css_classes(["tp-menu-group", "tp-menu-foot"])
            .build();
        let main_divider = gtk::Separator::new(gtk::Orientation::Horizontal);
        main_divider.add_css_class("tp-menu-divider");
        main_group.append(&main_divider);

        // Named where a device names itself in the groups above, and drawn the
        // same way down to the ellipsis it will never need, which is what says
        // this row is the same kind of thing at a different scope rather than a
        // third output.
        let main_label = gtk::Label::builder()
            .label(tr!("All Outputs").as_ref())
            .halign(gtk::Align::Start)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["tp-hint"])
            .build();

        let main_mute = gtk::Button::from_icon_name("audio-volume-high-symbolic");
        main_mute.add_css_class("tp-transport-button");
        main_mute.set_can_focus(false);
        name_it(&main_mute, &tr!("Silence all outputs"));

        let main = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
        main.set_draw_value(false);
        main.set_hexpand(true);
        main.set_can_focus(false);
        main.add_css_class("tp-progress");
        name_it(&main, &tr!("Volume, all outputs"));
        main.set_value(1.0);

        let main_reading = reading_label(&crate::app::volume_label(1.0, false));

        let main_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(ROW_SPACING)
            .build();
        main_row.set_focusable(true);
        name_it(&main_row, &tr!("Volume, all outputs"));
        main_row.append(&main_mute);
        main_row.append(&main);
        main_row.append(&main_reading);
        main_group.append(&main_label);
        main_group.append(&main_row);
        panel.append(&main_group);

        // Part of the strip rather than a popover hung off the button. A GTK 4
        // popover is its own surface, constrained to the monitor and not to
        // the window - GTK 3's window constraint is gone - so in a window it
        // hangs off the edge of the frame. Built into the strip it cannot.
        panel.set_size_request((PANEL_WIDTH * scale) as i32, -1);
        panel.set_halign(gtk::Align::End);
        let panel_reveal = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .transition_duration(150)
            .child(&panel)
            .halign(gtk::Align::End)
            .build();

        // Whichever output's soundtracks are open, in one scroller rather than
        // one each. Only one list can be open at a time, so a second scroller
        // would be a second thing to size, cap and scroll in step with the
        // first - and the panel it sits in is the subtitle chooser's, because
        // choosing a soundtrack and choosing a subtitle are the same act.
        let audio_stack = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        audio_stack.set_size_request((SUBTITLE_WIDTH * scale) as i32, -1);
        for output in &rows {
            output.tracks.set_visible(false);
            audio_stack.append(&output.tracks);
        }
        let audio_scroll = scroller(&audio_stack);
        audio_scroll.add_css_class("tp-subtitle-panel");
        audio_scroll.set_halign(gtk::Align::End);
        let audio_reveal = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .transition_duration(150)
            .child(&audio_scroll)
            .halign(gtk::Align::End)
            .build();

        // Built empty and filled per video, out of the same corner and with
        // the same background as the volume panel: they are the two things
        // the strip opens rather than does, and a list of subtitles that
        // arrived somewhere else would read as a different kind of thing.
        //
        // Sized to its contents above a floor, where the volume panel is a
        // fixed width - see `SUBTITLE_WIDTH`.
        let subtitle_list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        subtitle_list.set_size_request((SUBTITLE_WIDTH * scale) as i32, -1);
        // A film can carry more subtitles than fit above the bar - the sample
        // one has thirteen - and without this the list simply ran off the top
        // of the window with no way to reach what was up there.
        //
        // The panel's own look moves out here with it. Left on the list it
        // would be inside the scroller: the background would scroll with the
        // rows, and the margins that hold the panel off the edge of the screen
        // would be measured from the wrong box.
        let subtitle_scroll = scroller(&subtitle_list);
        subtitle_scroll.add_css_class("tp-subtitle-panel");
        subtitle_scroll.set_halign(gtk::Align::End);
        let subtitle_reveal = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .transition_duration(150)
            .child(&subtitle_scroll)
            .halign(gtk::Align::End)
            .build();

        // Away from the transport controls, being the one thing here that is
        // not about what playback is doing: it leaves playback rather than
        // changing it, landing back on the page the film was started from.
        //
        // It wore the gear until 2026-08-17, which promised the settings
        // screen and went somewhere else. The same mark as every other Back
        // button in the application, so that one picture means one thing.
        let back_mark = crate::app::back_image(crate::app::ICON_PX * scale);
        back_mark.add_css_class("tp-transport");
        let back = gtk::Button::new();
        back.set_child(Some(&back_mark));
        back.add_css_class("tp-transport-button");
        name_it(&back, "Back");

        // Two rows: where playback is, and what can be done to it. Separating
        // them is what lets a controller treat them differently - left and
        // right seek along the top row and move between buttons on the bottom
        // one, without the two meanings ever colliding.
        let timeline = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(16)
            .build();
        timeline.add_css_class("tp-timeline");
        timeline.append(&elapsed);
        timeline.append(&position);
        timeline.append(&duration);

        // Play sits in the middle and larger than the rest, because it is the
        // one control anybody reaches for and the one a controller lands on
        // first. What is left goes to the edges: leaving playback on the left,
        // and on the right the things that change how the video reaches you.
        let left = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(16)
            .build();
        left.append(&back);
        // Only under a launcher, where it is the one button here that does
        // something Back does not: there is no page of ours to return to, so
        // it finishes the playback the launcher is waiting on and closes the
        // window. Everywhere else the two ran the same code, which is two
        // buttons offering one thing and no way to tell which was which.
        if external {
            left.append(&stop);
        }

        // Skipping either side of play, which balances the group and gives a
        // pointer a way to skip at all: until now that was keyboard and
        // gamepad only.
        let middle = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(16)
            .build();
        middle.append(&skip_back);
        middle.append(&play);
        middle.append(&skip_forward);

        let buttons = gtk::CenterBox::new();
        buttons.add_css_class("tp-buttons");
        buttons.set_start_widget(Some(&left));
        buttons.set_center_widget(Some(&middle));
        // Subtitles keep company with volume and fullscreen: they are all
        // about how the video reaches you, where the left-hand pair is about
        // leaving playback or stopping it.
        let right = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(16)
            .build();
        right.append(&subtitles);
        // The three choosers together, then the sound, then the picture. They
        // are grouped by what a press gets you rather than by which output it
        // belongs to: a list to choose from, a menu of levels, a full screen.
        for output in &rows {
            right.append(&output.button);
        }
        right.append(&volume_button);
        right.append(&fullscreen);
        buttons.set_end_widget(Some(&right));

        // Its own revealer, so the buttons slide in and out the way the strip
        // itself does. Toggling visibility made them appear in one frame,
        // which reads as a glitch beside everything else that animates.
        let button_row = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .transition_duration(150)
            .child(&buttons)
            .build();

        // Scaled like everything else: a gap that reads as deliberate on a
        // monitor is a hairline on a television across a room.
        let bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing((12.0 * scale) as i32)
            .build();
        bar.add_css_class("tp-controls");
        bar.append(&timeline);
        bar.append(&button_row);

        // The panel sits above the bar rather than inside it, so opening it
        // does not extend the bar's black band the full width of the screen
        // for the sake of a panel in one corner. It carries its own
        // background, over on the button's side, and rises out of the same
        // corner it was opened from.
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .accessible_role(gtk::AccessibleRole::Group)
            .build();
        // Both panels sit above the bar, and only ever one of them is open, so
        // the order between them decides nothing.
        row.append(&subtitle_reveal);
        row.append(&audio_reveal);
        row.append(&panel_reveal);
        row.append(&bar);
        // Takes the focus for everything inside it. Nothing else in the strip
        // can hold it, which is what makes this the one place to put it.
        row.set_focusable(true);
        name_it(&row, &tr!("Playback controls"));

        // Slides up rather than appearing, which reads as deliberate at a
        // distance where a sudden change is just a flicker.
        let strip = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .transition_duration(150)
            .valign(gtk::Align::End)
            .child(&row)
            .build();

        button_row.set_reveal_child(true);

        // Space stays pause, even with a transport button holding the focus.
        //
        // The window's own key handling runs in the bubble phase, so a focused
        // GtkButton would activate itself on Space long before that handler
        // saw the key - pressing Space while the strip was up would Stop the
        // film, or toggle subtitles, depending on where you happened to be.
        // Capture on the bar runs ahead of the button, and pressing play is
        // the same thing the window handler would have done.
        //
        // Enter is deliberately left alone: activating the highlighted control
        // is exactly what it should do, and the button already does it.
        {
            let play_button = play.clone();
            let controller = gtk::EventControllerKey::new();
            controller.set_propagation_phase(gtk::PropagationPhase::Capture);
            controller.connect_key_pressed(move |_, key, _, _| match key {
                gdk::Key::space => {
                    play_button.emit_clicked();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            });
            row.add_controller(controller);
        }

        // The key list, over the picture rather than in place of it. Every
        // other full page in the application replaces the window's child,
        // which during playback would take the video widget down with it -
        // so this one is an overlay, like the strip it explains.
        let shortcuts_page = crate::shortcuts::page(scale);
        shortcuts_page.add_css_class("tp-shortcuts");
        // Held to its own height inside the scroller's viewport, which hands a
        // child the whole viewport when it is willing to fill one - and this
        // child carries the panel's background, so filling it is visible as a
        // panel taller than what is in it.
        shortcuts_page.set_valign(gtk::Align::Start);
        let shortcuts_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .propagate_natural_width(true)
            .vexpand(false)
            .valign(gtk::Align::Center)
            .child(&shortcuts_page)
            .build();
        shortcuts_scroll.set_focusable(false);
        let shortcuts = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .reveal_child(false)
            .child(&shortcuts_scroll)
            .build();

        let root = gtk::Overlay::new();
        root.set_child(Some(video));
        root.add_overlay(&strip);
        root.add_overlay(&shortcuts);

        // The order a controller steps through, which has to match the order
        // they are drawn in above - including Stop only where it was drawn,
        // and one button per output, however many there are.
        let mut order = vec![back.clone()];
        if external {
            order.push(stop.clone());
        }
        order.extend([
            skip_back.clone(),
            play.clone(),
            skip_forward.clone(),
            subtitles.clone(),
        ]);
        order.extend(rows.iter().map(|output| output.button.clone()));
        order.push(volume_button.clone());
        order.push(fullscreen.clone());

        let controls = Rc::new(Self {
            root,
            shortcuts,
            shortcuts_scroll,
            strip,
            holder: row.clone(),
            buttons: button_row.clone(),
            icon,
            play,
            stop,
            skip_back,
            skip_forward,
            back,
            elapsed,
            duration,
            remaining,
            position,
            subtitles,
            fullscreen,
            updating: Cell::new(false),
            generation: Rc::new(Cell::new(0)),
            order,
            focused: Cell::new(PLAY),
            row: Cell::new(Row::None),
            on_volume: RefCell::new(None),
            on_sync: RefCell::new(None),
            on_subtitle: RefCell::new(None),
            on_audio: RefCell::new(None),
            on_toggle_subtitles: RefCell::new(None),
            subtitle_panel: subtitle_reveal,
            subtitle_list,
            subtitle_scroll,
            subtitle_rows: RefCell::new(Vec::new()),
            subtitle_at: Cell::new(0),
            subtitle_current: Cell::new(None),
            panel: panel_reveal,
            audio_panel: audio_reveal,
            audio_scroll,
            outputs: rows,
            output: Cell::new(0),
            level_at: Cell::new(0),
            main_row,
            main_mute,
            main,
            main_reading,
            on_main: RefCell::new(None),
            on_hush: RefCell::new(None),
            volume_button,
            volume_icon,
            hushed: Cell::new(false),
            hold: Cell::new(0),
            holding: Cell::new(false),
            held: Cell::new(false),
            swallow_click: Cell::new(false),
            selected: Cell::new(false),
            scale,
            fullscreen_state: RefCell::new(fullscreen_now),
        });

        // The button opens the panel rather than the panel following the
        // pointer around: hovering is invisible to a controller, and this is
        // the one control that has to work the same both ways.
        for index in 0..controls.outputs.len() {
            let button = controls.outputs[index].button.clone();
            let handle = controls.clone();
            button.connect_clicked(move |_| {
                if handle.swallow_click.replace(false) {
                    return;
                }
                if handle.row.get() == Row::Audio && handle.output.get() == index {
                    handle.set_row(Row::Buttons);
                } else {
                    handle.open_audio(index);
                }
            });

            // A legacy controller rather than a gesture: a button claims the
            // button sequence for its own click, and a gesture added to it
            // gets a cancel where the release should be. This sees the events
            // on the way past regardless.
            let controller = gtk::EventControllerLegacy::new();
            controller.set_propagation_phase(gtk::PropagationPhase::Capture);
            let handle = controls.clone();
            controller.connect_event(move |_, event| {
                match event.event_type() {
                    gdk::EventType::ButtonPress => handle.press_hold(Hold::Output(index)),
                    gdk::EventType::ButtonRelease if !handle.release_hold() => {
                        handle.swallow_click.set(true);
                    }
                    _ => {}
                }
                glib::Propagation::Proceed
            });
            controls.outputs[index].button.add_controller(controller);
        }

        // The same arrangement, for the same reasons: the list is opened by
        // the button rather than by hovering, and the press is watched on its
        // way past so that holding it can mean the other thing.
        //
        // Which is the whole of the new behavior. Tapping the icon opens the
        // chooser and holding it shows or hides what is already chosen; the
        // keyboard's C and the gamepad's left face button are unchanged, and
        // still toggle without offering a choice.
        {
            // Off the built strip rather than the local, which has already
            // been moved into it - unlike the volume button, which is only
            // ever borrowed for the button order.
            let subtitles_button = controls.subtitles.clone();
            let handle = controls.clone();
            subtitles_button.connect_clicked(move |_| {
                if handle.swallow_click.replace(false) {
                    return;
                }
                if handle.row.get() == Row::Subtitles {
                    handle.set_row(Row::Buttons);
                } else {
                    handle.open_subtitles();
                }
            });

            let controller = gtk::EventControllerLegacy::new();
            controller.set_propagation_phase(gtk::PropagationPhase::Capture);
            let handle = controls.clone();
            controller.connect_event(move |_, event| {
                match event.event_type() {
                    gdk::EventType::ButtonPress => handle.press_hold(Hold::Subtitles),
                    gdk::EventType::ButtonRelease if !handle.release_hold() => {
                        handle.swallow_click.set(true);
                    }
                    _ => {}
                }
                glib::Propagation::Proceed
            });
            subtitles_button.add_controller(controller);
        }

        // The same arrangement the choosers have, and for the same reasons: it
        // is opened by the button rather than by hovering, and the press is
        // watched on its way past so that holding it can silence everything.
        {
            let volume_button = controls.volume_button.clone();
            let handle = controls.clone();
            volume_button.connect_clicked(move |_| {
                if handle.swallow_click.replace(false) {
                    return;
                }
                if handle.row.get() == Row::Volume {
                    handle.set_row(Row::Buttons);
                } else {
                    handle.open_levels();
                }
            });

            let controller = gtk::EventControllerLegacy::new();
            controller.set_propagation_phase(gtk::PropagationPhase::Capture);
            let handle = controls.clone();
            controller.connect_event(move |_, event| {
                match event.event_type() {
                    gdk::EventType::ButtonPress => handle.press_hold(Hold::Main),
                    gdk::EventType::ButtonRelease if !handle.release_hold() => {
                        handle.swallow_click.set(true);
                    }
                    _ => {}
                }
                glib::Propagation::Proceed
            });
            volume_button.add_controller(controller);
        }

        {
            let handle = controls.clone();
            controls.main_mute.connect_clicked(move |_| {
                handle.aim_at_main();
                handle.toggle_hush();
            });
            let handle = controls.clone();
            controls.main.connect_change_value(move |_, _, value| {
                handle.aim_at_main();
                handle.set_main(value.clamp(0.0, 1.0));
                glib::Propagation::Proceed
            });
        }

        for (index, output) in controls.outputs.iter().enumerate() {
            {
                let handle = controls.clone();
                output.mute.connect_clicked(move |_| {
                    handle.aim_at_output(index);
                    handle.toggle_muted(index);
                });
            }
            {
                let handle = controls.clone();
                output.level.connect_change_value(move |_, _, value| {
                    handle.aim_at(index, Control::Volume);
                    handle.set_level(index, value.clamp(0.0, 1.0));
                    glib::Propagation::Proceed
                });
            }
            {
                let handle = controls.clone();
                output.sync_toggle.connect_clicked(move |_| {
                    handle.aim_at(index, Control::Sync);
                    handle.toggle_sync(index);
                });
            }
            let handle = controls.clone();
            let max = crate::config::MAX_OFFSET_MS;
            output.sync.connect_change_value(move |_, _, value| {
                handle.aim_at(index, Control::Sync);
                handle.set_sync(index, value.clamp(-max, max));
                glib::Propagation::Proceed
            });
        }

        controls
    }

    /// Where each output's level stands, as a fraction of full, and whether it
    /// is silenced. The panel is built before anything has been played, so
    /// this is how playback tells it what to show. An output not listed is not
    /// in use, and its row is hidden rather than sitting there doing nothing.
    pub fn set_levels(&self, levels: &[(&str, f64, bool)]) {
        for (index, output) in self.outputs.iter().enumerate() {
            match levels.iter().find(|(role, _, _)| *role == output.role) {
                Some(&(_, level, muted)) => {
                    output.in_use.set(true);
                    output.level.set_value(level);
                    output
                        .level_reading
                        .set_text(&crate::app::volume_label(level, muted));
                    output.muted.set(muted);
                }
                // Not in use. Its button goes insensitive rather than hidden,
                // so the row of buttons keeps its shape from one film to the
                // next - and unlike the panel row this used to hide, a button
                // that comes and goes moves everything beside it.
                None => output.in_use.set(false),
            }
            output.button.set_sensitive(output.in_use.get());
            // Both outputs are shown together now, so this is decided here
            // rather than when the menu opens: an output turned off mid-film
            // takes its rows out of a menu that may be open at the time.
            output.group.set_visible(output.in_use.get());
            self.draw_output(index);
        }
        self.draw_main();
    }

    pub fn widget(&self) -> &gtk::Overlay {
        &self.root
    }

    pub fn row(&self) -> Row {
        self.row.get()
    }

    /// Puts the strip into, or takes it out of, being driven by a controller.
    ///
    /// While a row is being driven the strip stays up: hiding on a timer under
    /// someone who is deliberately moving through it would be maddening.
    pub fn set_row(self: &Rc<Self>, row: Row) {
        let was = self.row.replace(row);
        if was == Row::Volume && row != Row::Volume {
            self.panel.set_reveal_child(false);
            self.selected.set(false);
            self.select_position(None);
        }
        if was == Row::Subtitles && row != Row::Subtitles {
            self.subtitle_panel.set_reveal_child(false);
            self.selected.set(false);
            self.select_subtitle_row(None);
        }
        if was == Row::Audio && row != Row::Audio {
            self.audio_panel.set_reveal_child(false);
            self.selected.set(false);
            self.select_audio_row(None);
        }
        match row {
            // Straight away, rather than flashing it up and waiting out the
            // timer: down is a request to be rid of it.
            Row::None => self.hide(),
            Row::Buttons => {
                self.timeline_active(false);
                // Play, every time the row is taken hold of afresh, rather
                // than wherever it was left. Coming back to a highlight
                // somewhere down the row means hunting for it.
                if was == Row::None {
                    self.focused.set(PLAY);
                }
                // Nothing insensitive, so a file without subtitles does not
                // land on a button that cannot do anything.
                if !self.usable(self.focused.get()) {
                    self.step(1);
                }
                self.highlight(Some(self.focused.get()));
                self.flash(false);
            }
            Row::Timeline => {
                self.highlight(None);
                self.timeline_active(true);
                self.flash(false);
            }
            Row::Volume => {
                self.timeline_active(false);
                let button = self.volume_index();
                self.focused.set(button);
                self.highlight(Some(button));
                // Every output in use, together. One menu about the sound
                // rather than a menu per output, which is what the soundtrack
                // choosers on the strip are instead of.
                for output in self.outputs.iter() {
                    output.group.set_visible(output.in_use.get());
                }
                // Opened with nothing marked, resting on the last row - the
                // main level, which is nearest the button the menu rises out
                // of. The first press up marks it, and the outputs are above.
                //
                // Unlike a chooser, which opens on the choice in force. The
                // difference is what the menu is: a list is a set of choices
                // and has a place to start, where this is a set of controls and
                // marking one would mean the press that opened the menu was
                // followed by one working a control nobody had moved to.
                self.level_at.set(self.last_position());
                self.selected.set(false);
                self.select_position(None);
                self.panel.set_reveal_child(true);
                self.flash(false);
            }
            Row::Audio => {
                self.timeline_active(false);
                let index = self.output.get();
                self.focused.set(FIRST_OUTPUT + index);
                self.highlight(Some(FIRST_OUTPUT + index));
                // One scroller holds them all, so which output's list this is
                // comes down to which box inside it is showing.
                for (at, output) in self.outputs.iter().enumerate() {
                    output.tracks.set_visible(at == index);
                }
                // Opened on the soundtrack already playing, and marked at once,
                // exactly as the subtitle chooser opens. Both are lists, and a
                // list has both a place to start and a reason to say where.
                let output = &self.outputs[index];
                output.track_at.set(output.track_current.get().unwrap_or(0));
                self.selected.set(true);
                // Nothing below this list inside its own panel, so the bar is
                // all it has to clear.
                self.cap_list_height(&self.audio_scroll.clone(), 0);
                self.select_audio_row(Some(output.track_at.get()));
                self.audio_panel.set_reveal_child(true);
                self.flash(false);
            }
            Row::Subtitles => {
                self.timeline_active(false);
                self.focused.set(SUBTITLES);
                self.highlight(Some(SUBTITLES));
                // Opened on whatever is already in force, and marked at once.
                //
                // Unlike the volume panel, which opens with nothing marked
                // because it has no "current" to open on - marking a row there
                // would mean the press that opened the panel was followed by
                // one working a control nobody had moved to yet. A list of
                // choices has both a place to start and a reason to say where,
                // and leaving it unmarked meant pressing up before the list
                // could be read at all.
                self.subtitle_at
                    .set(self.subtitle_current.get().unwrap_or(0));
                self.selected.set(true);
                // Nothing below this list inside its own panel, unlike an
                // output's menu, so the bar is all it has to clear.
                self.cap_list_height(&self.subtitle_scroll.clone(), 0);
                self.select_subtitle_row(Some(self.subtitle_at.get()));
                self.subtitle_panel.set_reveal_child(true);
                self.flash(false);
            }
        }
        // After the arm, not inside it: every one of them has just settled
        // where the strip is, and Row::None has already let go through hide().
        self.announce_current();
    }

    /// Opens one output's soundtracks, on the one it is already playing.
    pub fn open_audio(self: &Rc<Self>, index: usize) {
        if index >= self.outputs.len() {
            return;
        }
        self.output.set(index);
        self.set_row(Row::Audio);
    }

    /// Opens the levels menu, which is what the speaker on the strip does.
    pub fn open_levels(self: &Rc<Self>) {
        self.set_row(Row::Volume);
    }

    /// Where the speaker sits in the button order: after one soundtrack chooser
    /// per output, however many there are, and before fullscreen.
    fn volume_index(&self) -> usize {
        FIRST_OUTPUT + self.outputs.len()
    }

    /// Silences one output, or puts it back. What holding that output's button
    /// means, where the keyboard's `M` still silences everything.
    ///
    /// The same path as the mute button inside the menu, so there is one answer
    /// to what an output's silence means however it was asked for.
    fn press_mute(self: &Rc<Self>, index: usize) {
        self.toggle_muted(index);
    }

    /// Caps a list so it cannot grow past the window, leaving room for the bar
    /// it rises out of and for whatever sits below it inside its own panel.
    ///
    /// Set when the list is opened rather than once at build time, because the
    /// height it has to fit inside is not a constant: the window is resized,
    /// fullscreen changes it, and `ui_scale` changes how much a row costs. The
    /// bar's own height is measured rather than assumed for the same reason.
    ///
    /// A floor under it, so that a window too short for any of this shows a
    /// couple of rows and scrolls rather than collapsing the list to nothing
    /// and looking broken.
    fn cap_list_height(&self, scroll: &gtk::ScrolledWindow, below: i32) {
        let available = self.root.height() - self.bar_height() - below;
        let floor = (MIN_LIST_HEIGHT * self.scale) as i32;
        scroll.set_max_content_height(available.max(floor));
    }

    /// How tall the bar itself is, so a list can be kept clear of it.
    ///
    /// Measured as well as read, and the larger taken, because an allocation is
    /// zero until the widget has been given one - and a list is very often
    /// opened out of a strip that is only now sliding into place. A zero here
    /// left the cap a whole bar too generous, so the same film gave a list that
    /// filled the screen or one that stopped short and scrolled depending on
    /// whether the strip happened to be up when it was asked. Reported by
    /// Scott, 2026-08-14.
    fn bar_height(&self) -> i32 {
        let (_, natural, _, _) = self.holder.measure(gtk::Orientation::Vertical, -1);
        self.strip.height().max(self.holder.height()).max(natural)
    }

    /// Opens the subtitle chooser on whatever is already in force, with that
    /// row marked and holding the focus, so it can be read and moved from
    /// without a press spent finding out where it starts.
    pub fn open_subtitles(self: &Rc<Self>) {
        self.set_row(Row::Subtitles);
    }

    /// Swaps the right-hand readout between the length and what is left of
    /// it, the same as clicking it does. Peeks the strip on the way, since
    /// otherwise a press that only changes a hidden readout looks like a
    /// press that did nothing.
    pub fn toggle_remaining(self: &Rc<Self>) {
        self.remaining.set(!self.remaining.get());
        self.peek();
    }

    /// Which button, if either, should have a press on it held rather than
    /// acted on at once. Only from the button row: inside a panel the same
    /// press works whatever the panel is pointing at, and holding it there
    /// would be two meanings on one button.
    pub fn holds_press(&self) -> Option<Hold> {
        if self.row.get() != Row::Buttons {
            return None;
        }
        let focused = self.focused.get();
        if focused == SUBTITLES {
            return Some(Hold::Subtitles);
        }
        if focused == self.volume_index() {
            return Some(Hold::Main);
        }
        let index = focused.checked_sub(FIRST_OUTPUT)?;
        (index < self.outputs.len()).then_some(Hold::Output(index))
    }

    /// Starts a hold. Repeats are ignored: a keyboard sends a press over and
    /// over while a key is down, and restarting the timer on each one would
    /// mean it never finished.
    ///
    /// What the hold does is settled here, where the button is still known,
    /// rather than asked again when the timer fires: by then the strip may
    /// have moved, and the answer would be about wherever it moved to.
    pub fn press_hold(self: &Rc<Self>, which: Hold) {
        if self.holding.replace(true) {
            return;
        }
        self.held.set(false);
        let mark = self.hold.get() + 1;
        self.hold.set(mark);
        let handle = self.clone();
        glib::timeout_add_local_once(HOLD, move || {
            if handle.hold.get() != mark {
                return;
            }
            handle.held.set(true);
            match which {
                Hold::Output(index) => handle.press_mute(index),
                Hold::Main => handle.toggle_hush(),
                Hold::Subtitles => handle.toggle_subtitles(),
            }
        });
    }

    /// Ends a hold, and says whether the release should still do the ordinary
    /// thing - which it should not, if the hold already did something else.
    ///
    /// Needs no [`Hold`]: the timer already captured which button it was, and
    /// only one can be held at a time.
    pub fn release_hold(self: &Rc<Self>) -> bool {
        self.holding.set(false);
        self.hold.set(self.hold.get() + 1);
        !self.held.replace(false)
    }

    /// Shows or hides the subtitle already chosen, without opening the list to
    /// choose a different one. Flashing the strip is the caller's, since the
    /// only confirmation is the icon dimming or lighting.
    fn toggle_subtitles(&self) {
        if let Some(handler) = self.on_toggle_subtitles.borrow().as_ref() {
            handler();
        }
    }

    /// Silences every output at once, or lets them go again. For the moment
    /// somebody knocks at the door: two outputs means two things to silence,
    /// and reaching into the menu for both of them is what this is instead of.
    ///
    /// **It changes no output's own state.** It is a layer over the top of
    /// them, which is why nothing here has to remember what to put back: an
    /// output muted before the door was knocked on is still muted after, and
    /// one that was not, is not. The mute buttons in the menu go on reporting
    /// what each output is set to throughout, and the only marks that move are
    /// The main level and the speaker on the strip.
    ///
    /// The first version of this wrote `true` into every output and kept the
    /// old values to restore, which flipped all the icons together and made the
    /// silence look like something that had been done to each output. Reported
    /// by Scott, 2026-08-14.
    pub fn toggle_hush(self: &Rc<Self>) {
        let hushed = !self.hushed.get();
        self.hushed.set(hushed);
        // Never kept. A film that started silent because of a door knocked on
        // last week would be a bug rather than a memory.
        if let Some(handler) = self.on_hush.borrow().as_ref() {
            handler(hushed);
        }
        self.draw_main();
        self.flash(false);
    }

    /// Told when everything is silenced at once, or let go.
    pub fn connect_hush(&self, handler: impl Fn(bool) + 'static) {
        *self.on_hush.borrow_mut() = Some(Box::new(handler));
    }

    /// Points the menu at an output's level, which is what a pointer touching
    /// it means. If a mark is already showing it follows along, so the two
    /// ways of driving it do not disagree about where it is.
    fn aim_at_output(&self, index: usize) {
        self.aim_at(index, Control::Volume);
    }

    /// The same, for the main level at the foot.
    fn aim_at_main(&self) {
        self.aim(Level::Main);
    }

    /// Points the menu at one of an output's controls: the pointer is about to
    /// change that one, so the keyboard should carry on from there rather than
    /// from wherever it was.
    fn aim_at(&self, index: usize, control: Control) {
        self.aim(Level::Output(index, control));
    }

    /// Puts the cursor on one row of the levels menu without moving anything.
    ///
    /// A row the menu is not currently offering is ignored rather than
    /// searched for: an output that is not in use has no row to aim at, and
    /// leaving the cursor where it was is better than moving it somewhere
    /// nobody asked for.
    fn aim(&self, level: Level) {
        let Some(at) = self.levels().iter().position(|row| *row == level) else {
            return;
        };
        self.level_at.set(at);
        if self.selected.get() {
            self.select_position(Some(at));
        }
    }

    /// How many soundtracks the open chooser is offering.
    fn track_count(&self) -> usize {
        self.outputs[self.output.get()].track_rows.borrow().len()
    }

    /// Every row of the levels menu in the order they are drawn: each output in
    /// use with its level and its sync, then the main level last.
    ///
    /// Built on the spot rather than kept, because an output comes and goes
    /// while a film plays - set the second device to None and its rows should
    /// leave the menu with it. A stored list would be a second answer to that,
    /// free to disagree with `in_use`.
    fn levels(&self) -> Vec<Level> {
        let mut rows = Vec::new();
        for (index, output) in self.outputs.iter().enumerate() {
            if !output.in_use.get() {
                continue;
            }
            rows.push(Level::Output(index, Control::Volume));
            rows.push(Level::Output(index, Control::Sync));
        }
        rows.push(Level::Main);
        rows
    }

    /// The row the cursor is on. The main level when the count has shrunk under
    /// it, which is where an output going away leaves it.
    fn current_level(&self) -> Level {
        let rows = self.levels();
        rows.get(self.level_at.get())
            .copied()
            .unwrap_or(Level::Main)
    }

    /// The lowest row of the levels menu, which is always the main level.
    fn last_position(&self) -> usize {
        self.levels().len().saturating_sub(1)
    }

    /// Whether the cursor is at the bottom of the menu, which is what makes a
    /// downward press leave it rather than move within it.
    pub fn at_last_output(&self) -> bool {
        self.selected.get() && self.position() >= self.last_position()
    }

    /// Moves through the levels menu, stopping at either end rather than
    /// wrapping, the same way the button row and the choosers do.
    pub fn move_output(self: &Rc<Self>, delta: isize) {
        if self.row.get() != Row::Volume {
            return;
        }
        // The first press marks where the menu already rests rather than
        // moving from a place nobody can see. It opens unmarked, so without
        // this the press that takes hold of it would also move it, and the row
        // it started on could never be reached in one press.
        if !self.selected.replace(true) {
            self.select_position(Some(self.position()));
            self.flash(false);
            return;
        }
        let next = self.position() as isize + delta;
        if next < 0 || next as usize > self.last_position() {
            return;
        }
        self.level_at.set(next as usize);
        self.select_position(Some(next as usize));
        self.flash(false);
    }

    /// Where the cursor is in the levels menu.
    fn position(&self) -> usize {
        self.level_at.get().min(self.last_position())
    }

    /// Marks one row of the levels menu and clears every other. `None` clears
    /// them all, which is how the menu opens and how it closes.
    fn select_position(&self, position: Option<usize>) {
        let rows = self.levels();
        let here = position.and_then(|at| rows.get(at).copied());
        for (index, output) in self.outputs.iter().enumerate() {
            mark(
                &output.row,
                here == Some(Level::Output(index, Control::Volume)),
            );
            mark(
                &output.sync_row,
                here == Some(Level::Output(index, Control::Sync)),
            );
        }
        mark(&self.main_row, here == Some(Level::Main));
        self.announce_current();
    }

    /// The bottom row of the open soundtrack chooser, which is where a downward
    /// press leaves it from.
    fn last_track(&self) -> usize {
        self.track_count().saturating_sub(1)
    }

    /// Whether the cursor is on that bottom row.
    pub fn at_last_audio(&self) -> bool {
        self.outputs[self.output.get()].track_at.get() >= self.last_track()
    }

    /// Moves through the open soundtrack chooser, stopping at either end.
    ///
    /// Straight to moving, like the subtitle chooser and unlike the levels
    /// menu: this opened already marked, so a press that only lit a row
    /// somebody could already see would be a press wasted.
    pub fn move_audio(self: &Rc<Self>, delta: isize) {
        if self.row.get() != Row::Audio {
            return;
        }
        let output = &self.outputs[self.output.get()];
        let next = output.track_at.get() as isize + delta;
        if next < 0 || next > self.last_track() as isize {
            return;
        }
        output.track_at.set(next as usize);
        self.select_audio_row(Some(next as usize));
        self.announce_current();
        self.flash(false);
    }

    /// Marks one row of the open soundtrack chooser and clears every other, in
    /// every output's list so a closed one leaves nothing lit behind it.
    fn select_audio_row(&self, at: Option<usize>) {
        let open = self.output.get();
        for (index, output) in self.outputs.iter().enumerate() {
            let here = (index == open).then_some(at).flatten();
            for (row_at, row) in output.track_rows.borrow().iter().enumerate() {
                mark(row, here == Some(row_at));
                // Scrolled to as it is marked, so a cursor moving past the
                // bottom of a long list brings the row with it rather than
                // marking one nobody can see.
                if here == Some(row_at) {
                    reveal_row(&self.audio_scroll, row);
                }
            }
        }
    }

    /// Fills one output's soundtrack list, and says which of them it is
    /// playing.
    ///
    /// Rebuilt whole for the same reason the subtitle chooser is: it is called
    /// whenever any of it could have changed, and the list is a handful of
    /// labels. `current` counts into `entries`, and `None` marks nothing.
    pub fn set_audio_entries(
        self: &Rc<Self>,
        index: usize,
        entries: &[String],
        current: Option<usize>,
    ) {
        let Some(output) = self.outputs.get(index) else {
            return;
        };
        while let Some(child) = output.tracks.first_child() {
            output.tracks.remove(&child);
        }
        let mut rows = Vec::new();
        for (at, text) in entries.iter().enumerate() {
            let row = subtitle_row(text);
            name_it(&row, text);
            if Some(at) == current {
                row.add_css_class("tp-current");
            }
            // A pointer picks a row by clicking it, the same press a
            // controller makes on the row it has moved to.
            {
                let handle = self.clone();
                let gesture = gtk::GestureClick::new();
                gesture.connect_released(move |_, _, _, _| handle.choose_audio(index, at));
                row.add_controller(gesture);
            }
            output.tracks.append(&row);
            rows.push(row);
        }
        *output.track_rows.borrow_mut() = rows;
        output.track_current.set(current);

        // Not the open chooser, so there is no cursor to disturb: leave it
        // where this output's list will open.
        if self.row.get() != Row::Audio || self.output.get() != index {
            output.track_at.set(current.unwrap_or(0));
            return;
        }
        // The list changed under an open chooser. The cursor stays where it was
        // rather than being pulled back to what is playing: somebody is moving
        // through it, and moving it under them would be worse than not
        // redrawing at all.
        let at = output.track_at.get().min(entries.len().saturating_sub(1));
        output.track_at.set(at);
        if self.selected.get() {
            self.select_audio_row(Some(at));
        }
    }

    /// Says which soundtrack was picked, and closes the menu the way choosing
    /// a subtitle closes the chooser.
    fn choose_audio(self: &Rc<Self>, index: usize, at: usize) {
        if let Some(handler) = self.on_audio.borrow().as_ref() {
            handler(self.outputs[index].role, at);
        }
        self.set_row(Row::Buttons);
    }

    /// Told which soundtrack an output was put onto.
    pub fn connect_audio_chosen(&self, handler: impl Fn(&str, usize) + 'static) {
        *self.on_audio.borrow_mut() = Some(Box::new(handler));
    }

    /// Fills the chooser with what this video offers, and says which of them
    /// is in force.
    ///
    /// Rebuilt whole rather than patched, because it is called whenever any of
    /// it could have changed - a different film, a different choice - and the
    /// list is a handful of labels. `current` counts into `entries`; `None`
    /// means nothing in the list is in force, which the caller signals by
    /// having no rows worth marking rather than by leaving one out.
    pub fn set_subtitle_entries(self: &Rc<Self>, entries: &[String], current: Option<usize>) {
        while let Some(child) = self.subtitle_list.first_child() {
            self.subtitle_list.remove(&child);
        }
        let mut rows = Vec::new();
        for (index, text) in entries.iter().enumerate() {
            let row = subtitle_row(text);
            name_it(&row, text);
            if Some(index) == current {
                row.add_css_class("tp-current");
            }
            // A pointer picks a row by clicking it, the same press a
            // controller makes on the row it has moved to.
            {
                let handle = self.clone();
                let gesture = gtk::GestureClick::new();
                gesture.connect_released(move |_, _, _, _| handle.choose_subtitle(index));
                row.add_controller(gesture);
            }
            self.subtitle_list.append(&row);
            rows.push(row);
        }
        *self.subtitle_rows.borrow_mut() = rows;
        self.subtitle_current.set(current);

        if self.row.get() != Row::Subtitles {
            self.subtitle_at.set(current.unwrap_or(0));
            return;
        }
        // The list changed under an open chooser, which the subtitle toggle
        // can do from a keyboard or a gamepad at any moment. The cursor stays
        // where it was rather than being pulled back to what is in force -
        // somebody is moving through the list, and moving it under them is
        // the one thing that would be worse than not redrawing at all.
        let at = self.subtitle_at.get().min(self.last_subtitle());
        self.subtitle_at.set(at);
        if self.selected.get() {
            self.select_subtitle_row(Some(at));
        }
    }

    /// The bottom row of the chooser, which is where a downward press leaves
    /// it from. Zero for an empty list, which no press can reach anyway.
    fn last_subtitle(&self) -> usize {
        self.subtitle_rows.borrow().len().saturating_sub(1)
    }

    /// Whether the cursor is on the bottom row, which is what makes a downward
    /// press leave the chooser rather than move within it.
    pub fn at_last_subtitle(&self) -> bool {
        self.subtitle_at.get() >= self.last_subtitle()
    }

    /// Moves through the chooser, stopping at either end rather than wrapping,
    /// the same way the button row and the volume panel do.
    pub fn move_subtitle(self: &Rc<Self>, delta: isize) {
        if self.row.get() != Row::Subtitles {
            return;
        }
        // Straight to moving. The volume panel spends the first press marking
        // where it already is; this one opened already marked, so a press that
        // only lit a row somebody could already see would be a press wasted.
        let next = self.subtitle_at.get() as isize + delta;
        if next < 0 || next > self.last_subtitle() as isize {
            return;
        }
        self.subtitle_at.set(next as usize);
        self.select_subtitle();
        self.flash(false);
    }

    fn select_subtitle(&self) {
        self.select_subtitle_row(Some(self.subtitle_at.get()));
        // Everything that moves within the chooser comes through here, both
        // the first press that takes hold of it and every one after.
        self.announce_current();
    }

    /// Marks one row of the chooser and clears every other. `None` clears them
    /// all. The row that is in force keeps its own mark throughout - see
    /// `subtitle_current`.
    fn select_subtitle_row(&self, at: Option<usize>) {
        for (index, row) in self.subtitle_rows.borrow().iter().enumerate() {
            mark(row, Some(index) == at);
            // Brought into view as it is marked, the same as an output's
            // soundtracks: a list of thirteen subtitles is taller than the
            // window, and a mark below the fold is a cursor that has vanished.
            if Some(index) == at {
                reveal_row(&self.subtitle_scroll, row);
            }
        }
    }

    /// Takes a row and closes the chooser: picking one is a finished act, and
    /// what somebody wants next is the film rather than the list.
    ///
    /// Back to the buttons rather than off the strip entirely, so the icon it
    /// came out of is highlighted and can be seen to have changed.
    fn choose_subtitle(self: &Rc<Self>, entry: usize) {
        self.set_row(Row::Buttons);
        if let Some(handler) = self.on_subtitle.borrow().as_ref() {
            handler(entry);
        }
    }

    /// Moves whichever bar is marked, while the levels menu is open. Every row
    /// in it is a bar, which is what lets left and right mean one thing here.
    pub fn adjust_level(self: &Rc<Self>, delta: isize) {
        if self.row.get() != Row::Volume {
            return;
        }
        match self.current_level() {
            Level::Output(index, Control::Volume) => {
                let Some(output) = self.outputs.get(index) else {
                    return;
                };
                let level = (output.level.value() + delta as f64 * VOLUME_STEP).clamp(0.0, 1.0);
                output.level.set_value(level);
                self.set_level(index, level);
            }
            Level::Output(index, Control::Sync) => self.shift_by(index, delta),
            Level::Main => self.nudge_main(delta),
        }
    }

    /// Moves the level over every output, wherever the strip happens to be.
    ///
    /// What the volume keys drive. Deliberately not "the output you last
    /// touched": with two people listening on two devices, the only level that
    /// means "this film is too loud" without asking whose headphones you meant
    /// is the one over both. Per-output levels stay in the panel, where each
    /// row says which device it belongs to.
    ///
    /// Shows the panel as it moves, because a level that changes with nothing
    /// on screen to say so is indistinguishable from a stuck key.
    pub fn nudge_main(self: &Rc<Self>, delta: isize) {
        let level = (self.main.value() + delta as f64 * VOLUME_STEP).clamp(0.0, 1.0);
        // The same road a hand or a remote takes, rather than a second one.
        self.main_to(level);
        if self.row.get() != Row::Volume {
            self.set_row(Row::Volume);
        }
        // Marked on the row being moved. The menu opens unmarked when a hand
        // opens it, because nothing has been chosen yet - but here something
        // has, and it is this row.
        let at = self.last_position();
        self.level_at.set(at);
        self.selected.set(true);
        self.select_position(Some(at));
    }

    /// Steps one output's shift, snapped to whole steps.
    ///
    /// Snapped because a pointer can leave the slider anywhere - 37ms, say -
    /// and stepping in tens from there would pass either side of zero without
    /// ever landing on it, which is the one value somebody is certain to want
    /// back.
    fn shift_by(self: &Rc<Self>, index: usize, delta: isize) {
        let Some(output) = self.outputs.get(index) else {
            return;
        };
        let steps = (output.sync.value() / SYNC_STEP).round() + delta as f64;
        self.shift_to(index, steps * SYNC_STEP);
    }

    /// Puts one output at a given shift, moving the slider with it.
    fn shift_to(self: &Rc<Self>, index: usize, ms: f64) {
        let Some(output) = self.outputs.get(index) else {
            return;
        };
        let max = crate::config::MAX_OFFSET_MS;
        let ms = ms.clamp(-max, max);
        output.sync.set_value(ms);
        self.set_sync(index, ms);
        self.flash(false);
    }

    /// Shifts one output in time and tells whoever is listening, so the change
    /// is heard against the picture as it is made. That is the whole reason
    /// this sits over the video rather than only in the settings menu: a delay
    /// can be judged against what is on screen and not by arithmetic.
    fn set_sync(self: &Rc<Self>, index: usize, ms: f64) {
        let Some(output) = self.outputs.get(index) else {
            return;
        };
        // Moving the bar turns the delay on, the way turning an output up
        // unmutes it: a control that moves and changes nothing audible reads
        // as a fault rather than as a setting.
        output.sync_on.set(true);
        Self::draw_sync(&output.sync_toggle, true);
        output.sync_reading.set_text(&crate::app::offset_label(ms));
        if let Some(handler) = self.on_sync.borrow().as_ref() {
            handler(output.role, ms, true);
        }
    }

    /// Uses one output's shift or stops using it, keeping what it is set to.
    ///
    /// The same gesture as mute, and for the same reason: hearing the
    /// difference means turning it off and on, which is no use if doing so
    /// throws away the value being judged.
    fn toggle_sync(self: &Rc<Self>, index: usize) {
        let Some(output) = self.outputs.get(index) else {
            return;
        };
        let on = !output.sync_on.get();
        output.sync_on.set(on);
        Self::draw_sync(&output.sync_toggle, on);
        let ms = output.sync.value();
        output
            .sync_reading
            .set_text(&crate::app::offset_label(if on { ms } else { 0.0 }));
        if let Some(handler) = self.on_sync.borrow().as_ref() {
            handler(output.role, ms, on);
        }
        self.flash(false);
    }

    /// Dimmed while the delay is not being used. One drawn mark rather than
    /// two: there is no second glyph meaning "not synchronised", and a button
    /// that changes shape says something different happened.
    fn draw_sync(button: &gtk::Button, on: bool) {
        if on {
            button.remove_css_class("tp-off");
        } else {
            button.add_css_class("tp-off");
        }
    }

    /// Told about a shift or about the switch, however it was made.
    pub fn connect_sync(&self, handler: impl Fn(&str, f64, bool) + 'static) {
        *self.on_sync.borrow_mut() = Some(Box::new(handler));
    }

    /// Where each output's shift stands and whether it is being applied, so
    /// the panel shows what the configuration holds when playback starts.
    pub fn set_syncs(&self, offsets: &[(&str, f64, bool)]) {
        for output in &self.outputs {
            if let Some(&(_, ms, on)) = offsets.iter().find(|(role, ..)| *role == output.role) {
                output.sync.set_value(ms);
                output.sync_on.set(on);
                Self::draw_sync(&output.sync_toggle, on);
                output
                    .sync_reading
                    .set_text(&crate::app::offset_label(if on { ms } else { 0.0 }));
            }
        }
    }

    /// Turning an output up unmutes it: hearing nothing after asking for more
    /// would look like a fault rather than a setting.
    ///
    /// A blanket silence is deliberately left alone. It is the main level's to
    /// lift, and lifting it from here would make one output's slider let the
    /// *other* one back in - which is the opposite of what an output's own
    /// control should be able to do.
    fn set_level(self: &Rc<Self>, index: usize, level: f64) {
        let Some(output) = self.outputs.get(index) else {
            return;
        };
        let muted = output.muted.get() && level <= 0.0;
        output.muted.set(muted);
        output
            .level_reading
            .set_text(&crate::app::volume_label(level, muted));
        self.draw_output(index);
        self.report(index, true);
        self.flash(false);
    }

    /// Silences one output, or lets it go, whatever the main level is doing
    /// over the top of it.
    fn toggle_muted(self: &Rc<Self>, index: usize) {
        let Some(output) = self.outputs.get(index) else {
            return;
        };
        let muted = !output.muted.get();
        output.muted.set(muted);
        self.draw_output(index);
        self.report(index, true);
        self.flash(false);
    }

    fn report(&self, index: usize, persist: bool) {
        let Some(output) = self.outputs.get(index) else {
            return;
        };
        if let Some(handler) = self.on_volume.borrow().as_ref() {
            handler(
                output.role,
                output.level.value(),
                output.muted.get(),
                persist,
            );
        }
    }

    /// Fires with the output, its level as a fraction of full, whether it is
    /// silenced, and whether to keep the change, every time any of it moves.
    pub fn connect_volume(&self, handler: impl Fn(&str, f64, bool, bool) + 'static) {
        *self.on_volume.borrow_mut() = Some(Box::new(handler));
    }

    /// Draws the mute button inside one output's rows.
    ///
    /// Read off the output rather than passed in, so that a caller which has
    /// just changed a level cannot leave this saying something else. Every
    /// place that moves a level or a mute comes through here.
    ///
    /// The blanket silence deliberately does not show here. This button reports
    /// what the output itself is set to, which a hush lays over rather than
    /// replaces, and putting it back has to find it unchanged. Where the hush
    /// does show is the speaker on the strip - see [`Self::draw_main`].
    fn draw_output(&self, index: usize) {
        let Some(output) = self.outputs.get(index) else {
            return;
        };
        output
            .mute
            .set_icon_name(volume_icon_name(output.level.value(), output.muted.get()));
        // And on the strip, where this output's own icon is the only thing that
        // can say it. Its own mute rather than the blanket silence, which
        // belongs to the speaker beside them: what these report is what each
        // output is set to, which is exactly what a hush leaves alone.
        if output.muted.get() {
            output.button.add_css_class("tp-soundtrack-muted");
        } else {
            output.button.remove_css_class("tp-soundtrack-muted");
        }
        self.draw_main();
    }

    /// Draws the main level: the button in its own row, and the speaker on the
    /// strip that opens the menu.
    ///
    /// The speaker stages with the main level, which is the one number that
    /// governs everything, and reads as silent when a hush is on or when every
    /// output in use is muted - because in both of those cases nothing is
    /// audible, and a speaker showing three quarters over a silent room is a
    /// lie a viewer has no way to check.
    fn draw_main(&self) {
        let level = self.main.value();
        let silent = self.hushed.get()
            || self
                .outputs
                .iter()
                .filter(|output| output.in_use.get())
                .all(|output| output.muted.get())
            || level <= 0.0;
        self.main_mute
            .set_icon_name(volume_icon_name(level, self.hushed.get()));
        self.volume_icon
            .set_icon_name(Some(volume_icon_name(level, silent)));
    }

    /// Moves the main level, and tells whoever is listening so every output can
    /// be pushed again at the level it now works out to.
    fn set_main(self: &Rc<Self>, level: f64) {
        self.main_reading
            .set_text(&crate::app::volume_label(level, false));
        self.draw_main();
        if let Some(handler) = self.on_main.borrow().as_ref() {
            handler(level);
        }
        self.flash(false);
    }

    /// Puts the main level where something other than a hand asked for it - a
    /// remote, today - by moving the bar and then taking the same road a hand
    /// would. The person in the room and the person with the phone drive one
    /// control rather than two paths to the same setting.
    pub fn main_to(self: &Rc<Self>, level: f64) {
        let level = level.clamp(0.0, 1.0);
        self.main.set_value(level);
        self.set_main(level);
    }

    /// Silences everything, or lets it go, when told which rather than to
    /// swap. Jellyfin offers Mute and Unmute as well as a toggle, and a remote
    /// that says "mute" while everything is already silent should leave it
    /// silent rather than turning the film back on.
    pub fn set_hushed(self: &Rc<Self>, hushed: bool) {
        if self.hushed.get() != hushed {
            self.toggle_hush();
        }
    }

    /// Where the main level stands, which playback sets from the configuration
    /// before a frame has played.
    pub fn set_main_level(&self, level: f64) {
        let level = level.clamp(0.0, 1.0);
        self.main.set_value(level);
        self.main_reading
            .set_text(&crate::app::volume_label(level, false));
        self.draw_main();
    }

    /// Whether the key list is on screen.
    pub fn shortcuts_open(&self) -> bool {
        self.shortcuts.reveals_child()
    }

    /// Shows the key list, or puts it away.
    ///
    /// Takes the strip down with it when it opens: the list explains the
    /// controls, and having both on screen leaves the strip's own highlight
    /// competing with a page nobody can drive.
    pub fn toggle_shortcuts(self: &Rc<Self>, scale: f64) {
        let opening = !self.shortcuts_open();
        if opening {
            // Built now, at the scale in force now. The stylesheet is only
            // half of a size: the spacing between rows and columns is worked
            // out in Rust when the list is made, and a list made once when the
            // controls were built keeps a windowed layout after a fullscreen
            // television has doubled everything around it. The same reasoning
            // as `App::restyle`, which rebuilds the menu for it.
            let page = crate::shortcuts::page(scale);
            page.add_css_class("tp-shortcuts");
            page.set_valign(gtk::Align::Start);
            self.shortcuts_scroll.set_child(Some(&page));

            self.set_row(Row::None);
            self.shortcuts_scroll.vadjustment().set_value(0.0);
            // The panel is as tall as the list and no taller, which is what a
            // ceiling rather than a height gives: `propagate_natural_height`
            // asks for the content's own height, and this only stops it
            // running off a short window - at which point it scrolls instead.
            // Measured now rather than at build time, since the window can be
            // resized, and gone fullscreen, since it was made.
            let room = (f64::from(self.root.height()) * SHORTCUTS_SHARE).round() as i32;
            if room > 0 {
                self.shortcuts_scroll.set_max_content_height(room);
            }
        }
        self.shortcuts.set_reveal_child(opening);
    }

    /// Closes it if it is open, and says whether it was. What "back" calls to
    /// find out whether it has already been answered.
    pub fn close_shortcuts(self: &Rc<Self>) -> bool {
        let open = self.shortcuts_open();
        if open {
            self.shortcuts.set_reveal_child(false);
        }
        open
    }

    /// Scrolls the key list, for the arrows and the D-pad while it is open.
    pub fn scroll_shortcuts(&self, delta: isize) {
        let adjustment = self.shortcuts_scroll.vadjustment();
        let step = adjustment.step_increment().max(24.0);
        let value = adjustment.value() + delta as f64 * step;
        adjustment.set_value(value.clamp(0.0, adjustment.upper() - adjustment.page_size()));
    }

    /// Told where the main level now stands, every time it moves.
    pub fn connect_main(&self, handler: impl Fn(f64) + 'static) {
        *self.on_main.borrow_mut() = Some(Box::new(handler));
    }

    /// A button worth landing on: one that is there and can act. Both, since
    /// the fullscreen button is hidden outright on a run where fullscreen is
    /// fixed, and a highlight on an invisible button would look like the row
    /// had swallowed a press.
    fn usable(&self, index: usize) -> bool {
        self.order
            .get(index)
            .is_some_and(|button| button.is_sensitive() && button.is_visible())
    }

    /// Moves to the next usable button in that direction, stopping at the
    /// end rather than wrapping: a row that comes back round the other side
    /// is disorienting when you cannot see where it starts.
    fn step(&self, delta: isize) {
        let mut index = self.focused.get() as isize;
        loop {
            index += delta;
            if index < 0 || index as usize >= self.order.len() {
                return;
            }
            if self.usable(index as usize) {
                self.focused.set(index as usize);
                return;
            }
        }
    }

    /// Lets go of the strip without touching whether it is on screen.
    fn release(&self) {
        if self.row.get() == Row::Volume {
            self.panel.set_reveal_child(false);
            self.selected.set(false);
            self.select_position(None);
        }
        if self.row.get() == Row::Subtitles {
            self.subtitle_panel.set_reveal_child(false);
            self.selected.set(false);
            self.select_subtitle_row(None);
        }
        if self.row.get() == Row::Audio {
            self.audio_panel.set_reveal_child(false);
            self.selected.set(false);
            self.select_audio_row(None);
        }
        self.row.set(Row::None);
        self.highlight(None);
        self.timeline_active(false);
        // Row::None now, so this hands the focus back rather than leaving it
        // on a strip that is about to go.
        self.announce_current();
    }

    pub fn move_focus(self: &Rc<Self>, delta: isize) {
        if self.row.get() != Row::Buttons {
            return;
        }
        self.step(delta);
        self.highlight(Some(self.focused.get()));
        self.announce_current();
        // Restarts the countdown, so working along the row does not run out
        // of time part way.
        self.flash(false);
    }

    pub fn activate_focused(self: &Rc<Self>) {
        match self.row.get() {
            // An output's menu goes its own way rather than through the
            // button's click handler: that handler cannot tell a press from a
            // pointer, and these are buttons where the difference shows, since
            // a press may yet turn out to be a hold.
            Row::Buttons
                if (FIRST_OUTPUT..FIRST_OUTPUT + self.outputs.len())
                    .contains(&self.focused.get()) =>
            {
                self.open_audio(self.focused.get() - FIRST_OUTPUT)
            }
            // The speaker, for the same reason again: it is held to silence
            // everything, so a press cannot be acted on until it is known not
            // to be a hold.
            Row::Buttons if self.focused.get() == self.volume_index() => self.open_levels(),
            // The same, and for the same reason: the click handler cannot
            // tell a press from a pointer, and this is a button where the
            // difference shows - a press may yet turn out to be a hold.
            Row::Buttons if self.focused.get() == SUBTITLES => self.open_subtitles(),
            Row::Buttons => {
                if let Some(button) = self.order.get(self.focused.get()) {
                    button.emit_clicked();
                }
            }
            // Nothing inside the menu taken hold of yet: the button is still
            // what is highlighted, so pressing it again shuts what it opened -
            // the same as clicking it a second time.
            Row::Volume if !self.selected.get() => self.set_row(Row::Buttons),
            // The button belonging to the marked row, and all three are the
            // same gesture: mute on a level row, use the delay or not on a sync
            // row, silence everything on the main level. None of them loses
            // what it is turning off - the level, the delay and the main level
            // all stay where they were.
            Row::Volume => match self.current_level() {
                Level::Output(index, Control::Volume) => self.toggle_muted(index),
                Level::Output(index, Control::Sync) => self.toggle_sync(index),
                Level::Main => self.toggle_hush(),
            },
            // A chooser opens on a marked row, so there is no unmarked state to
            // press through: a press takes the row it is on, which if nobody
            // has moved is what is already playing. That costs nothing and
            // closes the list, which is how pressing the icon twice still shuts
            // what it opened.
            Row::Audio => self.choose_audio(self.output.get(), self.current_track()),
            // The chooser opens on a marked row, so there is no unmarked state
            // to press through: a press takes the row it is on. Pressing
            // straight through therefore takes what is already playing, which
            // costs nothing and closes the list - the same as pressing the
            // icon a second time. See `App::choose_subtitle`, which is where
            // choosing what is already chosen is made free.
            Row::Subtitles => self.choose_subtitle(self.subtitle_at.get()),
            _ => {}
        }
    }

    /// Whether a press belongs to the strip rather than to playback.
    pub fn takes_activation(&self) -> bool {
        matches!(
            self.row.get(),
            Row::Buttons | Row::Volume | Row::Audio | Row::Subtitles
        )
    }

    /// Where the cursor is in the open soundtrack chooser.
    fn current_track(&self) -> usize {
        self.outputs[self.output.get()].track_at.get()
    }

    /// Points assistive technology at whatever the strip is on now.
    ///
    /// Worked out from the row being driven rather than hooked onto each of
    /// the three marks separately. The highlight, the timeline and the volume
    /// panel each set and clear their own, and in some orders one clears what
    /// another just set: `set_row` puts the timeline mark on and takes the
    /// button highlight off in the same breath. Deriving the answer in one
    /// place means whoever ran last cannot be the one who decides it.
    fn announce_current(&self) {
        let current: Option<gtk::Widget> = match self.row.get() {
            Row::None => None,
            Row::Buttons => self
                .order
                .get(self.focused.get())
                .map(|button| button.clone().upcast()),
            // The bar itself, because the scale must not take focus: its own
            // arrow bindings would move the playhead a pixel at a time
            // instead of seeking. Renamed below, so arriving here is still
            // announced as something rather than as the strip in general.
            Row::Timeline => Some(self.holder.clone().upcast()),
            // The row rather than the level inside it, so the output is named
            // as well as the number: "Headphones" is the part that says which
            // of the two you are about to change.
            // The row once one is taken hold of, and until then the button
            // that opened the panel - which is what is highlighted, and so
            // what a press is about to work.
            Row::Volume if !self.selected.get() => self
                .order
                .get(self.focused.get())
                .map(|button| button.clone().upcast()),
            Row::Volume => match self.current_level() {
                Level::Output(index, control) => self
                    .outputs
                    .get(index)
                    .map(|output| output.row_for(control).clone().upcast()),
                Level::Main => Some(self.main_row.clone().upcast()),
            },
            // Straight to the row, since a chooser opens with one marked: what
            // a press is about to take is the choice, not the icon.
            Row::Audio => self.outputs[self.output.get()]
                .track_rows
                .borrow()
                .get(self.current_track())
                .map(|row| row.clone().upcast()),
            // Straight to the row, since the chooser opens with one marked:
            // what a press is about to take is the choice, not the icon.
            Row::Subtitles => self
                .subtitle_rows
                .borrow()
                .get(self.subtitle_at.get())
                .map(|row| row.clone().upcast()),
        };

        name_it(
            &self.holder,
            &if self.row.get() == Row::Timeline {
                tr!("Playback position")
            } else {
                tr!("Playback controls")
            },
        );

        match current {
            Some(control) => {
                self.holder
                    .update_relation(&[gtk::accessible::Relation::ActiveDescendant(
                        match self.row.get() {
                            Row::Timeline => self.position.upcast_ref(),
                            _ => control.upcast_ref(),
                        },
                    )]);
                // The strip is very often still sliding into place when this
                // runs, and a widget part way through a transition will not
                // take focus. One retry after the frame settles, rather than
                // silently ending up with the focus still on the window.
                if !control.has_focus() && !control.grab_focus() {
                    let control = control.downgrade();
                    glib::idle_add_local_once(move || {
                        if let Some(control) = control.upgrade() {
                            control.grab_focus();
                        }
                    });
                }
            }
            // The relation goes and the focus is left alone. Taking it back to
            // the window by hand was wrong: the hide countdown ends here, so
            // every strip that timed out cleared the focus to the window and a
            // screen reader read the title bar. GTK already moves focus off a
            // widget when the revealer stops showing it.
            None => self
                .holder
                .reset_relation(gtk::AccessibleRelation::ActiveDescendant),
        }
    }

    fn highlight(&self, index: Option<usize>) {
        for (position, button) in self.order.iter().enumerate() {
            if Some(position) == index {
                button.add_css_class("tp-selected");
            } else {
                button.remove_css_class("tp-selected");
            }
        }
    }

    fn timeline_active(&self, active: bool) {
        if active {
            self.position.add_css_class("tp-selected");
        } else {
            self.position.remove_css_class("tp-selected");
        }
    }

    pub fn connect_play_pause(&self, handler: impl Fn() + 'static) {
        self.play.connect_clicked(move |_| handler());
    }

    /// Fires with the number of seconds to move, negative for backwards.
    pub fn connect_skip(&self, handler: impl Fn(f64) + 'static) {
        let handler = Rc::new(handler);
        {
            let handler = handler.clone();
            self.skip_back
                .connect_clicked(move |_| handler(-crate::player::STEP_SECONDS));
        }
        self.skip_forward
            .connect_clicked(move |_| handler(crate::player::STEP_SECONDS));
    }

    pub fn connect_stop(&self, handler: impl Fn() + 'static) {
        self.stop.connect_clicked(move |_| handler());
    }

    pub fn connect_back(&self, handler: impl Fn() + 'static) {
        self.back.connect_clicked(move |_| handler());
    }

    pub fn connect_fullscreen(&self, handler: impl Fn() + 'static) {
        self.fullscreen.connect_clicked(move |_| handler());
    }

    /// Told to show or hide whatever is already chosen. What holding the icon
    /// means; tapping it opens the chooser instead, which is why this is a
    /// stored handler rather than the button's click.
    pub fn connect_subtitles(&self, handler: impl Fn() + 'static) {
        *self.on_toggle_subtitles.borrow_mut() = Some(Box::new(handler));
    }

    /// Told which row of the chooser was picked. See [`SubtitleHandler`].
    pub fn connect_subtitle_chosen(&self, handler: impl Fn(usize) + 'static) {
        *self.on_subtitle.borrow_mut() = Some(Box::new(handler));
    }

    /// Reflects what subtitles are doing: unavailable when the video offers
    /// none at all, and dimmed while they are switched off, so the button says
    /// which state you are in rather than only offering a change.
    ///
    /// `available` is what the video *offers*, not what is playing. The button
    /// now opens a list that includes turning them on, so a film started with
    /// subtitles off must still be able to reach it - which asking whether
    /// anything is attached would refuse.
    pub fn set_subtitles(&self, available: bool, showing: bool) {
        self.subtitles.set_sensitive(available);
        if available && showing {
            self.subtitles.add_css_class("tp-subtitles-on");
        } else {
            self.subtitles.remove_css_class("tp-subtitles-on");
        }
    }

    /// Fires with the fraction of the file that was clicked or dragged to.
    pub fn connect_seek(self: &Rc<Self>, handler: impl Fn(f64) + 'static) {
        let controls = self.clone();
        self.position.connect_change_value(move |_, _, value| {
            if !controls.updating.get() {
                handler(value.clamp(0.0, 1.0));
            }
            glib::Propagation::Proceed
        });
    }

    /// Any pointer movement over the video brings the strip up.
    ///
    /// Movement means the pointer actually moved. A motion event is not proof
    /// of that: a pointer resting over the window still produces them, and
    /// each one would restart the countdown, so the strip stayed up forever
    /// with the mouse anywhere over the application. Seen on the Pi, where
    /// they arrive steadily; not on Windows, which is why it looked
    /// intermittent rather than constant.
    pub fn connect_motion(&self, handler: impl Fn() + 'static) {
        let motion = gtk::EventControllerMotion::new();
        let last = Cell::new((f64::NAN, f64::NAN));
        // Weak, because the controller this closure lives in is attached to
        // the very widget it holds.
        let root = self.root.downgrade();
        motion.connect_motion(move |_, x, y| {
            let (previous_x, previous_y) = last.get();
            let moved = (x - previous_x).hypot(y - previous_y);
            // A real movement, not any difference at all. Comparing for
            // inequality was enough on Linux and Windows but not on macOS,
            // which reports sub-pixel drift from a pointer nobody is touching:
            // the strip never timed out, and hiding it relaid out what sat
            // under the pointer, which produced another event and brought it
            // straight back.
            if moved.is_nan() || moved >= MOVEMENT {
                last.set((x, y));
                // The only thing that brings the pointer back. A key or a
                // gamepad press raises the strip without it: someone driving
                // from the sofa is not reaching for a mouse, and putting one
                // back on the picture every time they pause is the behaviour
                // being avoided.
                if let Some(root) = root.upgrade() {
                    root.set_cursor(None);
                }
                handler();
            }
        });
        self.root.add_controller(motion);
    }

    /// Double-clicking the picture toggles fullscreen, as it does in most
    /// players. Bubble phase, so a click landing on one of the controls
    /// belongs to that control and never reaches here.
    ///
    /// The strip is excluded by hand, because that only covers the buttons.
    /// Its background is not a widget that handles clicks, so a double click
    /// on the bar between the controls reaches the picture underneath and
    /// used to toggle fullscreen: an easy thing to hit while aiming for the
    /// scrubber, and a jarring result.
    pub fn connect_double_click(self: &Rc<Self>, handler: impl Fn() + 'static) {
        let controls = self.clone();
        let gesture = gtk::GestureClick::new();
        gesture.connect_pressed(move |_, presses, x, y| {
            if presses != 2 || controls.over_strip(x, y) {
                return;
            }
            handler();
        });
        self.root.add_controller(gesture);
    }

    /// Whether a point, in the coordinates of the widget the video sits in,
    /// falls on the control strip while it is up. Nothing is "on" a strip
    /// that is hidden, so those clicks belong to the picture.
    fn over_strip(&self, x: f64, y: f64) -> bool {
        if !self.strip.is_child_revealed() {
            return false;
        }
        let area = self.strip.allocation();
        let (left, top) = (f64::from(area.x()), f64::from(area.y()));
        x >= left
            && x < left + f64::from(area.width())
            && y >= top
            && y < top + f64::from(area.height())
    }

    pub fn set_fullscreen(&self, fullscreen: bool) {
        if *self.fullscreen_state.borrow() == fullscreen {
            return;
        }
        *self.fullscreen_state.borrow_mut() = fullscreen;
        self.fullscreen
            .set_child(Some(&crate::app::fullscreen_image(fullscreen, self.scale)));
    }

    /// Refreshes the readout. Cheap enough to call on a timer, since it is
    /// two pipeline queries and some label text.
    pub fn update(&self, playback: &Playback) {
        let position = playback.position().unwrap_or(gst::ClockTime::ZERO);
        let total = playback.duration();

        // Both readouts are held at the width of the longest they will get,
        // so the timeline between them keeps its size. Without it the bar
        // shrinks a little at 10:00 and again at an hour, and jitters as the
        // digits change width while scrubbing.
        let widest = total
            .filter(|total| *total > gst::ClockTime::ZERO)
            .map(|total| format_time(total).chars().count())
            .unwrap_or(5)
            .max(5) as i32;
        if self.elapsed.width_chars() != widest {
            self.elapsed.set_width_chars(widest);
            // One wider, for the minus sign a countdown carries, so switching
            // between the two does not resize anything either.
            self.duration.set_width_chars(widest + 1);
        }

        self.elapsed.set_text(&format_time(position));

        // Guarded, so writing the value back does not look like a drag.
        self.updating.set(true);
        match total {
            Some(total) if total > gst::ClockTime::ZERO => {
                self.duration.set_text(&if self.remaining.get() {
                    format!("-{}", format_time(total.saturating_sub(position)))
                } else {
                    format_time(total)
                });
                self.position
                    .set_value(position.nseconds() as f64 / total.nseconds() as f64);
            }
            // Live or still-parsing input: show elapsed and leave the bar
            // empty rather than inventing a proportion.
            _ => {
                self.duration.set_text("--:--");
                self.position.set_value(0.0);
            }
        }
        self.updating.set(false);

        // The icon names what pressing it will do, not what playback is
        // currently doing: a transport button showing "play" while a film
        // plays reads as a claim about the state, and the wrong one.
        self.icon.set_icon_name(Some(if playback.is_playing() {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        }));
    }

    /// Shows only where playback has reached, without the buttons.
    ///
    /// What a seek asks for: the timeline answers the question, and a row of
    /// buttons appearing over the picture every time somebody skips is more
    /// than was wanted.
    pub fn peek(self: &Rc<Self>) {
        // Unless a row is being driven, in which case the buttons are the
        // point and hiding them mid-navigation would be perverse.
        self.buttons.set_reveal_child(self.row.get() != Row::None);
        self.show(false);
    }

    /// Takes the strip off the screen at once, and lets go of it.
    pub fn hide(&self) {
        self.cancel();
        self.release();
        self.strip.set_reveal_child(false);
        // The pointer goes with it. This is the one path that takes the strip
        // down without the countdown, so nothing else would ever hide it: it
        // would sit on the picture until the mouse was moved, which is the
        // opposite of what asking for the strip to go away means.
        self.hide_pointer();
    }

    pub fn is_showing(&self) -> bool {
        self.strip.reveals_child()
    }

    /// Shows the whole strip: timeline and buttons both.
    pub fn flash(self: &Rc<Self>, paused: bool) {
        self.buttons.set_reveal_child(true);
        self.show(paused);
    }

    /// Brings the pointer back, wherever it was taken away.
    ///
    /// Public because leaving fullscreen has to call it: the pointer is hidden
    /// on a countdown that knows nothing about the window changing underneath
    /// it, and a windowed player with no pointer is one nobody can drive.
    pub fn reveal_pointer(&self) {
        self.root.set_cursor(None);
    }

    /// Takes the pointer off the picture once the strip has gone with it.
    ///
    /// Fullscreen only. A window sits on a desktop that the pointer belongs to
    /// as much as to us - there is a title bar above it and other windows
    /// behind - and one that vanishes while crossing an application is one
    /// somebody then has to go hunting for. Fullscreen is the case where the
    /// picture is all there is, and a pointer resting over it is just
    /// something left on screen.
    fn hide_pointer(&self) {
        let fullscreen = self
            .root
            .root()
            .and_downcast::<gtk::Window>()
            .is_some_and(|window| window.is_fullscreen());
        if fullscreen {
            self.root.set_cursor_from_name(Some("none"));
        }
    }

    /// Puts the strip on screen and starts the countdown to taking it off
    /// again. What is in it has already been decided by the caller.
    ///
    /// Paused playback keeps it up indefinitely, because a paused picture with
    /// no indication of why is just a frozen film.
    fn show(self: &Rc<Self>, paused: bool) {
        self.strip.set_reveal_child(true);

        let expected = self.generation.get().wrapping_add(1);
        self.generation.set(expected);
        // Paused, or with either panel open. Lining an output up against the
        // picture means listening for a while and changing nothing, which is
        // exactly what the hide timer reads as having wandered off - and
        // having the strip vanish mid-adjustment loses the row you were on.
        // Reading down a list of languages is the same kind of pause.
        // Closing either comes back through here and starts the countdown
        // again.
        if paused || matches!(self.row.get(), Row::Volume | Row::Subtitles) {
            return;
        }

        let linger = if self.row.get() == Row::None {
            LINGER
        } else {
            LINGER_HELD
        };
        let generation = Rc::clone(&self.generation);
        // Hiding lets go of the strip as well. Without that it would come back
        // still holding whichever row it had, so the next press up would climb
        // from there rather than starting at the buttons.
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(linger, move || {
            let Some(controls) = weak.upgrade() else {
                return;
            };
            if generation.get() == expected {
                controls.strip.set_reveal_child(false);
                controls.hide_pointer();
                controls.release();
            }
        });
    }

    /// Retires any pending hide, so a torn-down playback leaves no timer
    /// touching a widget that is going away.
    pub fn cancel(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
    }
}

/// `M:SS` under an hour, `H:MM:SS` beyond it, so a typical film reads at a
/// glance without a leading zero hour.
pub fn format_time(time: gst::ClockTime) -> String {
    let total = time.seconds();
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}
