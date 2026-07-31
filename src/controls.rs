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

/// How long the strip stays up after the last input. Long enough to read a
/// timestamp after a seek, short enough not to sit over the picture.
const LINGER: Duration = Duration::from_secs(3);

pub struct Controls {
    root: gtk::Overlay,
    strip: gtk::Revealer,
    icon: gtk::Image,
    play: gtk::Button,
    stop: gtk::Button,
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
        let play = gtk::Button::new();
        play.set_child(Some(&icon));
        play.add_css_class("tp-transport-button");
        play.set_can_focus(false);

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

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(16)
            .build();
        row.add_css_class("tp-controls");
        row.append(&play);
        row.append(&stop);
        row.append(&elapsed);
        row.append(&position);
        row.append(&duration);
        row.append(&subtitles);
        row.append(&settings);
        row.append(&fullscreen);

        // Slides up rather than appearing, which reads as deliberate at a
        // distance where a sudden change is just a flicker.
        let strip = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .transition_duration(150)
            .valign(gtk::Align::End)
            .child(&row)
            .build();

        let root = gtk::Overlay::new();
        root.set_child(Some(video));
        root.add_overlay(&strip);

        Rc::new(Self {
            root,
            strip,
            icon,
            play,
            stop,
            settings,
            elapsed,
            duration,
            remaining,
            position,
            subtitles,
            fullscreen,
            updating: Cell::new(false),
            generation: Rc::new(Cell::new(0)),
            scale,
            dark,
            fullscreen_state: RefCell::new(fullscreen_now),
        })
    }

    pub fn widget(&self) -> &gtk::Overlay {
        &self.root
    }

    pub fn connect_play_pause(&self, handler: impl Fn() + 'static) {
        self.play.connect_clicked(move |_| handler());
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
            if last.replace((x, y)) == (x, y) {
                return;
            }
            handler();
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
    pub fn flash(&self, paused: bool) {
        self.strip.set_reveal_child(true);

        let expected = self.generation.get().wrapping_add(1);
        self.generation.set(expected);
        if paused {
            return;
        }

        let strip = self.strip.clone();
        let generation = Rc::clone(&self.generation);
        glib::timeout_add_local_once(LINGER, move || {
            if generation.get() == expected {
                strip.set_reveal_child(false);
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
