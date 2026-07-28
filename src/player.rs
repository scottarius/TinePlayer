use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gstreamer as gst;
use gst::prelude::*;
use gtk::{gdk, glib};

use crate::config::{clear_position, load_positions, save_position, Config};
use crate::pipeline::build_pipeline;

/// Applied to the video widget so the letterbox area around the picture
/// can be styled black without affecting the rest of the interface.
pub const VIDEO_CSS_CLASS: &str = "video-surface";

/// Key auto-repeat delivers a fresh press event for as long as a key is
/// held, and GTK's key controller doesn't distinguish a repeat from a
/// genuine new press. Debouncing is what keeps "hold space" from
/// rapid-fire toggling play/pause.
const TOGGLE_DEBOUNCE: Duration = Duration::from_millis(300);

/// An in-progress playback: the pipeline, plus the widget its video is
/// drawn into. Single-threaded — every GTK callback runs on the main
/// thread — so `Rc`/`Cell` rather than `Arc`/atomics.
pub struct Playback {
    pipeline: gst::Pipeline,
    path: PathBuf,
    picture: gtk::Picture,
    playing: Cell<bool>,
    last_toggle: Cell<Instant>,
    /// Set when the pipeline reports end-of-stream, so teardown clears the
    /// saved resume position instead of saving one at the very end.
    reached_eos: Cell<bool>,
    /// Guards against saving twice — teardown can be reached from both the
    /// window closing and playback ending on its own.
    finished: Cell<bool>,
    /// Dropping this removes the bus watch, so it has to outlive playback.
    bus_watch: RefCell<Option<gst::bus::BusWatchGuard>>,
}

impl Playback {
    /// Builds the pipeline, seeks to any saved resume position, and starts
    /// playing. `on_ended` fires when the file finishes or errors out.
    pub fn start(
        path: &Path,
        primary_track: Option<u32>,
        secondary_track: Option<u32>,
        config: &Config,
        restart: bool,
        on_ended: impl Fn() + 'static,
    ) -> Result<Rc<Self>, String> {
        let pipeline = build_pipeline(path, primary_track, secondary_track, config)?;

        // gtk4paintablesink renders into a GdkPaintable rather than creating
        // its own window; handing that to a gtk::Picture is what embeds the
        // video as an ordinary widget in a window we own.
        let paintable = pipeline
            .by_name("vsink")
            .ok_or("missing vsink element")?
            .property::<gdk::Paintable>("paintable");

        let playback = Rc::new(Self {
            pipeline: pipeline.clone(),
            path: path.to_path_buf(),
            picture: gtk::Picture::builder()
                .paintable(&paintable)
                .css_classes([VIDEO_CSS_CLASS])
                .build(),
            playing: Cell::new(true),
            last_toggle: Cell::new(Instant::now() - TOGGLE_DEBOUNCE),
            reached_eos: Cell::new(false),
            finished: Cell::new(false),
            bus_watch: RefCell::new(None),
        });

        let bus = pipeline.bus().ok_or("pipeline has no bus")?;
        let watch = {
            let playback = playback.clone();
            bus.add_watch_local(move |_, msg| {
                use gst::MessageView;
                match msg.view() {
                    MessageView::Eos(_) => {
                        playback.reached_eos.set(true);
                        on_ended();
                    }
                    MessageView::Error(err) => {
                        eprintln!("Error: {} ({:?})", err.error(), err.debug());
                        on_ended();
                    }
                    MessageView::Warning(warn) => {
                        eprintln!(
                            "Warning [{}]: {} ({:?})",
                            msg.src().map(|s| s.name().to_string()).unwrap_or_default(),
                            warn.error(),
                            warn.debug()
                        );
                    }
                    _ => {}
                }
                glib::ControlFlow::Continue
            })
            .map_err(|e| e.to_string())?
        };
        *playback.bus_watch.borrow_mut() = Some(watch);

        // Preroll before PLAYING so a saved position is applied before
        // anything is visible or audible, rather than jumping after
        // playback has already started.
        pipeline
            .set_state(gst::State::Paused)
            .map_err(|e| format!("Failed to preroll: {e}"))?;
        let _ = pipeline.state(gst::ClockTime::from_seconds(10));

        if !restart {
            if let Some(ns) = load_positions().get(&path.to_string_lossy().to_string()).copied() {
                pipeline
                    .seek_simple(
                        gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                        gst::ClockTime::from_nseconds(ns),
                    )
                    .map_err(|e| e.to_string())?;
            }
        }

        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| format!("Failed to start playback: {e}"))?;

        Ok(playback)
    }

    pub fn widget(&self) -> &gtk::Picture {
        &self.picture
    }

    pub fn toggle_pause(&self) {
        if self.last_toggle.get().elapsed() < TOGGLE_DEBOUNCE {
            return;
        }
        self.last_toggle.set(Instant::now());

        if self.playing.get() {
            let _ = self.pipeline.set_state(gst::State::Paused);
        } else {
            let _ = self.pipeline.set_state(gst::State::Playing);
        }
        // Tracked rather than read back from the pipeline: state changes are
        // asynchronous, so a quick second toggle could observe a stale state
        // and issue the wrong transition, leaving playback stuck.
        self.playing.set(!self.playing.get());
    }

    /// Persist (or clear) the resume position and tear the pipeline down.
    /// Idempotent.
    pub fn stop(&self) {
        if self.finished.replace(true) {
            return;
        }

        if self.reached_eos.get() {
            clear_position(&self.path);
        } else if let Some(position) = self.pipeline.query_position::<gst::ClockTime>() {
            if position.nseconds() > 0 {
                save_position(&self.path, position.nseconds());
            }
        }

        let _ = self.pipeline.set_state(gst::State::Null);
        self.bus_watch.borrow_mut().take();
    }
}
