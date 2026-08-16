use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gst::prelude::*;
use gstreamer as gst;

use crate::config::Config;
use crate::devices::find_audio_output_device;
use crate::source::Source;
use crate::subtitles::SubtitleSource;

/// Pango leaves the family unspecified by default, which resolves to a serif
/// face. Bold with the renderer's black outline is what stays legible against
/// a moving picture.
pub const DEFAULT_SUBTITLE_FONT: &str = "Sans Bold";

/// Smaller than it looks: the renderer scales the font by the video's width,
/// so on a 1080p frame this draws text 46 pixels tall, about 4.3% of the
/// frame height. Measured, because the same description at 24 came out at 93
/// pixels and dominated the picture.
pub const DEFAULT_SUBTITLE_SIZE: u32 = 12;

/// What a stream was selected for, recorded when decodebin3 asks whether to
/// expose it and read back when its pad actually appears.
#[derive(Clone, Copy)]
enum Target {
    Video,
    /// A subtitle stream inside the file. Only one is ever selected, so
    /// unlike audio there is no index to route by.
    Subtitle,
    /// An audio stream inside the file. Carries no track number, unlike every
    /// earlier version of this: which output it feeds is not fixed and is the
    /// routing's business, so recording one here would be a second answer to
    /// the same question, free to disagree with the first.
    Audio,
}

/// The head element of each branch, i.e. the thing a decoded pad links into.
struct Targets {
    video: gst::Element,
    /// Where audio goes, which unlike the other two changes while playing.
    audio: Arc<Mutex<AudioRouting>>,
    /// The overlay subtitles are drawn by, when there are any.
    subtitle: Option<gst::Element>,
}

/// What an output is playing, in the only terms that survive a switch.
///
/// A track is named by its position among the file's audio streams rather than
/// by its GStreamer stream id, because the id is not known until the file has
/// been opened while the choice has to be expressible before that. A file is
/// named by its URI, which never had an id in the video's collection at all.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Playing {
    /// A stream inside the video, by the index `--list-tracks` prints.
    Track(u32),
    /// A separate audio file, by URI.
    File(String),
}

/// Which decoded stream feeds which output, and the plumbing between them.
///
/// The problem this solves is that the pairing is not fixed. Two outputs on
/// one track share a single decoded stream and must be fed from a `tee`; move
/// one of them to a track of its own and that `tee` has to give up a branch;
/// move it back and two branches have to become one again. Building each of
/// those separately would be three ways to get it wrong.
///
/// So nothing here is rebuilt. Each output's chain is made once and lives for
/// the whole film. Each decoded stream gets a `tee` of its own the moment its
/// pad appears. Changing a soundtrack is then only ever pointing an output's
/// chain at a different `tee`, and [`AudioRouting::reconcile`] is the single
/// operation that does it - splitting, merging and plain switching are all the
/// same act of pointing something somewhere.
///
/// **Hubs are keyed by pad and not by track**, which is the one thing here
/// that is not obvious and cost a rewrite. decodebin3 reuses the slot it
/// already has when a selection changes, so the pad that was carrying track 1
/// simply starts carrying track 2 with no pad event at all - measured
/// 2026-08-14. A hub keyed by track would go on claiming to hold a track that
/// had moved elsewhere. What each pad is carrying is therefore followed
/// through the stream-start event, which does fire.
///
/// **By pad, and a pad is not its name** - see [`hub_key`]. Names are unique
/// only within an element, and every external audio file brings a decoder of
/// its own that numbers its pads from zero exactly as the video's does.
///
/// Locked rather than borrowed because it is reached from two directions:
/// `pad-added` and the stream-start probe arrive on streaming threads, while a
/// viewer choosing a soundtrack arrives on the main one.
#[derive(Default)]
pub struct AudioRouting {
    /// The first element of each output's chain, by role. Built once.
    chains: HashMap<String, gst::Element>,
    /// The `tee` on each decoded stream, by the name of the pad feeding it.
    hubs: HashMap<String, gst::Element>,
    /// What each of those pads is currently carrying. Separate from `hubs`
    /// because it changes without the hub changing.
    carrying: HashMap<String, Playing>,
    /// What each output is meant to be playing. The intent, recorded before
    /// the pipeline is asked for anything, so that a stream arriving later can
    /// be placed rather than guessed at - which is exactly what the subtitle
    /// equivalent of this could not do.
    wanted: HashMap<String, Playing>,
    /// The `tee` pad each output's chain is fed by, so it can be released
    /// again. Absent for an output connected to nothing.
    links: HashMap<String, gst::Pad>,
    /// The file's streams, needed to say which track a stream id is. Caught
    /// from the bus, since an element will not hand it over on request.
    collection: Option<gst::StreamCollection>,
    /// The pipeline these live in, so a chain nobody is drawing from can be
    /// given something that does - see [`attach_pacer`].
    ///
    /// **Weak, or a film is never freed.** The handlers holding this routing
    /// belong to elements inside that pipeline, so a strong reference here
    /// closes a cycle and every film ever played stays in memory holding its
    /// audio devices open.
    pipeline: Option<gst::glib::WeakRef<gst::Pipeline>>,
}

impl AudioRouting {
    /// Says what an output should be playing from now on. Takes effect at the
    /// next [`Self::reconcile`].
    pub fn want(&mut self, role: &str, playing: Option<Playing>) {
        trace(format_args!("want {role} = {playing:?}"));
        match playing {
            Some(playing) => self.wanted.insert(role.into(), playing),
            None => self.wanted.remove(role),
        };
    }

    /// Everything this holds, for the trace. Printed rather than reasoned
    /// about, because the pipeline's account of itself is not evidence.
    fn trace_state(&self, what: &str) {
        if !tracing_audio() {
            return;
        }
        let list = |pairs: &HashMap<String, Playing>| {
            let mut out: Vec<String> = pairs
                .iter()
                .map(|(key, playing)| format!("{key}={playing:?}"))
                .collect();
            out.sort();
            out.join(", ")
        };
        let mut links: Vec<String> = self
            .links
            .iter()
            .map(|(role, pad)| {
                format!(
                    "{role}<-{}:{}",
                    pad.parent()
                        .map(|tee| tee.name().to_string())
                        .unwrap_or_default(),
                    pad.name()
                )
            })
            .collect();
        links.sort();
        let mut hubs: Vec<String> = self.hubs.keys().cloned().collect();
        hubs.sort();
        eprintln!(
            "[audio] {what}\n  wanted:   {}\n  carrying: {}\n  links:    {}\n  hubs:     {}",
            list(&self.wanted),
            list(&self.carrying),
            links.join(", "),
            hubs.join(", ")
        );
    }

    /// What an output is meant to be playing.
    pub fn wanted_by(&self, role: &str) -> Option<&Playing> {
        self.wanted.get(role)
    }

    /// Whether what an output now wants can be settled without waiting for the
    /// decoder: it is already being carried, or it is nothing at all.
    fn can_settle(&self, role: &str) -> bool {
        match self.wanted.get(role) {
            None => true,
            Some(wanted) => self.carrying.values().any(|playing| playing == wanted),
        }
    }

    /// Points an output at what it now wants, when that needs nothing to
    /// arrive first.
    ///
    /// **Every other [`Self::reconcile`] is driven by a pad**, and that covers
    /// a switch between two of the film's own tracks, because the stream
    /// selection answering it always moves one. Three switches move no pad at
    /// all: onto a separate audio file, whose chain has been carrying it since
    /// the pipeline was built; onto nothing; and onto a track the *other*
    /// output is already playing, which is a selection the decoder has already
    /// satisfied. Each of those was recorded and then never acted on, so the
    /// output carried on playing what it had.
    ///
    /// Deliberately does nothing when what is wanted is not carried yet. That
    /// is a stream on its way, and disconnecting to wait for it would turn a
    /// handover into a gap - the output keeps what it has until the pad
    /// arrives, which is what it has always done.
    pub fn settle(&mut self, role: &str) {
        let can = self.can_settle(role);
        self.trace_state(&format!("settle {role}, can_settle={can}"));
        if can {
            self.reconcile();
        }
    }

    /// Remembers the pipeline, weakly. Called once, as it is built.
    pub fn watch(&mut self, pipeline: &gst::Pipeline) {
        self.pipeline = Some(pipeline.downgrade());
    }

