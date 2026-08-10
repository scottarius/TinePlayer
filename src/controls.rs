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
    /// The volume panel is open. Up and down choose which output, left and
    /// right move that output's level.
    Volume,
}

/// Told which output changed, where its level now stands as a fraction of
/// full, whether it is silenced, and whether the change is worth keeping.
/// Silencing everything at once is not: it lasts the session, the way the
/// subtitle toggle does, and a film that started silent because of a door
/// knocked on last week would be a bug rather than a memory.
type VolumeHandler = Box<dyn Fn(&str, f64, bool, bool)>;

/// Told which output was shifted and by how many milliseconds. Separate from
/// [`VolumeHandler`] because a delay is always worth keeping: unlike silencing
/// everything at once, it describes the equipment rather than the moment.
type SyncHandler = Box<dyn Fn(&str, f64, bool)>;

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

/// One output's controls inside the volume panel.
struct Output {
    role: &'static str,
    /// The heading and both rows, hidden together when this output is not in
    /// use.
    group: gtk::Box,
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
    muted: Cell<bool>,
    /// What this output was doing before everything was silenced at once, so
    /// that letting go of it puts back what was there rather than unmuting an
    /// output somebody had deliberately turned off.
    before_hush: Cell<bool>,
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

/// Volume's place in the same order. The panel keeps it highlighted while it
/// is open, so it is clear where closing the panel goes back to.
const VOLUME: usize = 6;

/// How long the volume button has to be held for it to mean "silence
/// everything" rather than "show me the levels". Long enough not to fire
/// under an ordinary press, short enough to be a deliberate gesture rather
/// than a wait.
pub const HOLD: Duration = Duration::from_millis(600);

/// How far one press moves a level. Twenty steps across the range: coarse
/// enough to cross it in a second of held input, fine enough to settle on a
/// level rather than overshoot it.
const VOLUME_STEP: f64 = 0.05;

/// How wide the panel is, before scaling. A minimum rather than a fixed
/// width, but with the device names now cut to fit rather than stretching it,
/// it is what the panel actually comes out at.
const PANEL_WIDTH: f64 = 420.0;

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
    settings: gtk::Button,
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
    /// The volume panel, and which output within it is selected.
    panel: gtk::Revealer,
    outputs: Vec<Output>,
    output: Cell<usize>,
    /// Which of the selected output's two controls is being driven. Together
    /// with `output` this is the position in the panel: up and down step
    /// through every control of every output in turn.
    control: Cell<Control>,
    /// The volume button's own icon, which reports everything being silenced
    /// at once: the panel it opens says so too, but not while it is closed.
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
        name_it(&play, "Play or pause");

        // go-* rather than media-seek-*: the seek glyphs are absent from the
        // GTK that ships with GStreamer on Windows, and a missing icon draws
        // as a broken-image box. These are plain arrows, which is less
        // expressive than a skip glyph but present everywhere.
        let back_icon = gtk::Image::from_icon_name("go-previous-symbolic");
        back_icon.add_css_class("tp-transport");
        let skip_back = gtk::Button::new();
        skip_back.set_child(Some(&back_icon));
        skip_back.add_css_class("tp-transport-button");
        name_it(&skip_back, "Skip back");

        let forward_icon = gtk::Image::from_icon_name("go-next-symbolic");
        forward_icon.add_css_class("tp-transport");
        let skip_forward = gtk::Button::new();
        skip_forward.set_child(Some(&forward_icon));
        skip_forward.add_css_class("tp-transport-button");
        name_it(&skip_forward, "Skip forward");

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
        name_it(&fullscreen, "Toggle fullscreen");
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
        name_it(&subtitles, "Show or hide subtitles");
        subtitles.set_sensitive(false);

        // A panel rather than a slider in the bar: two outputs need naming,
        // and a pair of bare sliders cannot say which is the speakers and
        // which the headphones - which is the whole distinction here.
        let volume_icon = gtk::Image::from_icon_name("audio-volume-high-symbolic");
        volume_icon.add_css_class("tp-transport");
        let volume = gtk::Button::new();
        volume.set_child(Some(&volume_icon));
        volume.add_css_class("tp-transport-button");
        name_it(&volume, "Volume");

