use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gst::prelude::*;
use gstreamer as gst;
use gtk::{gdk, glib};

use crate::config::{Config, clear_position, save_position};
use crate::pipeline::build_pipeline;
use crate::source::Source;

/// Applied to the video widget so the letterbox area around the picture
/// can be styled black without affecting the rest of the interface.
pub const VIDEO_CSS_CLASS: &str = "video-surface";

/// Key auto-repeat delivers a fresh press event for as long as a key is
/// held, and GTK's key controller doesn't distinguish a repeat from a
/// genuine new press. Debouncing is what keeps "hold space" from
/// rapid-fire toggling play/pause.
const TOGGLE_DEBOUNCE: Duration = Duration::from_millis(300);

/// One press moves this far. Long enough to clear a scene, short enough to
/// tap repeatedly.
pub const STEP_SECONDS: f64 = 10.0;

/// How long a direction must be held before it scrubs rather than stepping.
/// Comfortably longer than a deliberate tap and shorter than any key-repeat
/// delay, so the two never get confused.
const HOLD_THRESHOLD: Duration = Duration::from_millis(350);

/// Seconds of film per second of holding, by how long it has been held.
///
/// Expressed as a rate rather than a step size on purpose: the movement is
/// driven by a timer, so it stays smooth regardless of how fast the keyboard
/// or controller happens to repeat. Raising the step size instead is what
/// made the fast tiers lurch.
const SCRUB_RATES: [(Duration, f64); 4] = [
    (Duration::from_secs(0), 60.0),
    (Duration::from_secs(2), 150.0),
    (Duration::from_secs(4), 350.0),
    (Duration::from_secs(6), 800.0),
];

/// Why playback stopped on its own.
///
/// The two are worth telling apart because reaching the end is the ordinary
/// close of a film, while failing partway leaves a reason someone needs to
/// read. Anywhere that acts on the end of playback has to decide which it is
/// looking at.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// The file played to its end.
    Finished,
    /// The pipeline reported an error partway through.
    Failed,
}

/// An in-progress playback: the pipeline, plus the widget its video is
/// drawn into. Single-threaded - every GTK callback runs on the main
/// thread - so `Rc`/`Cell` rather than `Arc`/atomics.
pub struct Playback {
    pipeline: gst::Pipeline,
    /// How this video's position is filed - a Kodi id when Kodi launched us,
    /// otherwise derived from the source itself.
    key: String,
    /// The path Kodi knows the item by, for reporting progress back to it.
    /// Empty when Kodi is not involved.
    kodi_file: String,
    /// The share of a video that counts as watched, from the config, so a
    /// position near the end is dropped rather than saved.
    watched_percent: f64,
    picture: gtk::Picture,
    playing: Cell<bool>,
    last_toggle: Cell<Instant>,
    /// Set when the pipeline reports end-of-stream, so teardown clears the
    /// saved resume position instead of saving one at the very end.
    reached_eos: Cell<bool>,
    /// Guards against saving twice - teardown can be reached from both the
    /// window closing and playback ending on its own.
    finished: Cell<bool>,
    /// Dropping this removes the bus watch, so it has to outlive playback.
    bus_watch: RefCell<Option<gst::bus::BusWatchGuard>>,
    /// The final report to Kodi, still in flight. Kept so an exit can wait for
    /// it: the thread is detached, and a process that ends first takes the
    /// request with it.
    final_report: RefCell<Option<std::thread::JoinHandle<()>>>,
    /// Where seeking is heading.
    ///
    /// Repeated skips accumulate against this rather than against
    /// `query_position`, which after a flushing seek still reports the old
    /// position for a moment - so asking the pipeline each time makes a
    /// second skip undo the first.
    seek_target: Cell<Option<gst::ClockTime>>,
    /// A flushing seek is in flight. Issuing another before the pipeline has
    /// finished the first is what stalls playback, so they are queued.
    seeking: Cell<bool>,
    /// A seek arrived mid-seek and still needs performing.
    seek_queued: Cell<bool>,
    /// Where scrubbing has traveled to, before any seek is issued.
    ///
    /// Holding a direction moves this and nothing else, so the timeline runs
    /// ahead while the pipeline carries on playing undisturbed. One seek is
    /// performed when scrubbing settles. Seeks are the expensive and fragile
    /// operation here, so the fewer of them a gesture costs, the better.
    scrub: Cell<Option<gst::ClockTime>>,
    /// Where scrubbing began, which is where a tap steps from.
    scrub_origin: Cell<Option<gst::ClockTime>>,
    scrub_started: Cell<Option<Instant>>,
    /// Sign of the direction being held.
    scrub_direction: Cell<f64>,
    /// Whether the hold lasted long enough to actually travel. A press that
    /// ends before it does is a tap, and steps instead.
    scrubbed: Cell<bool>,
}