    /// Gives a branch of its own to any external chain nothing is drawing
    /// from, so it keeps step instead of running to the end of the file.
    ///
    /// **Called from [`Self::reconcile`], which is the only place links
    /// change.** Doing it from `set_audio` instead looked equivalent and was
    /// not: an output moving to a track that has yet to be decoded is still
    /// linked to its old hub at that point, and only lets go later, on the
    /// streaming thread, when the new pad arrives. So nothing ever looked
    /// unattended and the chain raced away exactly as before.
    fn mind_unattended_files(&self) {
        let Some(pipeline) = self.pipeline.as_ref().and_then(|weak| weak.upgrade()) else {
            return;
        };
        for (hub, attended) in self.external_hubs() {
            // The pacer is named after the hub it hangs off, so asking the
            // pipeline for it by name is the whole record of whether one is
            // there already.
            let pacer = pipeline.by_name(&format!("pacer_{}", hub.name()));
            match (attended, pacer) {
                (false, None) => attach_pacer(&pipeline, &hub),
                // **Taken off again the moment an output comes back**, and
                // that is not tidiness either. Left on, it shares the hub with
                // the output's own branch, and the next flushing seek has it
                // waiting on a clock that has stopped because that very output
                // is prerolling - the startup deadlock again, from the other
                // direction. Seeking after a couple of switches locked the
                // picture solid.
                (true, Some(_)) => detach_pacer(&pipeline, &hub),
                _ => {}
            }
        }
    }

    /// Every hub carrying an external file, and whether an output is drawing
    /// from it.
    ///
    /// Answered from the links rather than from `wanted`, because what matters
    /// is what is *connected* right now, not what has been asked for and is
    /// still on its way.
    fn external_hubs(&self) -> Vec<(gst::Element, bool)> {
        self.carrying
            .iter()
            .filter(|(_, playing)| matches!(playing, Playing::File(_)))
            .filter_map(|(pad, _)| self.hubs.get(pad))
            .map(|hub| {
                let attended = self
                    .links
                    .values()
                    .any(|pad| pad.parent().as_ref() == Some(hub.upcast_ref()));
                (hub.clone(), attended)
            })
            .collect()
    }

    /// The tracks inside the video that some output is waiting for, which is
    /// what a stream selection has to ask the decoder for. External files are
    /// not among them: they are decoded by a chain of their own that the
    /// video's selection knows nothing about.
    pub fn wanted_tracks(&self) -> Vec<u32> {
        let mut tracks: Vec<u32> = self
            .wanted
            .values()
            .filter_map(|playing| match playing {
                Playing::Track(track) => Some(*track),
                Playing::File(_) => None,
            })
            .collect();
        tracks.sort_unstable();
        tracks.dedup();
        tracks
    }

    /// Keeps the collection, so a stream id arriving on a pad can be turned
    /// into the track number the rest of the application speaks in.
    pub fn set_collection(&mut self, collection: gst::StreamCollection) {
        self.collection = Some(collection);
    }

    /// Notes what a pad is carrying now, and re-points anything affected.
    fn now_carrying(&mut self, pad: &str, playing: Playing) {
        if self.carrying.get(pad) == Some(&playing) {
            return;
        }
        self.carrying.insert(pad.into(), playing);
        self.reconcile();
    }

    /// The track a decoded pad is carrying, by the stream on its stream-start
    /// event. `None` when the pad carries no stream yet, or one the collection
    /// does not describe.
    fn track_on(&self, pad: &gst::Pad) -> Option<Playing> {
        let collection = self.collection.as_ref()?;
        let id = pad.stream()?.stream_id()?;
        ordinal(collection, &id, gst::StreamType::AUDIO).map(Playing::Track)
    }

    /// Points every output's chain at the `tee` carrying what it is meant to
    /// be playing, and disconnects any output whose stream has not arrived, or
    /// has gone away.
    ///
    /// Safe to call whenever anything might have changed, and called from
    /// exactly that: a pad appearing, a pad going away, a pad changing what it
    /// carries, and a viewer choosing. Doing nothing when nothing has moved is
    /// what lets it be called freely, and what keeps those callers independent
    /// of each other rather than a sequence that has to be got right.
    fn reconcile(&mut self) {
        let roles: Vec<String> = self.chains.keys().cloned().collect();
        for role in roles {
            let hub = self.wanted.get(&role).and_then(|wanted| {
                self.carrying
                    .iter()
                    .find(|(_, playing)| *playing == wanted)
                    .and_then(|(pad, _)| self.hubs.get(pad))
                    .cloned()
            });
            trace(format_args!(
                "reconcile {role} -> {}",
                hub.as_ref()
                    .map(|hub| hub.name().to_string())
                    .unwrap_or_else(|| "nothing".into())
            ));
            self.point(&role, hub.as_ref());
        }
        self.mind_unattended_files();
    }

    /// Feeds one output's chain from `hub`, or from nothing when it is `None`.
    ///
    /// The `tee` pad is released rather than merely unlinked. A `tee` goes on
    /// pushing to every pad it has been asked for, so one left behind would
    /// have the element block waiting for a branch nobody is draining.
    fn point(&mut self, role: &str, hub: Option<&gst::Element>) {
        let Some(chain) = self.chains.get(role) else {
            return;
        };
        let Some(sink) = chain.static_pad("sink") else {
            return;
        };

        // Already fed by the right one, which is the ordinary case every time
        // this is called for a reason that has nothing to do with this output.
        if let (Some(current), Some(hub)) = (self.links.get(role), hub)
            && current.parent().as_ref() == Some(hub.upcast_ref())
        {
            trace(format_args!("point {role}: already fed by {}", hub.name()));
            return;
        }

        if let Some(pad) = self.links.remove(role) {
            let _ = pad.unlink(&sink);
            if let Some(tee) = pad.parent_element() {
                tee.release_request_pad(&pad);
            }
            trace(format_args!("point {role}: released its old branch"));
        }

        let Some(hub) = hub else { return };
        let Some(src) = hub.request_pad_simple("src_%u") else {
            eprintln!("Failed to get a tee pad for the {role} output");
            return;
        };
        if let Err(e) = src.link(&sink) {
            eprintln!("Failed to feed the {role} output: {e}");
            hub.release_request_pad(&src);
            return;
        }
        trace(format_args!(
            "point {role}: linked {}:{} -> {}, pad is {}",
            hub.name(),
            src.name(),
            sink.name(),
            if src.pad_flags().contains(gst::PadFlags::EOS) {
                "already EOS"
            } else {
                "flowing"
            }
        ));
        self.links.insert(role.into(), src);
    }
}

/// Whether to say what the audio routing is doing, on stderr. Set
/// `TINEPLAYER_TRACE_AUDIO=1` to turn it on; it is silent otherwise.
pub fn tracing_audio() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TINEPLAYER_TRACE_AUDIO").is_ok_and(|on| on != "0"))
}

/// One line of that trace.
pub fn trace(what: std::fmt::Arguments<'_>) {
    if tracing_audio() {
        eprintln!("[audio] {what}");
    }
}

/// What each external audio file's decoder is called in the pipeline, with its
/// index appended. Numbered from zero and contiguous, since there is one per
/// distinct file and at most one file per output.
///
/// The *decoder* rather than the source, because a seek sent here has to pass
/// back through the parser to be any use: a seek is in time, a file is in
/// bytes, and the parser inside `decodebin3` is what converts between them.
/// Sending one to `urisourcebin` instead reaches `filesrc`, which cannot
/// answer it, and the seek is simply refused.
pub const EXTERNAL_AUDIO_DECODER: &str = "extaudio_dec_";

/// The video's own decoder, named so a stream selection can be sent to it
/// rather than to the pipeline. See where it is set for why that matters.
pub const VIDEO_DECODER: &str = "video_dec";

/// The subtitle chain's own source, named so a seek can be delivered to it by
/// hand. There is only ever one, so it needs no number after it.
pub const EXTERNAL_SUBTITLE_SOURCE: &str = "extsub_src";

/// The parser on the same chain. Named for the same reason: switching to a
/// different subtitle takes the whole branch out, and both halves have to be
/// found to be removed.
pub const EXTERNAL_SUBTITLE_PARSER: &str = "extsub_parse";

/// Where one output's audio comes from.
///
/// A track inside the video, or a whole separate file. The second is what
/// makes TinePlayer usable on the films most people actually have: a download
/// carries one language, and the other language - or the described version -
/// arrives as an audio file of its own.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioSource {
    /// A stream inside the video, by the index `--list-tracks` prints.
    Track(u32),
    /// A separate audio file, local or remote.
    File(Source),
}