        let panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        panel.add_css_class("tp-volume-panel");
        let mut rows = Vec::new();
        for (role, name) in outputs {
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
            name_it(&sync, &format!("Audio sync, {name}"));

            // The same shape as the row above: a button the width of the
            // mute one, then the bar. Pressing it puts the output back in
            // sync, which is the one value somebody is ever certain they
            // want and the one that stepping in tens cannot reach from an
            // arbitrary place a pointer left it in.
            let sync_toggle = gtk::Button::new();
            sync_toggle.set_child(Some(&crate::app::sync_image(scale)));
            sync_toggle.add_css_class("tp-transport-button");
            sync_toggle.set_can_focus(false);
            name_it(&sync_toggle, &format!("Use audio sync, {name}"));

            let sync_reading = reading_label(&crate::app::offset_label(0.0));

            // The same spacing as the row above, not just the same widgets:
            // the two bars sit one under the other, and gaps of different
            // sizes either side leave them visibly different lengths.
            let sync_row = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(ROW_SPACING)
                .build();
            sync_row.set_focusable(true);
            name_it(&sync_row, &format!("Audio sync, {name}"));
            sync_row.append(&sync_toggle);
            sync_row.append(&sync);
            sync_row.append(&sync_reading);

            let group = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(4)
                .build();
            group.append(&label);
            group.append(&row);
            group.append(&sync_row);
            panel.append(&group);

            rows.push(Output {
                role,
                group,
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
                before_hush: Cell::new(false),
            });
        }

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

        // Away from the transport controls, beside the other things that are
        // not about what playback is doing: it leaves playback rather than
        // changing it.
        let settings_icon = crate::app::settings_image(crate::app::ICON_PX * scale);
        settings_icon.add_css_class("tp-transport");
        let settings = gtk::Button::new();
        settings.set_child(Some(&settings_icon));
        settings.add_css_class("tp-transport-button");
        name_it(&settings, "Settings");

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
        // first. Stop keeps beside it, being the other thing that acts on
        // playback itself. What is left goes to the edges: the two that change
        // how the video is presented on the left, the one that changes how
        // much of the screen it takes on the right.
        let left = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(16)
            .build();
        left.append(&settings);
        left.append(&stop);

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
        right.append(&volume);
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
        row.append(&panel_reveal);
        row.append(&bar);
        // Takes the focus for everything inside it. Nothing else in the strip
        // can hold it, which is what makes this the one place to put it.
        row.set_focusable(true);
        name_it(&row, "Playback controls");

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

        let root = gtk::Overlay::new();
        root.set_child(Some(video));
        root.add_overlay(&strip);

        let order = vec![
            settings.clone(),
            stop.clone(),
            skip_back.clone(),
            play.clone(),
            skip_forward.clone(),
            subtitles.clone(),
            volume.clone(),
            fullscreen.clone(),
        ];

        let controls = Rc::new(Self {
            root,
            strip,
            holder: row.clone(),
            buttons: button_row.clone(),
            icon,
            play,
            stop,
            skip_back,
            skip_forward,
            settings,
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
            panel: panel_reveal,
            outputs: rows,
            output: Cell::new(0),
            control: Cell::new(Control::Volume),
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
        {
            let volume_button = volume.clone();
            let handle = controls.clone();
            volume_button.connect_clicked(move |_| {
                if handle.swallow_click.replace(false) {
                    return;
                }
                if handle.row.get() == Row::Volume {
                    handle.set_row(Row::Buttons);
                } else {
                    handle.open_volume(false);
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
                    gdk::EventType::ButtonPress => handle.press_volume(),
                    gdk::EventType::ButtonRelease if !handle.release_volume() => {
                        handle.swallow_click.set(true);
                    }
                    _ => {}
                }
                glib::Propagation::Proceed
            });
            volume.add_controller(controller);
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
        for output in &self.outputs {
            match levels.iter().find(|(role, _, _)| *role == output.role) {
                Some(&(_, level, muted)) => {
                    output.group.set_visible(true);
                    output.level.set_value(level);
                    output
                        .level_reading
                        .set_text(&crate::app::volume_label(level, muted));
                    output.muted.set(muted);
                    Self::draw_mute(&output.mute, level, muted);
                }
                None => output.group.set_visible(false),
            }
        }
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
            self.select_output_row(None);
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
                self.focused.set(VOLUME);
                self.highlight(Some(VOLUME));
                // The row nearest the button, since the panel opens upward
                // from it: going up the list, the first row reached is the one
                // at the bottom of it, which is the last output's sync.
                //
                // Also where `at_last_output` measures from, so moving down
                // from here leaves the panel rather than stepping within it.
                self.output.set(self.last_output());
                self.control.set(Control::Sync);
                self.selected.set(false);
                self.select_output_row(None);
                self.panel.set_reveal_child(true);
                self.flash(false);
            }
        }
        // After the arm, not inside it: every one of them has just settled
        // where the strip is, and Row::None has already let go through hide().
        self.announce_current();
    }

