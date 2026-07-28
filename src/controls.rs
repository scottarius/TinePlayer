//! The strip of playback information laid over the video: where you are, how
//! long the file is, and whether it is running.
//!
//! Deliberately holds no focus. During playback there is nothing to choose
//! between, so making the scrubber a focusable widget would only introduce a
//! focus state to get wrong; seeking is driven by left and right from either
//! the keyboard or a controller, and the strip simply reports the result.
//! That also keeps it working when the video surface has keyboard focus,
//! which is exactly where focus-based schemes have caused trouble before.

use std::cell::Cell;
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
    elapsed: gtk::Label,
    duration: gtk::Label,
    progress: gtk::ProgressBar,
    /// Bumped every time the strip is shown. A pending hide captures the
    /// value it was scheduled under and does nothing if it no longer matches,
    /// which is what stops repeated seeks from hiding the strip three seconds
    /// after the *first* one.
    ///
    /// Preferred over cancelling the timer by id: a source that has already
    /// fired cannot be removed, and trying logs a GLib critical.
    generation: Rc<Cell<u64>>,
}

impl Controls {
    pub fn new(video: &gtk::Picture) -> Rc<Self> {
        let icon = gtk::Image::from_icon_name("media-playback-start-symbolic");
        icon.add_css_class("tp-transport");
        let elapsed = gtk::Label::new(Some("0:00"));
        elapsed.add_css_class("tp-time");
        let duration = gtk::Label::new(Some("0:00"));
        duration.add_css_class("tp-time");

        let progress = gtk::ProgressBar::builder()
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        progress.add_css_class("tp-progress");

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(16)
            .build();
        row.add_css_class("tp-controls");
        row.append(&icon);
        row.append(&elapsed);
        row.append(&progress);
        row.append(&duration);

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
            elapsed,
            duration,
            progress,
            generation: Rc::new(Cell::new(0)),
        })
    }

    pub fn widget(&self) -> &gtk::Overlay {
        &self.root
    }

    /// Refreshes the readout. Cheap enough to call on a timer, since it is
    /// two pipeline queries and some label text.
    pub fn update(&self, playback: &Playback) {
        let position = playback.position().unwrap_or(gst::ClockTime::ZERO);
        let total = playback.duration();

        self.elapsed.set_text(&format_time(position));
        match total {
            Some(total) if total > gst::ClockTime::ZERO => {
                self.duration.set_text(&format_time(total));
                self.progress
                    .set_fraction(position.nseconds() as f64 / total.nseconds() as f64);
            }
            // Live or still-parsing input: show elapsed and leave the bar
            // empty rather than inventing a proportion.
            _ => {
                self.duration.set_text("--:--");
                self.progress.set_fraction(0.0);
            }
        }

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
fn format_time(time: gst::ClockTime) -> String {
    let total = time.seconds();
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}