impl Playback {
    /// Builds the pipeline, seeks to any saved resume position, and starts
    /// playing. `on_ended` fires when the file finishes or errors out.
    ///
    /// Called from exactly one place, and every argument is a different kind
    /// of thing, so grouping them into a struct would only move the same list
    /// somewhere less obvious than the call site.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        source: &Source,
        primary_audio: Option<&crate::pipeline::AudioSource>,
        secondary_audio: Option<&crate::pipeline::AudioSource>,
        subtitle: Option<&crate::subtitles::SubtitleSource>,
        // Whether the video has any subtitle to offer, which is not the same
        // as one being chosen. The overlay is built either way so that
        // subtitles can be switched on later; see `build_pipeline`.
        offers_subtitles: bool,
        config: &Config,
        resume_ns: Option<u64>,
        key: String,
        kodi_file: String,
        on_ended: impl Fn(Ended) + 'static,
    ) -> Result<Rc<Self>, String> {
        let pipeline = build_pipeline(
            source,
            primary_audio,
            secondary_audio,
            subtitle,
            offers_subtitles,
            config,
        )?;

        // gtk4paintablesink renders into a GdkPaintable rather than creating
        // its own window; handing that to a gtk::Picture is what embeds the
        // video as an ordinary widget in a window we own.
        let paintable = pipeline
            .by_name("vsink")
            .ok_or("missing vsink element")?
            .property::<gdk::Paintable>("paintable");

        let playback = Rc::new(Self {
            pipeline: pipeline.clone(),
            key,
            kodi_file,
            watched_percent: config.watched_percent(),
            picture: gtk::Picture::builder()
                .paintable(&paintable)
                .css_classes([VIDEO_CSS_CLASS])
                .build(),
            playing: Cell::new(true),
            last_toggle: Cell::new(Instant::now() - TOGGLE_DEBOUNCE),
            reached_eos: Cell::new(false),
            finished: Cell::new(false),
            bus_watch: RefCell::new(None),
            final_report: RefCell::new(None),
            seek_target: Cell::new(None),
            seeking: Cell::new(false),
            seek_queued: Cell::new(false),
            scrub: Cell::new(None),
            scrub_origin: Cell::new(None),
            scrub_started: Cell::new(None),
            scrub_direction: Cell::new(0.0),
            scrubbed: Cell::new(false),
        });

        let bus = pipeline.bus().ok_or("pipeline has no bus")?;
        let watch = {
            let playback = playback.clone();
            bus.add_watch_local(move |_, msg| {
                use gst::MessageView;
                match msg.view() {
                    MessageView::Eos(_) => {
                        playback.reached_eos.set(true);
                        on_ended(Ended::Finished);
                    }
                    MessageView::Error(err) => {
                        eprintln!("Error: {} ({:?})", err.error(), err.debug());
                        on_ended(Ended::Failed);
                    }
                    // Posted when a flushing seek has finished settling.
                    // Also fires after the initial preroll, which harmlessly
                    // finds nothing queued.
                    MessageView::AsyncDone(_) => {
                        playback.seeking.set(false);
                        if playback.seek_queued.replace(false) {
                            playback.run_seek();
                        } else {
                            playback.seek_target.set(None);
                        }
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

        // Resolved by the caller rather than read here, because where to pick
        // up from depends on who launched us: under Kodi it is Kodi's library
        // that decides, not our own saved position.
        if let Some(ns) = resume_ns.filter(|ns| *ns > 0) {
            // ACCURATE rather than KEY_UNIT: the latter lands on a keyframe,
            // and with no snap direction that means the keyframe *before* the
            // target. Keyframes are commonly several seconds apart, so
            // resuming a little way into a video snapped back to the one at
            // zero and started it over. Accurate seeking decodes forward from
            // that keyframe to the exact position instead, which costs a
            // moment once at startup and lands where the viewer actually was.
            pipeline
                .seek_simple(
                    gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                    gst::ClockTime::from_nseconds(ns),
                )
                .map_err(|e| e.to_string())?;
        }

        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| format!("Failed to start playback: {e}"))?;

        Ok(playback)
    }

    pub fn widget(&self) -> &gtk::Picture {
        &self.picture
    }

    /// Where playback is, or is about to be. Reporting the seek target while
    /// one is in flight keeps the timeline moving with each press instead of
    /// freezing until the pipeline catches up.
    pub fn position(&self) -> Option<gst::ClockTime> {
        self.scrub
            .get()
            .or_else(|| self.seek_target.get())
            .or_else(|| self.pipeline.query_position::<gst::ClockTime>())
    }

    /// Queried on demand rather than cached at startup: a file whose header
    /// carries no duration only reports one once enough has been parsed.
    pub fn duration(&self) -> Option<gst::ClockTime> {
        self.pipeline.query_duration::<gst::ClockTime>()
    }

    pub fn is_playing(&self) -> bool {
        self.playing.get()
    }

    /// Notes that a direction is being held. Moves nothing by itself.
    ///
    /// Whether this is a tap or a hold is not yet knowable, and guessing is
    /// what made the old behavior unpleasant: it jumped ten seconds on press
    /// and then had to take it back. Nothing moves until either enough time
    /// passes to call it a hold, or the release arrives and calls it a tap.
    pub fn scrub_input(&self, seconds: f64) {
        if self.scrub_started.get().is_none() {
            self.scrub_started.set(Some(Instant::now()));
            self.scrub_origin.set(self.position());
            self.scrubbed.set(false);
        }
        self.scrub_direction.set(seconds.signum());
    }

    /// Advances scrubbing by one frame's worth of travel.
    pub fn scrub_tick(&self, elapsed: Duration) {
        let Some(started) = self.scrub_started.get() else {
            return;
        };
        let held = started.elapsed();
        if held < HOLD_THRESHOLD {
            return;
        }

        let rate = SCRUB_RATES
            .iter()
            .rev()
            .find(|(after, _)| held >= *after)
            .map(|(_, rate)| *rate)
            .unwrap_or(SCRUB_RATES[0].1);

        let Some(from) = self.scrub.get().or_else(|| self.position()) else {
            return;
        };
        let delta = rate * elapsed.as_secs_f64() * self.scrub_direction.get();
        self.scrub.set(Some(self.offset(from, delta)));
        self.scrubbed.set(true);
    }

    pub fn is_scrubbing(&self) -> bool {
        self.scrub_started.get().is_some()
    }

    /// `from` moved by `seconds`, kept inside the file. Landing exactly on
    /// the end would finish it, which is a surprising outcome for scrubbing
    /// forward, so it stops just short.
    fn offset(&self, from: gst::ClockTime, seconds: f64) -> gst::ClockTime {
        let delta = (seconds * gst::ClockTime::SECOND.nseconds() as f64) as i64;
        let mut target = (from.nseconds() as i64).saturating_add(delta).max(0) as u64;
        if let Some(duration) = self.duration() {
            target = target.min(
                duration
                    .nseconds()
                    .saturating_sub(gst::ClockTime::SECOND.nseconds()),
            );
        }
        gst::ClockTime::from_nseconds(target)
    }

    /// Level and mute for one output, adjusted while playing.
    ///
    /// Each branch carries its own `volume` element, so the two outputs are
    /// independent - which is the point, when two people are listening on
    /// different devices. Nothing here touches the machine's own mixer.
    pub fn set_volume(&self, role: &str, level: f64) {
        if let Some(volume) = self.pipeline.by_name(&format!("{role}_volume")) {
            volume.set_property("volume", level.clamp(0.0, 1.0));
        }
    }

    pub fn volume(&self, role: &str) -> Option<f64> {
        self.pipeline
            .by_name(&format!("{role}_volume"))
            .map(|volume| volume.property::<f64>("volume"))
    }

    pub fn set_muted(&self, role: &str, muted: bool) {
        if let Some(volume) = self.pipeline.by_name(&format!("{role}_volume")) {
            volume.set_property("mute", muted);
        }
    }

    pub fn muted(&self, role: &str) -> bool {
        self.pipeline
            .by_name(&format!("{role}_volume"))
            .is_some_and(|volume| volume.property::<bool>("mute"))
    }

    /// Whether this playback has an output for `role` at all: a single-device
    /// setup has no secondary branch, and nothing to adjust.
    pub fn has_output(&self, role: &str) -> bool {
        self.pipeline.by_name(&format!("{role}_volume")).is_some()
    }

    /// Holds one output back, while a film is playing, so it can be lined up
    /// against the picture by ear rather than by arithmetic.
    ///
    /// Takes effect on the next buffer the sink renders, so the change is
    /// heard within a moment without interrupting playback - which is the
    /// point, since the only way to judge a delay is to hear it against
    /// something.
    pub fn set_offset_ms(&self, role: &str, ms: f64) {
        if let Some(sink) = self.pipeline.by_name(&format!("{role}_out")) {
            crate::pipeline::set_offset(&sink, ms);
        }
    }

    /// Holds the picture still while the scrubber is being dragged, and lets
    /// it go afterwards.
    ///
    /// Deliberately not `toggle_pause`: `playing` is left alone, so the
    /// transport button keeps showing what playback will do when the drag
    /// ends, and letting go resumes only if it was running to begin with.
    /// Dragging a playhead while the film carries on underneath it is a fight
    /// between the pointer and the clock.
    pub fn hold_for_scrub(&self) {
        let _ = self.pipeline.set_state(gst::State::Paused);
    }

    pub fn release_from_scrub(&self) {
        if self.playing.get() {
            let _ = self.pipeline.set_state(gst::State::Playing);
        }
    }

    /// Notes where the scrubber has been dragged to, without moving the
    /// pipeline yet.
    ///
    /// Separated from the seek itself so a drag can report where it is going
    /// immediately while the expensive part waits for the pointer to settle.
    /// The readout asks [`Self::position`], which prefers this target, so the
    /// playhead follows the pointer instead of being dragged back to where
    /// playback still is.
    pub fn aim_at(&self, target: gst::ClockTime) {
        self.scrub.set(None);
        self.scrub_started.set(None);
        self.scrubbed.set(false);
        self.seek_target.set(Some(target));
    }

    /// Performs whatever [`Self::aim_at`] last recorded. Goes through the same
    /// queue as everything else, so a drag cannot pile seeks onto a pipeline
    /// still servicing the last one.
    pub fn commit_seek(&self) {
        if self.seek_target.get().is_none() {
            return;
        }
        if self.seeking.get() {
            self.seek_queued.set(true);
        } else {
            self.run_seek();
        }
    }

    /// Performs the one seek the gesture asked for: to wherever scrubbing
    /// traveled, or one step along if it turned out to be a tap.
    pub fn commit_scrub(&self) {
        let scrubbed = self.scrubbed.replace(false);
        let direction = self.scrub_direction.replace(0.0);
        let origin = self.scrub_origin.take();
        let traveled = self.scrub.take();
        self.scrub_started.set(None);

        let target = if scrubbed {
            traveled
        } else {
            origin.map(|origin| self.offset(origin, STEP_SECONDS * direction))
        };
        let Some(target) = target else {
            return;
        };
        self.seek_target.set(Some(target));

        if self.seeking.get() {
            self.seek_queued.set(true);
        } else {
            self.run_seek();
        }
    }
}

/// The GStreamer release that no longer needs the seek workaround below.
const SEEK_WORKAROUND_FIXED_IN: (u32, u32) = (1, 24);

/// Whether each audio sink has to be taken down around a flushing seek.
///
/// With two audio outputs on Linux, a flushing seek silences them for the rest
/// of playback. Taking each sink to NULL before the seek and re-syncing it
/// afterwards avoids it, because the sinks never see the flush - keeping them
/// up through it and restarting after was measured and is markedly worse.
///
/// **This is a GStreamer bug, not ours, and it is fixed upstream in 1.24.**
/// Measured 2026-08-05 by running one binary and one clip across seven
/// environments, recording each output device and reading the audio back
/// rather than asking the pipeline how it thought it was doing:
///
/// | GStreamer | x86_64      | aarch64      |
/// |-----------|-------------|--------------|
/// | 1.20.3    | fails 5/8   | fails 3/3    |
/// | 1.22.0    | fails 2/8   | fails 3/3    |
/// | 1.24.2    | passes 8/8  | -            |
/// | 1.26.2    | -           | passes 11/11 |
///
/// So it is not a 1.22 regression and not specific to the Raspberry Pi: a VM
/// on entirely different hardware reproduces it, and PipeWire's version does
/// not predict it (0.3.65 and 1.2.7 fail; 0.3.48, 1.0.5 and 1.4.2 pass). It is
/// a race - lost far more readily on ARM than x86, and the rate moves with the
/// compositor - which is why the passing cells carry 8 and 11 runs. A clean
/// run of three proves nothing here, and believing one cost three wrong
/// conclusions along the way.
///
/// The mechanism is still unknown, and the version check stands on the
/// measurements rather than on understanding the fault. What was ruled out:
/// buffers keep arriving at the sink pad at full rate carrying real audio,
/// the sink reports rendering them with none dropped, flush events arrive
/// matched at every point down both branches, and the segment and base time
/// match the video branch's - which keeps playing throughout.
///
/// Gated below 1.24 rather than at the versions measured, so anything older
/// and untested - including the 1.18 baseline `Cargo.toml` supports - is
/// covered. Over-applying costs only the seek reporting noted at the call
/// site; under-applying costs the audio.
fn needs_seek_workaround() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    // The runtime version, not the one built against: the .deb is compiled
    // once and runs on whatever GStreamer the machine provides.
    let (major, minor, _, _) = gst::version();
    seek_workaround_applies(major, minor)
}

/// Split out from [`needs_seek_workaround`] so the boundary itself can be
/// tested. Getting this comparison backwards would be silent on a developer
/// machine with a new GStreamer and would cost every Debian 12 user their
/// audio.
fn seek_workaround_applies(major: u32, minor: u32) -> bool {
    (major, minor) < SEEK_WORKAROUND_FIXED_IN
}

impl Playback {
    /// Hands a seek straight to each external audio file's own source.
    ///
    /// Only needed alongside the Linux workaround above, which is the one case
    /// where the ordinary route - upstream from the sinks - is closed. The
    /// same flags as the pipeline seek, so the two branches land in the same
    /// place by the same rules rather than nearly.
    fn seek_external_audio(&self, target: gst::ClockTime) {
        for index in 0.. {
            let name = format!("{}{index}", crate::pipeline::EXTERNAL_AUDIO_DECODER);
            let Some(source) = self.pipeline.by_name(&name) else {
                // Numbered from zero without gaps, so the first miss is the end.
                break;
            };
            let seek = gst::event::Seek::new(
                1.0,
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::SeekType::Set,
                target,
                gst::SeekType::None,
                gst::ClockTime::NONE,
            );
            if !source.send_event(seek) {
                eprintln!("Failed to seek the external audio source {name}");
            }
        }
    }

    /// Sends a seek to the subtitle chain, which has a source of its own.
    ///
    /// The same hand delivery the external audio needs, and for the same
    /// reason: this chain is fed by its own source rather than by the video's,
    /// so a seek sent through the pipeline does not reliably reach it. Without
    /// this the subtitles carry on from where they were while the picture
    /// moves, which reads as them being wrong rather than merely behind.
    fn seek_external_subtitle(&self, target: gst::ClockTime) {
        let Some(source) = self
            .pipeline
            .by_name(crate::pipeline::EXTERNAL_SUBTITLE_SOURCE)
        else {
            return;
        };
        let seek = gst::event::Seek::new(
            1.0,
            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
            gst::SeekType::Set,
            target,
            gst::SeekType::None,
            gst::ClockTime::NONE,
        );
        if !source.send_event(seek) {
            eprintln!("Failed to seek the subtitle source");
        }
    }

    fn run_seek(&self) {
        let Some(target) = self.seek_target.get() else {
            return;
        };

        // ACCURATE rather than KEY_UNIT. Landing on a keyframe is cheaper, but
        // it lands on the keyframe *before* the target, so the playhead settles
        // behind where it was sent - every time. How far behind depends on the
        // encode: barely visible with keyframes two seconds apart, glaring with
        // eight. SNAP_NEAREST was tried first and only halves it, because the
        // nearest keyframe is still not the place that was asked for.
        //
        // The cost is decoding forward from that keyframe, which is why this
        // was avoided originally. It is paid once per gesture rather than per
        // press: holding a direction moves the target alone, and a single seek
        // is issued when it settles.
        let workaround = needs_seek_workaround();

        if workaround {
            for role in ["primary", "secondary"] {
                if let Some(sink) = self.pipeline.by_name(&format!("{role}_out")) {
                    let _ = sink.set_state(gst::State::Null);
                }
            }
        }

        let result = self
            .pipeline
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE, target);

        // Unconditional, not part of the workaround above. Measured on the Pi
        // on 2026-08-10: an external file goes silent on the first seek with
        // the workaround on *and* off, so the seek is not reaching its source
        // by the ordinary upstream route either way. An embedded track is
        // unaffected because it shares the video's source, which the seek does
        // reach. Seeking the same target twice is harmless - both branches are
        // being sent to the same place.
        self.seek_external_audio(target);
        self.seek_external_subtitle(target);

        if workaround {
            for role in ["primary", "secondary"] {
                if let Some(sink) = self.pipeline.by_name(&format!("{role}_out")) {
                    let _ = sink.sync_state_with_parent();
                }
            }
            // With a sink in NULL the pipeline cannot report the seek as
            // handled, even though it takes effect through the video branch,
            // so the result says nothing here and reporting it would print a
            // failure on every skip. That cost is why the workaround is gated
            // rather than unconditional: on a GStreamer that does not need it,
            // a genuine seek failure is reported again.
            self.seeking.set(true);
            return;
        }

        match result {
            Ok(()) => self.seeking.set(true),
            Err(e) => {
                eprintln!("Seek failed: {e}");
                self.seek_target.set(None);
            }
        }
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

        // Pausing is the clearest signal that someone has stopped watching,
        // so it is worth a report of its own rather than waiting for the timer.
        self.report_to_kodi();
    }

    /// Tells Kodi where playback has reached, if Kodi launched us.
    ///
    /// Called often - on a timer, and whenever playback pauses or stops - so
    /// that a film abandoned without closing the player cleanly still leaves
    /// Kodi's library close to right. The call itself is made on a thread and
    /// cannot block playback.
    pub fn report_to_kodi(&self) {
        if self.kodi_file.is_empty() {
            return;
        }
        let (Some(position), Some(duration)) = (
            self.pipeline.query_position::<gst::ClockTime>(),
            self.pipeline.query_duration::<gst::ClockTime>(),
        ) else {
            return;
        };
        crate::kodi::report_position(&self.kodi_file, position.nseconds(), duration.nseconds());
    }

    /// Persist (or clear) the resume position and tear the pipeline down.
    /// Idempotent.
    pub fn stop(&self) {
        if self.finished.replace(true) {
            return;
        }

        if self.reached_eos.get() {
            clear_position(&self.key);
            // Watched to the end, which Kodi records as a play rather than a
            // resume point. Reported as the full duration so it crosses the
            // same threshold Kodi's own player uses.
            if !self.kodi_file.is_empty()
                && let Some(duration) = self.pipeline.query_duration::<gst::ClockTime>()
            {
                *self.final_report.borrow_mut() = crate::kodi::report_position(
                    &self.kodi_file,
                    duration.nseconds(),
                    duration.nseconds(),
                );
            }
        } else if let Some(position) = self.pipeline.query_position::<gst::ClockTime>()
            && position.nseconds() > 0
        {
            save_position(
                &self.key,
                position.nseconds(),
                self.pipeline
                    .query_duration::<gst::ClockTime>()
                    .map(|d| d.nseconds())
                    .unwrap_or(0),
                self.watched_percent,
            );
            self.report_to_kodi();
        }

        let _ = self.pipeline.set_state(gst::State::Null);
        self.bus_watch.borrow_mut().take();
    }

    /// Whether there are subtitles to turn on and off right now.
    ///
    /// The overlay alone stopped being the answer. It is built whenever the
    /// video offers a subtitle rather than only when one was chosen, so that
    /// they can be switched on later - which means it is often sitting there
    /// with nothing feeding it. Asking whether anything is attached is what
    /// keeps the button from offering to show what is not there.
    pub fn has_subtitles(&self) -> bool {
        self.pipeline
            .by_name("suboverlay")
            .and_then(|overlay| overlay.static_pad("subtitle_sink"))
            .is_some_and(|pad| pad.is_linked())
    }

    /// Turns subtitles on or off mid-playback, returning whether they are now
    /// showing. Does nothing, and reports false, when there are none.
    ///
    /// `subtitleoverlay`'s property is `silent`, and its blurb reads "Whether
    /// to show subtitles", which is backwards: silent means *not* drawn. It
    /// takes effect on the next frame, so nothing is rebuilt or re-prerolled
    /// and the picture never stutters.
    pub fn toggle_subtitles(&self) -> bool {
        let Some(overlay) = self.pipeline.by_name("suboverlay") else {
            return false;
        };
        // Named for what the property means rather than what it is called.
        // Flipping it makes the new state the opposite of this, which is the
        // same value again: what was hidden is now showing.
        let was_hidden = overlay.property::<bool>("silent");
        overlay.set_property("silent", !was_hidden);
        was_hidden
    }

    /// Whether subtitles are currently drawn.
    pub fn subtitles_showing(&self) -> bool {
        self.pipeline
            .by_name("suboverlay")
            .is_some_and(|overlay| !overlay.property::<bool>("silent"))
    }

    /// Waits for the last report to Kodi to finish, for a caller that is about
    /// to end the process. Bounded by the socket timeout in [`crate::kodi`],
    /// and does nothing at all when Kodi is not involved.
    pub fn finish_reporting(&self) {
        if let Some(handle) = self.final_report.borrow_mut().take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::seek_workaround_applies;

    #[test]
    fn workaround_covers_the_versions_measured_broken() {
        // Measured failing: see the table on `needs_seek_workaround`.
        assert!(seek_workaround_applies(1, 20));
        assert!(seek_workaround_applies(1, 22));
        // Older than anything tested, and older than the supported baseline.
        assert!(seek_workaround_applies(1, 18));
        assert!(seek_workaround_applies(0, 11));
    }

    #[test]
    fn workaround_is_skipped_where_measured_healthy() {
        assert!(!seek_workaround_applies(1, 24));
        assert!(!seek_workaround_applies(1, 26));
        // A future major release must not silently fall back into it.
        assert!(!seek_workaround_applies(2, 0));
    }

    #[test]
    fn the_boundary_is_exactly_at_the_fixed_release() {
        assert!(seek_workaround_applies(1, 23));
        assert!(!seek_workaround_applies(1, 24));
    }
}
