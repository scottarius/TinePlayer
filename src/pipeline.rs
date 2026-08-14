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
}

impl AudioRouting {
    /// Says what an output should be playing from now on. Takes effect at the
    /// next [`Self::reconcile`].
    pub fn want(&mut self, role: &str, playing: Option<Playing>) {
        match playing {
            Some(playing) => self.wanted.insert(role.into(), playing),
            None => self.wanted.remove(role),
        };
    }

    /// What an output is meant to be playing.
    pub fn wanted_by(&self, role: &str) -> Option<&Playing> {
        self.wanted.get(role)
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
            self.point(&role, hub.as_ref());
        }
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
            return;
        }

        if let Some(pad) = self.links.remove(role) {
            let _ = pad.unlink(&sink);
            if let Some(tee) = pad.parent_element() {
                tee.release_request_pad(&pad);
            }
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
        self.links.insert(role.into(), src);
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
        for (role, audio) in [("primary", primary_audio), ("secondary", secondary_audio)] {
            let Some(audio) = audio else { continue };
            routing
                .chains
                .insert(role.into(), build_output_chain(&pipeline, role, config)?);
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

    pipeline
        .add_many([&queue, &convert, &resample, &volume, &sink])
        .map_err(|e| e.to_string())?;
    gst::Element::link_many([&queue, &convert, &resample, &volume, &sink])
        .map_err(|_| format!("Failed to link {role} audio branch"))?;

    Ok(queue)
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
    let name = pad.name().to_string();
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
        routing.hubs.insert(name.clone(), tee);
        routing.carrying.insert(name, playing);
        routing.reconcile();
    }
    follow_stream_changes(routing, pad);
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
            let name = pad.name().to_string();
            routing.now_carrying(&name, playing);
        }
        gst::PadProbeReturn::Ok
    });
}

/// Takes away the hub on a pad that has gone, once whatever was drawing from
/// it has been let go.
fn remove_audio_hub(pipeline: &gst::Pipeline, routing: &Arc<Mutex<AudioRouting>>, pad: &gst::Pad) {
    let name = pad.name().to_string();
    let tee = {
        let mut routing = routing.lock().unwrap();
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