/// Builds the playback pipeline for `source`.
///
/// The source may be a local file or a remote URL - everything below treats
/// them alike, since `urisourcebin` hides the difference.
///
/// `decodebin3` rather than a named demuxer, which is what makes this
/// container-agnostic: it typefinds the file and picks the demuxer itself, so
/// Matroska, MP4, AVI, MPEG-TS and anything else GStreamer can demux all work
/// through the same code path.
///
/// The reason it is decodebin3 and not plain decodebin is stream selection.
/// decodebin exposes and decodes *every* stream it finds, which on a
/// Blu-ray rip means spinning up a decoder for all five audio tracks to use
/// two of them. decodebin3 decodes only what is selected, via the
/// `select-stream` signal below.
///
/// Because decodebin3 has no pads until it has parsed the file, the branches
/// are built up front and connected in `pad-added`. Building them up front
/// also keeps device errors synchronous: a missing or unplugged output
/// device fails here, with a message naming it, rather than asynchronously
/// once playback has already been started.
///
/// `primary_track`/`secondary_track` of `None` means no audio on that output
/// (no secondary device configured, or "None" chosen explicitly), and that
/// branch is not built at all. Both pointing at the *same* track is
/// supported and gets a `tee`: one decode feeding two devices, which is what
/// you want when two people are listening to the same language on different
/// hardware.
///
/// The video branch always ends in `gtk4paintablesink`, on every platform.
/// It renders into a `GdkPaintable` that the GTK window displays as an
/// ordinary widget, rather than creating its own OS window, which is what
/// lets the application own the window (and therefore its decorations and
/// keyboard input) instead of relaying input back out of a sink-created
/// window. Caller reads the sink's `paintable` property to attach it.
pub fn build_pipeline(
    source: &Source,
    primary_audio: Option<&AudioSource>,
    secondary_audio: Option<&AudioSource>,
    subtitle: Option<&SubtitleSource>,
    // Whether the video offers any subtitle at all, chosen or not. Distinct
    // from `subtitle` being set, and the distinction is the whole point: the
    // overlay has to exist before anybody can switch subtitles on, and a film
    // started with them off would otherwise have nothing to switch into.
    offers_subtitles: bool,
    config: &Config,
) -> Result<(gst::Pipeline, Arc<Mutex<AudioRouting>>), String> {
    let pipeline = gst::Pipeline::new();

    // urisourcebin rather than filesrc so that anything GStreamer can open
    // works: a local file, an HTTP stream from a media server, an SMB share.
    // It picks the right source element for the scheme and adds buffering for
    // the ones that need it.
    //
    // Its pads appear as the source is opened rather than existing up front,
    // so the link to the decoder is made when they arrive. decodebin3 takes a
    // request pad per stream.
    let src = make("urisourcebin")?;
    src.set_property("uri", source.uri());
    let decode = make("decodebin3")?;
    // Named so a stream selection can be delivered straight here.
    //
    // Sending one to the pipeline instead makes it every sink's business, and
    // a sink that cannot pass it upstream drags the whole answer down: an
    // output sitting on None has an unlinked chain, and with it in the
    // pipeline every `SelectStreams` came back refused - which reads as the
    // decoder rejecting the choice rather than as an idle branch that was
    // never asked. This is the one element that decides the question anyway.
    decode.set_property("name", VIDEO_DECODER);
    pipeline
        .add_many([&src, &decode])
        .map_err(|e| e.to_string())?;
    {
        let decode = decode.clone();
        src.connect_pad_added(move |_, pad| {
            let Some(sink) = decode
                .request_pad_simple("sink_%u")
                .or_else(|| decode.static_pad("sink"))
            else {
                eprintln!("Failed to get a decoder sink pad for {}", pad.name());
                return;
            };
            if let Err(e) = pad.link(&sink) {
                eprintln!("Failed to link source to decoder: {e}");
            }
        });
    }

    // Family and size are stored apart so each can be a menu of its own, and
    // joined here into the single description Pango expects.
    let font = format!(
        "{} {}",
        config
            .subtitle_font
            .as_deref()
            .unwrap_or(DEFAULT_SUBTITLE_FONT),
        config.subtitle_size.unwrap_or(DEFAULT_SUBTITLE_SIZE)
    );
    let (video_head, overlay) = build_video_branch(&pipeline, offers_subtitles, &font)?;

    // A subtitle that is not inside the video is its own small source chain,
    // fed into the same overlay an embedded stream would use.
    if let Some(overlay) = overlay.as_ref()
        && let Some(SubtitleSource::Uri(uri)) = subtitle
    {
        attach_external_subtitle(&pipeline, overlay, uri)?;
    }

    // Nothing selected, so the overlay is there to be switched on rather than
    // to draw anything yet. `silent` means not drawn, despite the name.
    if let Some(overlay) = overlay.as_ref()
        && subtitle.is_none()
    {
        overlay.set_property("silent", true);
    }

    // One chain per output, built once and kept for the whole film. Which
    // decoded stream feeds each of them is the routing's business and changes
    // as the viewer changes it; the chain itself, and every setting hanging
    // off it, does not.
    let routing = Arc::new(Mutex::new(AudioRouting::default()));
    {
        let mut routing = routing.lock().unwrap();
        routing.watch(&pipeline);
        for (role, audio) in [("primary", primary_audio), ("secondary", secondary_audio)] {
            // A chain for every output the configuration names a device for,
            // whether or not it starts with anything to play. An output set to
            // None still has a device and still has a menu to be switched on
            // from, and building its chain up front is what lets that happen
            // without rebuilding anything mid-film.
            let named = match role {
                "primary" => config.primary_sink.is_some(),
                _ => config.secondary_sink.is_some(),
            };
            if !named {
                continue;
            }
            routing.chains.insert(
                role.into(),
                build_output_chain(&pipeline, role, config, audio.is_none())?,
            );
            let Some(audio) = audio else { continue };
            routing.want(
                role,
                Some(match audio {
                    AudioSource::Track(track) => Playing::Track(*track),
                    AudioSource::File(file) => Playing::File(file.uri()),
                }),
            );
        }
    }

    // A separate audio file is its own source chain feeding the same kind of
    // hub an embedded track gets, inside the one pipeline - so both run off
    // the same clock and stay in step by construction rather than by being
    // nudged back into line. Grouped by URI, so two outputs on one file cost
    // one decode exactly as two outputs on one track do.
    let mut files: Vec<String> = [primary_audio, secondary_audio]
        .into_iter()
        .flatten()
        .filter_map(|audio| match audio {
            AudioSource::File(file) => Some(file.uri()),
            AudioSource::Track(_) => None,
        })
        .collect();
    files.sort();
    files.dedup();
    for (index, uri) in files.iter().enumerate() {
        attach_external_audio(&pipeline, &routing, uri, index)?;
    }

    let wanted: Vec<u32> = [primary_audio, secondary_audio]
        .into_iter()
        .flatten()
        .filter_map(|audio| match audio {
            AudioSource::Track(track) => Some(*track),
            AudioSource::File(_) => None,
        })
        .collect();
    let wanted_subtitle = match subtitle {
        Some(SubtitleSource::Embedded(index)) => Some(*index),
        _ => None,
    };
    let targets = Arc::new(Targets {
        video: video_head,
        audio: routing.clone(),
        subtitle: overlay,
    });
    // Written by select-stream on a streaming thread and read by pad-added on
    // another, hence Mutex rather than RefCell.
    let selected: Arc<Mutex<HashMap<String, Target>>> = Arc::new(Mutex::new(HashMap::new()));

    connect_stream_selection(
        &decode,
        wanted,
        wanted_subtitle,
        selected.clone(),
        routing.clone(),
    );
    connect_pad_added(&pipeline, &decode, targets, selected);
    connect_pad_removed(&pipeline, &decode, routing.clone());

    // With two audio sinks, GStreamer's default clock election would pick one
    // of them (whichever it finds last, sink to source) as the master clock
    // for the whole pipeline. On Linux this caused a real bug: the two sinks
    // are on genuinely independent hardware clock domains (e.g. HDMI audio vs.
    // a USB headset), and PipeWire auto-suspends an idle device after a few
    // seconds. If the elected clock's device got suspended mid-pause, the
    // whole pipeline stalled on resume, including the *other* sink. Forcing
    // the system clock fixed that.
    //
    // Deliberately Linux-only. WASAPI has no equivalent aggressive idle
    // suspend, and forcing a clock a sink did not choose can make it hold or
    // drop buffers instead of writing them (audio sinks use the pipeline
    // clock to decide *when* to submit each buffer), which matched an
    // observed symptom on Windows of video playing while audio was silent.
    if cfg!(target_os = "linux") {
        pipeline.use_clock(Some(&gst::SystemClock::obtain()));
    }

    Ok((pipeline, routing))
}

fn make(factory: &str) -> Result<gst::Element, String> {
    gst::ElementFactory::make(factory)
        .build()
        .map_err(|_| format!("Missing GStreamer element \"{factory}\". Check the install."))
}

