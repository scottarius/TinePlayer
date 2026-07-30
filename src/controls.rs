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
    elapsed: gtk::Label,
    duration: gtk::Label,
    position: gtk::Scale,
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
        let icon = gtk::Image::from_icon_name("media-playback-start-symbolic");
        icon.add_css_class("tp-transport");
        let play = gtk::Button::new();
        play.set_child(Some(&icon));
        play.add_css_class("tp-transport-button");
        play.set_can_focus(false);

        let elapsed = gtk::Label::new(Some("0:00"));
        elapsed.add_css_class("tp-time");
        let duration = gtk::Label::new(Some("0:00"));
        duration.add_css_class("tp-time");

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

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(16)
            .build();
        row.add_css_class("tp-controls");
        row.append(&play);
        row.append(&elapsed);
        row.append(&position);
        row.append(&duration);
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
            elapsed,
            duration,
            position,
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

    pub fn connect_fullscreen(&self, handler: impl Fn() + 'static) {
        self.fullscreen.connect_clicked(move |_| handler());
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
    pub fn connect_motion(&self, handler: impl Fn() + 'static) {
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(move |_, _, _| handler());
        self.root.add_controller(motion);
    }

    /// Double-clicking the picture toggles fullscreen, as it does in most
    /// players. Bubble phase, so a click landing on one of the controls
    /// belongs to that control and never reaches here.
    pub fn connect_double_click(&self, handler: impl Fn() + 'static) {
        let gesture = gtk::GestureClick::new();
        gesture.connect_pressed(move |_, presses, _, _| {
            if presses == 2 {
                handler();
            }
        });
        self.root.add_controller(gesture);
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

        self.elapsed.set_text(&format_time(position));

        // Guarded, so writing the value back does not look like a drag.
        self.updating.set(true);
        match total {
            Some(total) if total > gst::ClockTime::ZERO => {
                self.duration.set_text(&format_time(total));
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

        self.icon.set_icon_name(Some(if playback.is_playing() {
            "media-playback-start-symbolic"
        } else {
            "media-playback-pause-symbolic"
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
