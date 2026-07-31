//! The strip of playback controls laid over the video: where you are, how
//! long the file is, whether it is running, and the means to change all three.
//!
//! Nothing here takes keyboard focus. A controller drives playback through the
//! same actions the keyboard does, so making these focusable would only add a
//! focus state to get wrong, and focus on the video surface has caused trouble
//! before. They are all reachable with a pointer, which is what they are for.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gstreamer as gst;
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

pub struct Controls {
    root: gtk::Overlay,
    strip: gtk::Revealer,
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
    /// Tracked here rather than through GTK focus: the video surface taking
    /// focus has caused trouble before, and this way an insensitive button is
    /// simply skipped rather than needing to be made unfocusable and back.
    focused: Cell<usize>,
    row: Cell<Row>,
    /// Kept so the fullscreen mark can be redrawn when the state changes.
    scale: f64,
    dark: bool,
    fullscreen_state: RefCell<bool>,
}

impl Controls {
    pub fn new(video: &gtk::Picture, scale: f64, dark: bool, fullscreen_now: bool) -> Rc<Self> {
        // Pause, because playback begins playing. The readout corrects this
        // on its first tick anyway, but half a second of the wrong icon is
        // half a second of it looking stopped.
        let icon = gtk::Image::from_icon_name("media-playback-pause-symbolic");
        icon.add_css_class("tp-transport");
        icon.add_css_class("tp-transport-main");
        let play = gtk::Button::new();
        play.set_child(Some(&icon));
        play.add_css_class("tp-transport-button");
        play.set_can_focus(false);

        // go-* rather than media-seek-*: the seek glyphs are absent from the
        // GTK that ships with GStreamer on Windows, and a missing icon draws
        // as a broken-image box. These are plain arrows, which is less
        // expressive than a skip glyph but present everywhere.
        let back_icon = gtk::Image::from_icon_name("go-previous-symbolic");
        back_icon.add_css_class("tp-transport");
        let skip_back = gtk::Button::new();
        skip_back.set_child(Some(&back_icon));
        skip_back.add_css_class("tp-transport-button");
        skip_back.set_can_focus(false);

        let forward_icon = gtk::Image::from_icon_name("go-next-symbolic");
        forward_icon.add_css_class("tp-transport");
        let skip_forward = gtk::Button::new();
        skip_forward.set_child(Some(&forward_icon));
        skip_forward.add_css_class("tp-transport-button");
        skip_forward.set_can_focus(false);

        // Beside play, because they are the same kind of thing: what playback
        // is doing right now.
        let stop_icon = gtk::Image::from_icon_name("media-playback-stop-symbolic");
        stop_icon.add_css_class("tp-transport");
        let stop = gtk::Button::new();
        stop.set_child(Some(&stop_icon));
        stop.add_css_class("tp-transport-button");
        stop.set_can_focus(false);

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

        let fullscreen = gtk::Button::new();
        fullscreen.set_child(Some(&crate::app::fullscreen_image(
            fullscreen_now,
            scale,
            dark,
        )));
        fullscreen.add_css_class("tp-transport-button");
        fullscreen.set_can_focus(false);

        // A bundled image rather than a themed icon name: no subtitle glyph
        // ships with GTK on Windows, and a missing icon draws as a
        // broken-image box.
        let subtitles = gtk::Button::new();
        subtitles.set_child(Some(&crate::app::subtitles_image(scale)));
        subtitles.add_css_class("tp-transport-button");
        subtitles.add_css_class("tp-subtitles-button");
        subtitles.set_can_focus(false);
        subtitles.set_sensitive(false);

        // Away from the transport controls, beside the other things that are
        // not about what playback is doing: it leaves playback rather than
        // changing it.
        let settings_icon = gtk::Image::from_icon_name("emblem-system-symbolic");
        settings_icon.add_css_class("tp-transport");
        let settings = gtk::Button::new();
        settings.set_child(Some(&settings_icon));
        settings.add_css_class("tp-transport-button");
        settings.set_can_focus(false);

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
        // Subtitles keep company with fullscreen, and with volume when that
        // arrives: they are all about how the video is presented, where the
        // left-hand pair is about leaving it or changing it.
        let right = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(16)
            .build();
        right.append(&subtitles);
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

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .build();
        row.add_css_class("tp-controls");
        row.append(&timeline);
        row.append(&button_row);

        // Slides up rather than appearing, which reads as deliberate at a
        // distance where a sudden change is just a flicker.
        let strip = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .transition_duration(150)
            .valign(gtk::Align::End)
            .child(&row)
            .build();

        button_row.set_reveal_child(true);

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
            fullscreen.clone(),
        ];

        Rc::new(Self {
            root,
            strip,
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
            scale,
            dark,
            fullscreen_state: RefCell::new(fullscreen_now),
        })
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
        }
    }

    fn usable(&self, index: usize) -> bool {
        self.order
            .get(index)
            .is_some_and(|button| button.is_sensitive())
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
        self.row.set(Row::None);
        self.highlight(None);
        self.timeline_active(false);
    }

    pub fn move_focus(self: &Rc<Self>, delta: isize) {
        if self.row.get() != Row::Buttons {
            return;
        }
        self.step(delta);
        self.highlight(Some(self.focused.get()));
        // Restarts the countdown, so working along the row does not run out
        // of time part way.
        self.flash(false);
    }

    pub fn activate_focused(&self) {
        if self.row.get() != Row::Buttons {
            return;
        }
        if let Some(button) = self.order.get(self.focused.get()) {
            button.emit_clicked();
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
            .set_child(Some(&crate::app::fullscreen_image(
                fullscreen, self.scale, self.dark,
            )));
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

    /// Shows the strip and restarts the countdown to hiding it. Paused
    /// playback keeps it up indefinitely, because a paused picture with no
    /// indication of why is just a frozen film.
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
    }

    pub fn is_showing(&self) -> bool {
        self.strip.reveals_child()
    }

    /// Shows the whole strip: timeline and buttons both.
    pub fn flash(self: &Rc<Self>, paused: bool) {
        self.buttons.set_reveal_child(true);
        self.show(paused);
    }

    /// Puts the strip on screen and starts the countdown to taking it off
    /// again. What is in it has already been decided by the caller.
    fn show(self: &Rc<Self>, paused: bool) {
        self.strip.set_reveal_child(true);

        let expected = self.generation.get().wrapping_add(1);
        self.generation.set(expected);
        if paused {
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