/// Returns the element a decoded video pad should link into.
/// Returns the element a decoded video pad links into, and the overlay
/// subtitles are drawn by when there are any.
///
/// `subtitleoverlay` chooses its own parser and renderer from whatever
/// arrives on its subtitle pad, so text, ASS and DVD subtitles all reach it
/// through the same wiring. It sits on the video branch, so it runs off the
/// same clock as the picture and adds no separate timing to keep in step.
fn build_video_branch(
    pipeline: &gst::Pipeline,
    with_subtitles: bool,
    font: &str,
) -> Result<(gst::Element, Option<gst::Element>), String> {
    let queue = make("queue")?;
    let convert = make("videoconvert")?;
    let sink = gst::ElementFactory::make("gtk4paintablesink")
        .name("vsink")
        .build()
        .map_err(|_| "Missing gtk4paintablesink".to_string())?;

    pipeline
        .add_many([&queue, &convert, &sink])
        .map_err(|e| e.to_string())?;

    if !with_subtitles {
        gst::Element::link_many([&queue, &convert, &sink])
            .map_err(|_| "Failed to link video branch".to_string())?;
        return Ok((queue, None));
    }

    let overlay = gst::ElementFactory::make("subtitleoverlay")
        .name("suboverlay")
        .build()
        .map_err(|_| "Missing GStreamer element \"subtitleoverlay\". Check the install.")?;
    overlay.set_property("font-desc", font);
    // Converted again afterwards: the overlay may hand on a different format
    // from the one it was given.
    let after = make("videoconvert")?;
    pipeline
        .add_many([&overlay, &after])
        .map_err(|e| e.to_string())?;
    gst::Element::link_many([&queue, &convert, &overlay, &after, &sink])
        .map_err(|_| "Failed to link video branch".to_string())?;

    Ok((queue, Some(overlay)))
}

/// `filesrc ! subparse` rather than handing the file straight to the overlay:
/// established by experiment that the overlay cannot preroll from an unparsed
/// file, having no way to tell what format it has been given.
/// Feeds an audio branch from a separate audio file rather than from a stream
/// inside the video.
///
/// The same two elements the video itself goes through, for the same reason:
/// `urisourcebin` so a path and a URL are alike, and `decodebin3` so the
/// container and codec are its problem rather than ours. It lives in the same
/// pipeline as everything else, which is what keeps it on the same clock.
///
/// Both pad-added handlers wait for pads that do not exist yet - the source's
/// appear as the file is opened, the decoder's once it has been parsed.
fn attach_external_audio(
    pipeline: &gst::Pipeline,
    routing: &Arc<Mutex<AudioRouting>>,
    uri: &str,
    index: usize,
) -> Result<(), String> {
    let src = make("urisourcebin")?;
    src.set_property("uri", uri);
    let decode = make("decodebin3")?;
    // Named so a seek can be delivered here by hand. Everything else in the
    // pipeline is seeked through the video's source; this chain has one of its
    // own and does not reliably hear about a seek at all. See `run_seek`.
    decode.set_property("name", format!("{EXTERNAL_AUDIO_DECODER}{index}"));
    pipeline
        .add_many([&src, &decode])
        .map_err(|e| e.to_string())?;

    {
        let decode = decode.clone();
        src.connect_pad_added(move |_, pad| {
            let Some(sink) = decode
                .request_pad_simple("sink_%u")
                .or_else(|| decode.static_pad("sink"))
            else {
                eprintln!("Failed to get a decoder sink pad for the audio file");
                return;
            };
            if let Err(e) = pad.link(&sink) {
                eprintln!("Failed to link the audio file to its decoder: {e}");
            }
        });
    }

    {
        let routing = routing.clone();
        let pipeline = pipeline.clone();
        let uri = uri.to_string();
        decode.connect_pad_added(move |_, pad| {
            // `pad-added` fires for request sink pads as well, so the direction
            // has to be checked before anything is linked to them.
            if pad.direction() != gst::PadDirection::Src {
                return;
            }
            // A file offered as audio can still hold a picture - cover art in
            // an MP3 is the common one - and that is not ours to render.
            //
            // Written as "anything but a picture" rather than "audio only" on
            // purpose. A decodebin3 pad usually has no negotiated caps yet
            // when it appears, so testing for `audio/` rejects the very pad we
            // are waiting for and the file plays nothing at all: the parser
            // stops with `not-linked` and the pipeline reports an internal
            // data stream error, which says nothing about the real cause.
            let media = pad
                .current_caps()
                .unwrap_or_else(|| pad.query_caps(None))
                .structure(0)
                .map(|structure| structure.name().to_string())
                .unwrap_or_default();
            if media.starts_with("video/") || media.starts_with("image/") {
                return;
            }
            // Only the first audio pad of the file. A hub already standing for
            // this URI means the file offered a second stream, which nothing
            // can currently choose between - see the plan's note about picking
            // a track *within* an external file.
            if routing
                .lock()
                .unwrap()
                .carrying
                .values()
                .any(|playing| *playing == Playing::File(uri.clone()))
            {
                return;
            }
            add_audio_hub(&pipeline, &routing, pad, Playing::File(uri.clone()));
        });
    }
    Ok(())
}

pub fn attach_external_subtitle(
    pipeline: &gst::Pipeline,
    overlay: &gst::Element,
    uri: &str,
) -> Result<(), String> {
    // `urisourcebin` rather than `filesrc`, so a subtitle held by a media
    // server opens by exactly the same route as one on disk. It streams the
    // file rather than saving it anywhere: a subtitle is tens of kilobytes,
    // so there is nothing a cache would buy that a second request would not.
    let src = make("urisourcebin")?;
    src.set_property("uri", uri);
    // Named so a seek can be delivered here by hand, for the same reason the
    // external audio source is - this chain has a source of its own and does
    // not reliably hear a seek sent through the video's. See `run_seek`.
    src.set_property("name", EXTERNAL_SUBTITLE_SOURCE);
    let parse = make("subparse")?;
    parse.set_property("name", EXTERNAL_SUBTITLE_PARSER);

    pipeline
        .add_many([&src, &parse])
        .map_err(|e| e.to_string())?;
    // `urisourcebin` has no pads until it has opened what it was given, so the
    // link waits for one to arrive rather than being made now.
    {
        let parse = parse.clone();
        src.connect_pad_added(move |_, pad| {
            let Some(sink) = parse.static_pad("sink") else {
                return;
            };
            if sink.is_linked() {
                return;
            }
            if let Err(e) = pad.link(&sink) {
                eprintln!("Failed to link the subtitle source: {e}");
            }
        });
    }

    let sink_pad = overlay
        .static_pad("subtitle_sink")
        .ok_or("subtitleoverlay has no subtitle pad")?;
    parse
        .static_pad("src")
        .ok_or("subparse has no src pad")?
        .link(&sink_pad)
        .map_err(|e| format!("Failed to attach subtitles: {e}"))?;
    Ok(())
}

/// Builds one output's chain, from the queue that is fed to the device that
/// plays it, and returns its head.
///
/// Built once per output and never rebuilt. What changes while a film plays is
/// only which `tee` this queue is fed from - see [`AudioRouting`] - so
/// everything carrying a setting for the output, the volume element above all,
/// survives every soundtrack change without being re-created or re-read.
///
/// The queue is not optional. A `tee` without one on each branch deadlocks the
/// moment the two sinks consume at even slightly different rates, which two
/// independent audio devices always do.
fn build_output_chain(
    pipeline: &gst::Pipeline,
    role: &str,
    config: &Config,
    // Whether this output starts with nothing to play, which decides whether
    // its sink is allowed to hold up the pipeline waiting to preroll.
    idle: bool,
) -> Result<gst::Element, String> {
    let queue = make("queue")?;
    let convert = make("audioconvert")?;
    let resample = make("audioresample")?;
    // Level and mute for this output alone, which is the point: two people
    // on two devices need two settings. In the pipeline rather than on the
    // sink, so it only ever affects this application - turning a film down
    // must not turn the whole machine down.
    let volume = gst::ElementFactory::make("volume")
        .name(format!("{role}_volume"))
        .build()
        .map_err(|_| "Missing GStreamer element \"volume\". Check the install.".to_string())?;
    volume.set_property("volume", config.volume(role));
    volume.set_property("mute", config.muted(role));
    let sink = build_device_sink(role, config)?;

    // An output that starts on None has a chain with nothing feeding it. A
    // sink gates the pipeline's state change until it has prerolled, and one
    // that is never sent a buffer never will - so the whole pipeline would sit
    // ASYNC and the film would not start at all. Letting this one sink out of
    // that is what makes an empty output cost nothing until it is switched on.
    //
    // The same property Linux sets on every sink, for a different reason - see
    // `build_device_sink` - and with the same consequence, which is that the
    // sink still honors `sync` and so stays in step once it does get audio.
    if idle && sink.find_property("async").is_some() {
        sink.set_property("async", false);
    }

    pipeline
        .add_many([&queue, &convert, &resample, &volume, &sink])
        .map_err(|e| e.to_string())?;
    gst::Element::link_many([&queue, &convert, &resample, &volume, &sink])
        .map_err(|_| format!("Failed to link {role} audio branch"))?;

    Ok(queue)
}