    /// Opens the panel, marking an output only when something that cannot
    /// point at one is driving it.
    pub fn open_volume(self: &Rc<Self>, selected: bool) {
        self.set_row(Row::Volume);
        if selected {
            self.selected.set(true);
            self.select_output();
        }
    }

    /// Swaps the right-hand readout between the length and what is left of
    /// it, the same as clicking it does. Peeks the strip on the way, since
    /// otherwise a press that only changes a hidden readout looks like a
    /// press that did nothing.
    pub fn toggle_remaining(self: &Rc<Self>) {
        self.remaining.set(!self.remaining.get());
        self.peek();
    }

    /// Whether a press on the volume button should be held rather than acted
    /// on at once. Only from the button row: inside the panel the same press
    /// silences the one output it is pointing at, and holding it there would
    /// be two meanings on one button.
    pub fn holds_press(&self) -> bool {
        self.row.get() == Row::Buttons && self.focused.get() == VOLUME
    }

    /// Starts a hold. Repeats are ignored: a keyboard sends a press over and
    /// over while a key is down, and restarting the timer on each one would
    /// mean it never finished.
    pub fn press_volume(self: &Rc<Self>) {
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
            handle.toggle_hush();
        });
    }

    /// Ends a hold, and says whether the release should still do the ordinary
    /// thing - which it should not, if the hold already did something else.
    pub fn release_volume(self: &Rc<Self>) -> bool {
        self.holding.set(false);
        self.hold.set(self.hold.get() + 1);
        !self.held.replace(false)
    }

    /// Silences every output at once, or puts back what each was doing
    /// before. For the moment somebody knocks at the door: two outputs means
    /// two things to silence, and reaching into the panel for both of them is
    /// what this is instead of.
    pub fn toggle_hush(self: &Rc<Self>) {
        if !self.release_hush() {
            for output in &self.outputs {
                output.before_hush.set(output.muted.get());
            }
            self.hushed.set(true);
            for index in 0..self.outputs.len() {
                self.outputs[index].muted.set(true);
                let output = &self.outputs[index];
                Self::draw_mute(&output.mute, output.level.value(), true);
                self.report(index, false);
            }
            self.draw_hush();
        }
        self.flash(false);
    }

    /// Makes the silence the real state rather than a layer over one, and
    /// keeps it. Reaching into the panel while everything is hushed goes
    /// through here first: what is on screen is that everything is muted, so
    /// that is what a press should be acting on. Restoring first would mean
    /// unmuting an output only to have the press mute it straight back, which
    /// looks like a control that does nothing.
    fn absorb_hush(&self) {
        if !self.hushed.replace(false) {
            return;
        }
        for index in 0..self.outputs.len() {
            self.report(index, true);
        }
        self.draw_hush();
    }

    /// Puts back what each output was doing before everything was silenced,
    /// and says whether there was anything to put back. This is what a second
    /// hold does; a press inside the panel absorbs the silence instead.
    fn release_hush(&self) -> bool {
        if !self.hushed.replace(false) {
            return false;
        }
        for index in 0..self.outputs.len() {
            let muted = self.outputs[index].before_hush.get();
            self.outputs[index].muted.set(muted);
            let output = &self.outputs[index];
            Self::draw_mute(&output.mute, output.level.value(), muted);
            self.report(index, false);
        }
        self.draw_hush();
        true
    }

    fn draw_hush(&self) {
        self.volume_icon.set_icon_name(Some(if self.hushed.get() {
            "audio-volume-muted-symbolic"
        } else {
            "audio-volume-high-symbolic"
        }));
    }

    /// Points the panel at an output without marking it, which is what a
    /// pointer does. If a mark is already showing it follows along, so the
    /// two ways of driving it do not disagree about where it is.
    fn aim_at_output(&self, index: usize) {
        self.aim_at(index, Control::Volume);
    }

    /// Points the panel at one control of one output, which is what a pointer
    /// touching a slider means: it is about to change that one, so the
    /// keyboard should carry on from there rather than from wherever it was.
    fn aim_at(&self, index: usize, control: Control) {
        self.output.set(index);
        self.control.set(control);
        if self.selected.get() {
            self.select_output();
        }
    }

    /// The lowest row the panel is showing, which is where it starts.
    fn last_output(&self) -> usize {
        self.outputs
            .iter()
            .rposition(|output| output.group.is_visible())
            .unwrap_or(0)
    }

    /// The lowest place in the panel, counted through every control. Distinct
    /// from [`Self::last_output`], which is an output: conflating the two made
    /// the panel start at a position no output has, so nothing moved.
    fn last_position(&self) -> usize {
        self.last_output() * 2 + 1
    }

    /// Whether the selected output is the lowest one, which is what makes a
    /// downward press leave the panel rather than move within it. Never while
    /// nothing is marked, so the first press takes hold of the panel instead
    /// of closing it.
    pub fn at_last_output(&self) -> bool {
        self.selected.get() && self.position() >= self.last_position()
    }

    /// Moves between outputs, stopping at either end rather than wrapping,
    /// the same way the button row does.
    pub fn move_output(self: &Rc<Self>, delta: isize) {
        if self.row.get() != Row::Volume {
            return;
        }
        // The first press marks where the panel already is, rather than
        // moving from an unmarked place nobody can see.
        if !self.selected.replace(true) {
            self.select_output();
            self.flash(false);
            return;
        }
        // Every control of every output in turn, so that moving down from an
        // output's volume lands on its own sync rather than skipping to the
        // next device. Counted rather than held as a flat list: the outputs
        // are the thing everything else here is indexed by, and a second
        // ordering to keep in step would be one more thing to get wrong.
        let mut position = self.position() as isize;
        loop {
            position += delta;
            if position < 0 || position as usize >= self.outputs.len() * 2 {
                return;
            }
            let (index, control) = Self::at(position as usize);
            if self.outputs[index].group.is_visible() {
                self.output.set(index);
                self.control.set(control);
                break;
            }
        }
        self.select_output();
        self.flash(false);
    }

    /// Where the panel is now, counted through every control of every output.
    fn position(&self) -> usize {
        self.output.get() * 2 + usize::from(self.control.get() == Control::Sync)
    }

    /// The output and control at a counted position.
    fn at(position: usize) -> (usize, Control) {
        (
            position / 2,
            if position.is_multiple_of(2) {
                Control::Volume
            } else {
                Control::Sync
            },
        )
    }

    fn select_output(&self) {
        self.select_output_row(Some(self.position()));
        // Everything that moves within the panel comes through here, both
        // the first press that takes hold of it and every one after.
        self.announce_current();
    }

    /// Marks one control of one output, by counted position, and clears every
    /// other. `None` clears them all.
    fn select_output_row(&self, position: Option<usize>) {
        for (index, output) in self.outputs.iter().enumerate() {
            for control in [Control::Volume, Control::Sync] {
                let this = index * 2 + usize::from(control == Control::Sync);
                let row = output.row_for(control);
                if Some(this) == position {
                    row.add_css_class("tp-selected");
                } else {
                    row.remove_css_class("tp-selected");
                }
            }
        }
    }

    /// Moves the selected output's level, while the panel is open.
    pub fn adjust_level(self: &Rc<Self>, delta: isize) {
        if self.row.get() != Row::Volume {
            return;
        }
        let index = self.output.get();
        let Some(output) = self.outputs.get(index) else {
            return;
        };
        match self.control.get() {
            Control::Volume => {
                let level = (output.level.value() + delta as f64 * VOLUME_STEP).clamp(0.0, 1.0);
                output.level.set_value(level);
                self.set_level(index, level);
            }
            Control::Sync => self.shift_by(index, delta),
        }
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
    fn set_level(self: &Rc<Self>, index: usize, level: f64) {
        self.absorb_hush();
        let Some(output) = self.outputs.get(index) else {
            return;
        };
        let muted = output.muted.get() && level <= 0.0;
        output.muted.set(muted);
        output
            .level_reading
            .set_text(&crate::app::volume_label(level, muted));
        Self::draw_mute(&output.mute, level, muted);
        self.report(index, true);
        self.flash(false);
    }

    /// Silences one output, or lets it go. Reads the state after any blanket
    /// silence has been lifted, so that pressing mute on an output that was
    /// already unmuted before the hush mutes it rather than appearing to do
    /// nothing.
    fn toggle_muted(self: &Rc<Self>, index: usize) {
        self.absorb_hush();
        let Some(output) = self.outputs.get(index) else {
            return;
        };
        let muted = !output.muted.get();
        output.muted.set(muted);
        Self::draw_mute(&output.mute, output.level.value(), muted);
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

    /// The icon says how loud the output is, not only whether it is silenced,
    /// so the panel can be read at a glance from across a room.
    fn draw_mute(button: &gtk::Button, level: f64, muted: bool) {
        let name = if muted || level <= 0.0 {
            "audio-volume-muted-symbolic"
        } else if level < 0.34 {
            "audio-volume-low-symbolic"
        } else if level < 0.67 {
            "audio-volume-medium-symbolic"
        } else {
            "audio-volume-high-symbolic"
        };
        button.set_icon_name(name);
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
            self.select_output_row(None);
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
            // Volume goes its own way rather than through the button's click
            // handler: that handler cannot tell a press from a pointer, and
            // this is the one control where the difference shows.
            // Opened with the button still the thing highlighted, and
            // nothing inside it marked. Marking a row straight away meant the
            // press that opened the panel was followed by one that worked a
            // control nobody had moved to yet.
            Row::Buttons if self.focused.get() == VOLUME => self.open_volume(false),
            Row::Buttons => {
                if let Some(button) = self.order.get(self.focused.get()) {
                    button.emit_clicked();
                }
            }
            // In the panel it is the button belonging to the selected row,
            // and both are the same gesture: mute on a volume row, use the
            // delay or not on a sync row. Neither loses what it is turning
            // off - the level and the delay both stay where they were.
            // Nothing inside the panel taken hold of yet: the button is
            // still what is highlighted, so pressing it again shuts what it
            // opened - the same as clicking it a second time.
            Row::Volume if !self.selected.get() => self.set_row(Row::Buttons),
            Row::Volume => match self.control.get() {
                Control::Volume => self.toggle_muted(self.output.get()),
                Control::Sync => self.toggle_sync(self.output.get()),
            },
            _ => {}
        }
    }

    /// Whether a press belongs to the strip rather than to playback.
    pub fn takes_activation(&self) -> bool {
        matches!(self.row.get(), Row::Buttons | Row::Volume)
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
            Row::Volume => self
                .outputs
                .get(self.output.get())
                .map(|output| output.row_for(self.control.get()).clone().upcast()),
        };

        name_it(
            &self.holder,
            if self.row.get() == Row::Timeline {
                "Playback position"
            } else {
                "Playback controls"
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

    pub fn connect_settings(&self, handler: impl Fn() + 'static) {
        self.settings.connect_clicked(move |_| handler());
    }

    pub fn connect_fullscreen(&self, handler: impl Fn() + 'static) {
        self.fullscreen.connect_clicked(move |_| handler());
    }

    pub fn connect_subtitles(&self, handler: impl Fn() + 'static) {
        self.subtitles.connect_clicked(move |_| handler());
    }

    /// Reflects what subtitles are doing: unavailable when the file has none
    /// selected, and dimmed while they are switched off, so the button says
    /// which state you are in rather than only offering a change.
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
        // Paused, or with the audio panel open. Lining an output up against
        // the picture means listening for a while and changing nothing, which
        // is exactly what the hide timer reads as having wandered off - and
        // having the strip vanish mid-adjustment loses the row you were on.
        // Closing the panel comes back through here and starts the countdown
        // again.
        if paused || self.row.get() == Row::Volume {
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