/// How a decoded audio pad is keyed in the routing.
///
/// **The element's name as well as the pad's, because a pad name is unique
/// only within its own element.** Every `decodebin3` in the pipeline calls its
/// first audio pad `audio_0`, and there is one per external audio file besides
/// the video's own - so a bare pad name had the video's audio pad and an
/// external soundtrack's sharing a key, and the second to arrive quietly
/// replaced the first.
///
/// Measured 2026-08-16, from a trace of the routing's own state: after
/// switching an output from a separate soundtrack to a track inside the film,
/// `carrying` held `audio_0=Track(0)` and nothing at all knew where the file
/// was. Switching back then found nothing carrying it, unlinked the output and
/// linked it to nothing - silence, and a picture that froze at the next seek,
/// because a sink with no branch never prerolls and the seek waits for it.
fn hub_key(pad: &gst::Pad) -> String {
    match pad.parent() {
        Some(element) => format!("{}:{}", element.name(), pad.name()),
        None => pad.name().to_string(),
    }
}

/// Puts a `tee` on a decoded audio stream and hands it to the routing, which
/// connects whichever outputs are waiting for that stream.
///
/// Always a `tee`, even for a stream only one output wants. It costs nothing
/// on a single branch, and it means a second output arriving later is a pad
/// request rather than a rebuild - which is the whole reason changing a
/// soundtrack does not interrupt the film.
fn add_audio_hub(
    pipeline: &gst::Pipeline,
    routing: &Arc<Mutex<AudioRouting>>,
    pad: &gst::Pad,
    playing: Playing,
) {
    let name = hub_key(pad);
    let Ok(tee) = gst::ElementFactory::make("tee").build() else {
        eprintln!("Missing GStreamer element \"tee\". Check the install.");
        return;
    };
    // A hub often exists for a moment with nothing drawing from it - between a
    // stream arriving and the output that asked for it being pointed at it -
    // and a tee with no branches is an error rather than a pause by default.
    tee.set_property("allow-not-linked", true);
    if let Err(e) = pipeline.add(&tee) {
        eprintln!("Failed to add the tee for {name}: {e}");
        return;
    }
    // Added to a pipeline that may already be running, so it starts itself
    // rather than waiting for a state change that has already happened.
    if tee.sync_state_with_parent().is_err() {
        eprintln!("Failed to start the tee for {name}");
        return;
    }
    let Some(sink) = tee.static_pad("sink") else {
        return;
    };
    if let Err(e) = pad.link(&sink) {
        eprintln!("Failed to connect decoded audio on {name}: {e}");
        return;
    }

    {
        let mut routing = routing.lock().unwrap();
        trace(format_args!("hub added on {name} carrying {playing:?}"));
        routing.hubs.insert(name.clone(), tee.clone());
        routing.carrying.insert(name, playing);
        routing.reconcile();
    }
    // A separate file needs somebody drawing from it at all times; a track
    // inside the video does not, since its demuxer is paced by the picture.
    //
    // **After the routing has linked whatever wanted this hub, never before.**
    // A `tee` pushes to its branches one after another, and a branch that
    // blocks holds up the rest - so a pacer attached while it was the only
    // branch filled its queue, blocked the tee, and left the output's sink
    // waiting for the one buffer it needs to preroll. The pipeline then never
    // reached PLAYING at all: one frame, in silence, going nowhere.
    // No pacer here, deliberately: a hub exists because an output asked for
    // what it carries, so somebody is drawing from it already. See
    // `attach_pacer` for why attaching one now would deadlock the startup.
    follow_stream_changes(routing, pad);
}

/// Keeps an external file's chain walking in step with the film while no
/// output is drawing from it.
///
/// **Nothing else paces it.** That chain has a source of its own, and the
/// `tee` it feeds drops what it pushes rather than blocking - so the moment
/// the last output leaves, it decodes the whole file as fast as it can and
/// stops at the end. Measured in `harness`: ten seconds of audio consumed in
/// under two, against seventy-seven buffers with this branch attached.
///
/// The alternative was to seek it back on return, and that is worse. A
/// flushing seek makes the output's sink preroll again, and against a file on
/// a network share that is seconds of frozen picture, after which the audio
/// carries on from the seek and the picture is behind it. A chain that never
/// ran away needs no seek at all.
///
/// **Attached only once the output has gone, never while one is connected**,
/// and that is not a tidiness point - it is a deadlock. On Windows the
/// pipeline's clock comes from an audio sink, and that clock reads zero until
/// the sink's ringbuffer starts. A pacer waiting for the clock to advance
/// fills its queue, a full branch blocks the `tee` it hangs off, and the sink
/// being starved is the one that would have started the clock. Measured: with
/// a pacer attached at startup the film sat on one frame indefinitely; with it
/// attached and not syncing, or not attached, it played. Once an output has
/// moved to a track inside the film, that same sink is running off the film's
/// own audio and the clock is live, so there is nothing left to deadlock.
///
/// Deliberately not counted as a sink. It must not gate the pipeline's preroll
/// (`async`), and it must not gate the *end of the film* either: EOS is posted
/// once every sink has seen it, and a soundtrack longer than the picture would
/// otherwise hold the film open after it had plainly finished.
fn attach_pacer(pipeline: &gst::Pipeline, tee: &gst::Element) {
    let (Ok(queue), Ok(sink)) = (
        gst::ElementFactory::make("queue").build(),
        gst::ElementFactory::make("fakesink").build(),
    ) else {
        eprintln!("Could not build the pacing branch for an external audio file");
        return;
    };
    queue.set_property("name", format!("pacerq_{}", tee.name()));
    sink.set_property("name", format!("pacer_{}", tee.name()));
    sink.set_property("sync", true);
    sink.set_property("async", false);
    sink.unset_element_flags(gst::ElementFlags::SINK);
    if pipeline.add_many([&queue, &sink]).is_err() {
        eprintln!("Failed to add the pacing branch for an external audio file");
        return;
    }
    if gst::Element::link_many([tee, &queue, &sink]).is_err() {
        eprintln!("Failed to link the pacing branch for an external audio file");
        return;
    }
    for element in [&queue, &sink] {
        if element.sync_state_with_parent().is_err() {
            eprintln!("Failed to start the pacing branch for an external audio file");
            return;
        }
    }
    trace(format_args!("pacing branch attached to {}", tee.name()));
}

/// Takes the pacing branch off again, once an output is drawing from the hub
/// itself. The counterpart of [`attach_pacer`], and see there for why leaving
/// it on is a deadlock rather than a waste.
fn detach_pacer(pipeline: &gst::Pipeline, tee: &gst::Element) {
    let (Some(queue), Some(sink)) = (
        pipeline.by_name(&format!("pacerq_{}", tee.name())),
        pipeline.by_name(&format!("pacer_{}", tee.name())),
    ) else {
        return;
    };
    // The tee lets go first, so nothing is pushed into elements on their way
    // out - and the request pad is released rather than merely unlinked, since
    // a tee goes on feeding a branch nobody is draining.
    if let Some(pad) = queue.static_pad("sink")
        && let Some(src) = pad.peer()
    {
        let _ = src.unlink(&pad);
        tee.release_request_pad(&src);
    }
    let _ = queue.set_state(gst::State::Null);
    let _ = sink.set_state(gst::State::Null);
    let _ = pipeline.remove_many([&queue, &sink]);
    trace(format_args!("pacing branch taken off {}", tee.name()));
}

/// Follows what a decoded pad is carrying for as long as it exists.
///
/// The reason this is needed at all is that decodebin3 reuses a slot rather
/// than making a new one: asked for a different track, it pushes a fresh
/// stream-start down the pad it already has and no pad event fires anywhere.
/// So the pad's identity is stable and its contents are not, and this is what
/// notices the difference.
///
/// External audio never comes through here. Its chain decodes one file and
/// carries the same thing for the life of the pipeline.
fn follow_stream_changes(routing: &Arc<Mutex<AudioRouting>>, pad: &gst::Pad) {
    let routing = routing.clone();
    pad.add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, move |pad, info| {
        let Some(event) = info.event() else {
            return gst::PadProbeReturn::Ok;
        };
        if event.type_() != gst::EventType::StreamStart {
            return gst::PadProbeReturn::Ok;
        }
        let mut routing = routing.lock().unwrap();
        if let Some(playing) = routing.track_on(pad) {
            let name = hub_key(pad);
            routing.now_carrying(&name, playing);
        }
        gst::PadProbeReturn::Ok
    });
}

/// Takes away the hub on a pad that has gone, once whatever was drawing from
/// it has been let go.
fn remove_audio_hub(pipeline: &gst::Pipeline, routing: &Arc<Mutex<AudioRouting>>, pad: &gst::Pad) {
    let name = hub_key(pad);
    let tee = {
        let mut routing = routing.lock().unwrap();
        trace(format_args!("hub removed on {name}"));
        routing.carrying.remove(&name);
        let Some(tee) = routing.hubs.remove(&name) else {
            return;
        };
        // Before the element goes, so no output is left holding a pad on it.
        routing.reconcile();
        tee
    };
    let _ = tee.set_state(gst::State::Null);
    let _ = pipeline.remove(&tee);
}

/// Notices a decoded audio pad going away, which is what happens when two
/// outputs that were on different tracks are put onto the same one: the
/// selection drops from two audio streams to one and decodebin3 retires the
/// slot it no longer needs.
fn connect_pad_removed(
    pipeline: &gst::Pipeline,
    decode: &gst::Element,
    routing: Arc<Mutex<AudioRouting>>,
) {
    let pipeline = pipeline.clone();
    decode.connect_pad_removed(move |_, pad| {
        if pad.direction() != gst::PadDirection::Src {
            return;
        }
        remove_audio_hub(&pipeline, &routing, pad);
    });
}

/// Holds a sink back by `ms` milliseconds.
///
/// Shared with [`crate::player::Playback::set_offset_ms`] so a delay set while
/// a film is playing is applied exactly as one read from the config at build
/// time. Silently does nothing on a sink without the property, which no audio
/// sink we build should be, rather than failing playback over a setting.
pub fn set_offset(sink: &gst::Element, ms: f64) {
    if sink.find_property("ts-offset").is_none() {
        return;
    }
    let ns = (ms * 1_000_000.0) as i64;
    sink.set_property("ts-offset", ns);
}

/// Creates the real sink element for a configured output device.
///
/// Via `Device::create_element()` rather than a hardcoded factory name plus a
/// platform-specific device string, which is what makes device targeting
/// genuinely cross-platform: pulsesink on Linux, wasapi2sink on Windows,
/// osxaudiosink on macOS, each already configured for the chosen device.
fn build_device_sink(role: &str, config: &Config) -> Result<gst::Element, String> {
    let configured = match role {
        "primary" => config.primary_sink.as_deref(),
        _ => config.secondary_sink.as_deref(),
    };
    let name = configured.ok_or_else(|| format!("{role}_sink not set in config"))?;

    let sink = find_audio_output_device(name)?
        .create_element(Some(&format!("{role}_out")))
        .map_err(|e| format!("Failed to create element for {role} device: {e}"))?;

    // Every sink gates the pipeline's state changes by default, waiting to
    // preroll before the change completes. With two audio sinks on separate
    // devices plus the video sink, a flushing seek left those state changes
    // permanently ASYNC on Linux: pausing after a seek never completed, and
    // playback stopped for good. Measured directly - with this set, resuming
    // returns Success and position advances again; without it, the pipeline
    // reports Playing while no buffers reach either sink.
    //
    // Correct as well as expedient: preroll is the video sink's job here, and
    // the audio sinks still honor `sync`, so they stay in step.
    //
    // Linux-only, matching the forced clock below. Windows is verified
    // working as it is, and this pipeline has a history of platform-specific
    // sink behavior that punishes changing both at once.
    if cfg!(target_os = "linux") && sink.find_property("async").is_some() {
        sink.set_property("async", false);
    }

    // How far this output is held back, so it lines up with the picture and
    // with the other one. `ts-offset` is in nanoseconds and delays rendering
    // by that much; see `Config::offset_ms` for why the figure has to come
    // from a person rather than from the sink.
    set_offset(&sink, config.applied_offset_ms(role));

    Ok(sink)
}

/// Answers decodebin3's question of whether to expose each stream, and
/// records what the ones we accept are for.
///
/// Returning 0 for everything unwanted is the point of using decodebin3:
/// unselected audio tracks and any subtitle streams are never decoded.
fn connect_stream_selection(
    decode: &gst::Element,
    wanted: Vec<u32>,
    wanted_subtitle: Option<u32>,
    selected: Arc<Mutex<HashMap<String, Target>>>,
    routing: Arc<Mutex<AudioRouting>>,
) {
    decode.connect("select-stream", false, move |values| {
        let collection = values[1].get::<gst::StreamCollection>().ok();
        let stream = values[2].get::<gst::Stream>().ok();
        let (Some(collection), Some(stream)) = (collection, stream) else {
            // -1 leaves the decision to decodebin3 rather than silently
            // dropping a stream we failed to inspect.
            return Some((-1i32).to_value());
        };
        let Some(id) = stream.stream_id() else {
            return Some((-1i32).to_value());
        };

        // Taken here rather than off the bus, which is where everything else
        // gets it. The bus is drained by the main loop, and the main loop is
        // not running during the preroll that opens the file - so a pad can
        // and does arrive before the bus has delivered anything, and the
        // routing would have no way to say which track it was looking at.
        // This signal is answered on the spot, once per stream, before any
        // pad exists.
        routing.lock().unwrap().set_collection(collection.clone());

        let kind = stream.stream_type();
        let decision = if kind.contains(gst::StreamType::VIDEO) {
            match ordinal(&collection, &id, gst::StreamType::VIDEO) {
                Some(0) => {
                    selected
                        .lock()
                        .unwrap()
                        .insert(id.to_string(), Target::Video);
                    1
                }
                _ => 0,
            }
        } else if kind.contains(gst::StreamType::AUDIO) {
            match ordinal(&collection, &id, gst::StreamType::AUDIO) {
                Some(track) if wanted.contains(&track) => {
                    selected
                        .lock()
                        .unwrap()
                        .insert(id.to_string(), Target::Audio);
                    1
                }
                _ => 0,
            }
        } else if kind.contains(gst::StreamType::TEXT) {
            match ordinal(&collection, &id, gst::StreamType::TEXT) {
                Some(index) if Some(index) == wanted_subtitle => {
                    selected
                        .lock()
                        .unwrap()
                        .insert(id.to_string(), Target::Subtitle);
                    1
                }
                _ => 0,
            }
        } else {
            0
        };

        Some(decision.to_value())
    });
}

fn connect_pad_added(
    pipeline: &gst::Pipeline,
    decode: &gst::Element,
    targets: Arc<Targets>,
    selected: Arc<Mutex<HashMap<String, Target>>>,
) {
    let pipeline = pipeline.clone();
    decode.connect_pad_added(move |_, pad| {
        // `pad-added` fires for every pad an element gains, in either
        // direction - including the sink pad requested to link the source in.
        // Only decoded output is of interest here.
        if pad.direction() != gst::PadDirection::Src {
            return;
        }
        let Some(id) = pad.stream_id() else {
            eprintln!(
                "Decoded pad {} has no stream id, so nothing knows what it was \
                 selected for; ignoring it. Caps: {:?}",
                pad.name(),
                pad.current_caps()
            );
            return;
        };
        let target = selected.lock().unwrap().get(id.as_str()).copied();
        let target = match target {
            Some(target) => target,
            // A subtitle switched on part way through a film that began with
            // none.
            //
            // This map is filled by `select-stream` while the file is being
            // opened, and an explicit `select-streams` event does not go back
            // through that signal - decodebin3 takes the application's word
            // for it - so a stream chosen later was never recorded here.
            //
            // It only shows up in this one case. Switching *between* embedded
            // subtitles reuses decodebin3's existing text slot and its pad,
            // and so never reaches `pad-added` at all; with nothing selected
            // at the start there is no slot to reuse, and the one it makes
            // arrives here naming a stream nothing knows about.
            //
            // Safe to take at face value: only ever one subtitle is asked for
            // at a time, and the overlay accepts one at a time.
            None if carries_text(pad) => Target::Subtitle,
            // An audio track switched on for an output that had none, which
            // makes a slot rather than reusing one. Unlike the subtitle case
            // this needs no guessing: the routing was told what each output
            // wants before the selection was sent, so the stream can be placed
            // by asking it rather than by inferring from the pad.
            None if carries_audio(pad) => Target::Audio,
            // Anything else decodebin3 exposed without being asked to.
            // Leaving it unlinked is correct; it just plays no part.
            None => return,
        };

        // Audio does not link to a fixed head the way the other two do. Which
        // output a stream feeds changes while the film plays, and may be both
        // of them or neither, so it goes to a `tee` the routing then draws
        // from. See `AudioRouting`.
        if let Target::Audio = target {
            let playing = targets.audio.lock().unwrap().track_on(pad);
            let Some(playing) = playing else {
                eprintln!("Decoded audio on {} names no track we know", pad.name());
                return;
            };
            add_audio_hub(&pipeline, &targets.audio, pad, playing);
            return;
        }

        let head = match target {
            Target::Video => Some(&targets.video),
            _ => targets.subtitle.as_ref(),
        };
        let pad_name = match target {
            Target::Video => "sink",
            _ => "subtitle_sink",
        };
        let Some(head) = head else { return };
        let Some(sink_pad) = head.static_pad(pad_name) else {
            return;
        };
        if let Err(e) = pad.link(&sink_pad) {
            eprintln!("Failed to connect decoded stream {id}: {e}");
        }
    });
}

/// Whether a decoded pad carries audio, asked the same two ways
/// [`carries_text`] asks its question and for the same reasons.
fn carries_audio(pad: &gst::Pad) -> bool {
    if let Some(stream) = pad.stream() {
        return stream.stream_type().contains(gst::StreamType::AUDIO);
    }
    pad.current_caps()
        .unwrap_or_else(|| pad.query_caps(None))
        .structure(0)
        .is_some_and(|structure| structure.name().starts_with("audio/"))
}

/// Whether a decoded pad carries subtitles.
///
/// Asked of the stream the pad was opened with first, and of its caps only if
/// there is no stream to ask. Both, because neither is certain on its own: the
/// stream is there only if decodebin3 attached one to the stream-start event,
/// and a pad often has no negotiated caps when it appears - which is the same
/// trap `attach_external_audio` documents at greater length.
///
/// Three caps families, because a subtitle is text in some containers and a
/// picture in others: `text/` covers SRT and ASS once parsed,
/// `application/x-subtitle` the unparsed forms, and `subpicture/` the bitmap
/// subtitles DVDs and Blu-rays carry.
fn carries_text(pad: &gst::Pad) -> bool {
    if let Some(stream) = pad.stream() {
        return stream.stream_type().contains(gst::StreamType::TEXT);
    }
    let media = pad
        .current_caps()
        .unwrap_or_else(|| pad.query_caps(None))
        .structure(0)
        .map(|structure| structure.name().to_string())
        .unwrap_or_default();
    media.starts_with("text/")
        || media.starts_with("subpicture/")
        || media.starts_with("application/x-subtitle")
}

/// Position of `id` among the collection's streams of the given type, which
/// is the numbering `--list-tracks` prints and the menu offers.
///
/// Shared with alignment, so that the track it measures and the track playback
/// selects are counted the same way. A second counting would be a silent way
/// for the two to disagree.
pub fn ordinal(collection: &gst::StreamCollection, id: &str, kind: gst::StreamType) -> Option<u32> {
    let mut position = 0;
    for index in 0..collection.len() {
        let Some(stream) = collection.stream(index as u32) else {
            continue;
        };
        if !stream.stream_type().contains(kind) {
            continue;
        }
        if stream.stream_id().as_deref() == Some(id) {
            return Some(position);
        }
        position += 1;
    }
    None
}

#[cfg(test)]
mod harness {
    //! What an external audio chain does with nothing drawing from it.
    //!
    //! Built because the same symptom was diagnosed twice by reading and both
    //! answers were wrong. The topology here is the real one in miniature: a
    //! file with a source of its own, decoded into a `tee` that nothing is
    //! linked to, exactly as an external soundtrack is while the output that
    //! was playing it is listening to a track inside the film instead.
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// One at a time. Each of these runs a live pipeline and judges it by what
    /// arrives within a couple of seconds, so two at once on the same machine
    /// measure each other rather than the thing under test - which is exactly
    /// how they first failed, having passed individually.
    fn alone() -> std::sync::MutexGuard<'static, ()> {
        static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
        ONE_AT_A_TIME
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Ten seconds of tone, written where the test can open it.
    fn tone(path: &std::path::Path) {
        let pipeline = gst::parse::launch(&format!(
            "audiotestsrc num-buffers=430 ! audioconvert ! wavenc ! filesink location=\"{}\"",
            path.display().to_string().replace('\\', "/")
        ))
        .expect("the writing pipeline parses");
        pipeline.set_state(gst::State::Playing).expect("it starts");
        let bus = pipeline.bus().expect("it has a bus");
        bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(10),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        );
        let _ = pipeline.set_state(gst::State::Null);
    }

    #[test]
    #[ignore = "runs a live pipeline for a few seconds"]
    fn an_unlinked_chain_races_to_the_end_and_a_seek_brings_it_back() {
        let _alone = alone();
        gst::init().expect("GStreamer initialises");
        let dir = std::env::temp_dir().join("tineplayer-harness");
        std::fs::create_dir_all(&dir).expect("the directory is made");
        let audio = dir.join("tone.wav");
        tone(&audio);

        let pipeline = gst::Pipeline::new();
        let src = make("urisourcebin").expect("urisourcebin");
        src.set_property(
            "uri",
            glib::filename_to_uri(&audio, None)
                .expect("the path is a uri")
                .to_string(),
        );
        let decode = make("decodebin3").expect("decodebin3");
        decode.set_property("name", format!("{EXTERNAL_AUDIO_DECODER}0"));
        pipeline.add_many([&src, &decode]).expect("both go in");
        {
            let decode = decode.clone();
            src.connect_pad_added(move |_, pad| {
                let sink = decode
                    .request_pad_simple("sink_%u")
                    .or_else(|| decode.static_pad("sink"))
                    .expect("a decoder sink pad");
                pad.link(&sink).expect("the source links");
            });
        }

        // The counters the whole question turns on: how much the chain pushes
        // when nobody is drawing, and whether it ends.
        let buffers = Arc::new(AtomicU32::new(0));
        let eos = Arc::new(AtomicU32::new(0));
        {
            let pipeline = pipeline.clone();
            let buffers = buffers.clone();
            let eos = eos.clone();
            decode.connect_pad_added(move |_, pad| {
                if pad.direction() != gst::PadDirection::Src {
                    return;
                }
                let tee = make("tee").expect("tee");
                tee.set_property("allow-not-linked", true);
                pipeline.add(&tee).expect("the tee goes in");
                tee.sync_state_with_parent().expect("the tee starts");
                pad.link(&tee.static_pad("sink").expect("a tee sink pad"))
                    .expect("the decoder links to the tee");
                let counted = buffers.clone();
                let ended = eos.clone();
                pad.add_probe(
                    gst::PadProbeType::BUFFER | gst::PadProbeType::EVENT_DOWNSTREAM,
                    move |_, info| {
                        match info.data {
                            Some(gst::PadProbeData::Buffer(_)) => {
                                counted.fetch_add(1, Ordering::Relaxed);
                            }
                            Some(gst::PadProbeData::Event(ref event))
                                if event.type_() == gst::EventType::Eos =>
                            {
                                ended.fetch_add(1, Ordering::Relaxed);
                            }
                            _ => {}
                        }
                        gst::PadProbeReturn::Ok
                    },
                );
            });
        }

        pipeline.set_state(gst::State::Playing).expect("it plays");
        std::thread::sleep(std::time::Duration::from_secs(2));
        let raced = buffers.load(Ordering::Relaxed);
        let ended = eos.load(Ordering::Relaxed);
        println!("unlinked for 2s: {raced} buffers pushed, EOS seen {ended} time(s)");

        // Now the return: the seek `seek_external_audio` sends, by hand.
        let decoder = pipeline
            .by_name(&format!("{EXTERNAL_AUDIO_DECODER}0"))
            .expect("the decoder is findable by name");
        let taken = decoder.send_event(seek_to(2));
        std::thread::sleep(std::time::Duration::from_secs(1));
        let after = buffers.load(Ordering::Relaxed);
        println!("seek accepted: {taken}; buffers after the seek: {after} (was {raced})");

        let _ = pipeline.set_state(gst::State::Null);
        assert!(ended > 0, "the chain should have run to the end unattended");
        assert!(taken, "the decoder should take a seek by name");
        assert!(after > raced, "the seek should start it flowing again");
    }

    fn seek_to(seconds: u64) -> gst::Event {
        gst::event::Seek::new(
            1.0,
            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
            gst::SeekType::Set,
            gst::ClockTime::from_seconds(seconds),
            gst::SeekType::None,
            gst::ClockTime::NONE,
        )
    }

    /// What an output hears after it comes back to a file that has run away.
    ///
    /// Counts buffers and EOS arriving at the far end of a branch requested
    /// from the `tee` *after* the chain has already finished, which is the
    /// whole of what "switching back is silent" is about.
    fn returning(order: &str) -> (u32, u32) {
        gst::init().expect("GStreamer initialises");
        let dir = std::env::temp_dir().join("tineplayer-harness");
        std::fs::create_dir_all(&dir).expect("the directory is made");
        let audio = dir.join("tone.wav");
        if !audio.exists() {
            tone(&audio);
        }

        let pipeline = gst::Pipeline::new();
        let src = make("urisourcebin").expect("urisourcebin");
        src.set_property(
            "uri",
            glib::filename_to_uri(&audio, None)
                .expect("the path is a uri")
                .to_string(),
        );
        let decode = make("decodebin3").expect("decodebin3");
        decode.set_property("name", format!("{EXTERNAL_AUDIO_DECODER}0"));
        pipeline.add_many([&src, &decode]).expect("both go in");
        {
            let decode = decode.clone();
            src.connect_pad_added(move |_, pad| {
                let sink = decode
                    .request_pad_simple("sink_%u")
                    .or_else(|| decode.static_pad("sink"))
                    .expect("a decoder sink pad");
                pad.link(&sink).expect("the source links");
            });
        }
        let hub: Arc<Mutex<Option<gst::Element>>> = Arc::new(Mutex::new(None));
        {
            let pipeline = pipeline.clone();
            let hub = hub.clone();
            decode.connect_pad_added(move |_, pad| {
                if pad.direction() != gst::PadDirection::Src {
                    return;
                }
                let tee = make("tee").expect("tee");
                tee.set_property("allow-not-linked", true);
                pipeline.add(&tee).expect("the tee goes in");
                tee.sync_state_with_parent().expect("the tee starts");
                pad.link(&tee.static_pad("sink").expect("a tee sink pad"))
                    .expect("the decoder links to the tee");
                *hub.lock().unwrap() = Some(tee);
            });
        }

        pipeline.set_state(gst::State::Playing).expect("it plays");
        // Long enough for the chain to finish the file with nobody drawing.
        std::thread::sleep(std::time::Duration::from_secs(2));

        let decoder = pipeline
            .by_name(&format!("{EXTERNAL_AUDIO_DECODER}0"))
            .expect("the decoder is findable by name");
        let tee = hub.lock().unwrap().clone().expect("a hub by now");

        // The output's chain, built the way `point` links one: a request pad
        // on the tee feeding a queue and a sink.
        let heard = Arc::new(AtomicU32::new(0));
        let ended = Arc::new(AtomicU32::new(0));
        let link = || {
            let queue = make("queue").expect("queue");
            let sink = make("fakesink").expect("fakesink");
            sink.set_property("sync", false);
            pipeline.add_many([&queue, &sink]).expect("both go in");
            gst::Element::link_many([&queue, &sink]).expect("they link");
            let pad = queue.static_pad("sink").expect("a queue sink pad");
            let heard = heard.clone();
            let ended = ended.clone();
            pad.add_probe(
                gst::PadProbeType::BUFFER | gst::PadProbeType::EVENT_DOWNSTREAM,
                move |_, info| {
                    match info.data {
                        Some(gst::PadProbeData::Buffer(_)) => {
                            heard.fetch_add(1, Ordering::Relaxed);
                        }
                        Some(gst::PadProbeData::Event(ref event))
                            if event.type_() == gst::EventType::Eos =>
                        {
                            ended.fetch_add(1, Ordering::Relaxed);
                        }
                        _ => {}
                    }
                    gst::PadProbeReturn::Ok
                },
            );
            let src = tee.request_pad_simple("src_%u").expect("a tee src pad");
            src.link(&pad).expect("the branch links");
            queue.sync_state_with_parent().expect("the queue starts");
            sink.sync_state_with_parent().expect("the sink starts");
        };

        match order {
            // What the application does now: seek, then link at once.
            "seek then link" => {
                decoder.send_event(seek_to(2));
                link();
            }
            // Link first and let the flush pass through the new branch.
            "link then seek" => {
                link();
                decoder.send_event(seek_to(2));
            }
            // Seek, wait for it to settle, and only then link.
            _ => {
                decoder.send_event(seek_to(2));
                std::thread::sleep(std::time::Duration::from_millis(300));
                link();
            }
        }

        std::thread::sleep(std::time::Duration::from_secs(1));
        let _ = pipeline.set_state(gst::State::Null);
        (heard.load(Ordering::Relaxed), ended.load(Ordering::Relaxed))
    }

    /// Whether a branch of its own keeps the chain at walking pace.
    ///
    /// The alternative to seeking it back on return, and better than seeking
    /// if it holds: a chain that never ran away needs no flush, and a flush is
    /// what makes the whole pipeline wait for a sink to preroll again.
    fn paced() -> u32 {
        gst::init().expect("GStreamer initialises");
        let dir = std::env::temp_dir().join("tineplayer-harness");
        std::fs::create_dir_all(&dir).expect("the directory is made");
        let audio = dir.join("tone.wav");
        if !audio.exists() {
            tone(&audio);
        }

        let pipeline = gst::Pipeline::new();
        let src = make("urisourcebin").expect("urisourcebin");
        src.set_property(
            "uri",
            glib::filename_to_uri(&audio, None)
                .expect("the path is a uri")
                .to_string(),
        );
        let decode = make("decodebin3").expect("decodebin3");
        pipeline.add_many([&src, &decode]).expect("both go in");
        {
            let decode = decode.clone();
            src.connect_pad_added(move |_, pad| {
                let sink = decode
                    .request_pad_simple("sink_%u")
                    .or_else(|| decode.static_pad("sink"))
                    .expect("a decoder sink pad");
                pad.link(&sink).expect("the source links");
            });
        }
        let counted = Arc::new(AtomicU32::new(0));
        {
            let pipeline = pipeline.clone();
            let counted = counted.clone();
            decode.connect_pad_added(move |_, pad| {
                if pad.direction() != gst::PadDirection::Src {
                    return;
                }
                let tee = make("tee").expect("tee");
                tee.set_property("allow-not-linked", true);
                // The pacing branch: nothing listens to it, and holding each
                // buffer until its time is the whole of its job.
                let queue = make("queue").expect("queue");
                let sink = make("fakesink").expect("fakesink");
                sink.set_property("sync", true);
                sink.set_property("async", false);
                pipeline
                    .add_many([&tee, &queue, &sink])
                    .expect("all three go in");
                gst::Element::link_many([&tee, &queue, &sink]).expect("they link");
                for element in [&tee, &queue, &sink] {
                    element.sync_state_with_parent().expect("it starts");
                }
                pad.link(&tee.static_pad("sink").expect("a tee sink pad"))
                    .expect("the decoder links to the tee");
                let counted = counted.clone();
                pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                    counted.fetch_add(1, Ordering::Relaxed);
                    gst::PadProbeReturn::Ok
                });
            });
        }

        pipeline.set_state(gst::State::Playing).expect("it plays");
        std::thread::sleep(std::time::Duration::from_secs(2));
        let pushed = counted.load(Ordering::Relaxed);
        let _ = pipeline.set_state(gst::State::Null);
        pushed
    }

    /// Ten seconds of tone is 430 buffers, so two seconds of it is about 86.
    /// Unpaced, the same chain pushes all 430 and stops at the end - which is
    /// what makes coming back to it silence.
    #[test]
    #[ignore = "runs live pipelines for several seconds"]
    fn a_branch_of_its_own_keeps_the_chain_at_walking_pace() {
        let _alone = alone();
        let pushed = paced();
        println!("paced: {pushed} buffers in 2s (the whole file is 430)");
        assert!(
            (40..200).contains(&pushed),
            "should be about two seconds' worth, got {pushed}"
        );
    }

    /// **The order the application has to use, and the two that are silent.**
    ///
    /// Measured 2026-08-16, after the same symptom had been diagnosed twice by
    /// reading and answered wrongly both times. A branch requested from the
    /// `tee` once the chain has finished receives nothing at all, and waiting
    /// for the seek to settle first does not help - what matters is that the
    /// branch is already there when the flush passes through it.
    #[test]
    #[ignore = "runs live pipelines for several seconds"]
    fn coming_back_to_a_chain_that_has_run_away() {
        let _alone = alone();
        let (heard, _) = returning("link then seek");
        assert!(heard > 0, "linking before the seek should play: {heard}");

        for silent in ["seek then link", "seek, settle, link"] {
            let (heard, _) = returning(silent);
            assert_eq!(heard, 0, "\"{silent}\" is the order that loses the audio");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A routing that wants and carries the given things, with no pipeline
    /// behind it. Enough to ask what [`AudioRouting::settle`] would decide,
    /// which is the whole of what is worth testing without elements.
    fn routing(wanted: &[(&str, Playing)], carrying: &[(&str, Playing)]) -> AudioRouting {
        let mut routing = AudioRouting::default();
        for (role, playing) in wanted {
            routing.wanted.insert((*role).into(), playing.clone());
        }
        for (pad, playing) in carrying {
            routing.carrying.insert((*pad).into(), playing.clone());
        }
        routing
    }

    #[test]
    fn an_external_file_is_settled_at_once() {
        let file = Playing::File("file:///described.mp3".into());
        let routing = routing(&[("primary", file.clone())], &[("extaudio", file)]);
        assert!(routing.can_settle("primary"));
    }

    #[test]
    fn a_track_the_other_output_already_plays_is_settled_at_once() {
        let routing = routing(
            &[
                ("primary", Playing::Track(1)),
                ("secondary", Playing::Track(1)),
            ],
            &[("audio_0", Playing::Track(1))],
        );
        assert!(routing.can_settle("secondary"));
    }

    #[test]
    fn an_output_turned_off_is_settled_at_once() {
        let routing = routing(&[], &[("audio_0", Playing::Track(0))]);
        assert!(routing.can_settle("secondary"));
    }

    /// The case that must *not* settle: a stream has been asked for and is on
    /// its way, and the output keeps what it has until the pad arrives.
    #[test]
    fn a_track_not_yet_decoded_waits_for_its_pad() {
        let routing = routing(
            &[("primary", Playing::Track(2))],
            &[("audio_0", Playing::Track(0))],
        );
        assert!(!routing.can_settle("primary"));
    }
}
