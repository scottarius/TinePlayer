use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use gst::prelude::*;
use gstreamer as gst;

use crate::config::Config;
use crate::devices::find_audio_output_device;
use crate::subtitles::SubtitleChoice;

/// Pango leaves the family unspecified by default, which resolves to a serif
/// face. Bold with the renderer's black outline is what stays legible against
/// a moving picture.
///
/// The number is smaller than it looks: the renderer scales the font by the
/// video's width, so on a 1080p frame this size draws text 46 pixels tall,
/// about 4.3% of the frame height. Measured, because the same description at
/// 24 came out at 93 pixels and dominated the picture.
const DEFAULT_SUBTITLE_FONT: &str = "Sans Bold 12";

/// What a stream was selected for, recorded when decodebin3 asks whether to
/// expose it and read back when its pad actually appears.
#[derive(Clone, Copy)]
enum Target {
    Video,
    /// A subtitle stream inside the file. Only one is ever selected, so
    /// unlike audio there is no index to route by.
    Subtitle,
    /// Index among the file's audio streams, matching what `--list-tracks`
    /// prints.
    Audio(u32),
}

/// The head element of each branch, i.e. the thing a decoded pad links into.
struct Targets {
    video: gst::Element,
    audio: HashMap<u32, gst::Element>,
    /// The overlay subtitles are drawn by, when there are any.
    subtitle: Option<gst::Element>,
}

/// Builds the playback pipeline for `path`.
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
    path: &Path,
    primary_track: Option<u32>,
    secondary_track: Option<u32>,
    subtitle: Option<&SubtitleChoice>,
    config: &Config,
) -> Result<gst::Pipeline, String> {
    let pipeline = gst::Pipeline::new();

    // Set as a property rather than parsed from a pipeline string: GStreamer's
    // pipeline mini-language treats "(" and ")" as bin grouping, and real
    // filenames commonly contain them (e.g. "Movie (2019).mkv").
    let src = make("filesrc")?;
    src.set_property("location", path.to_string_lossy().to_string());
    let decode = make("decodebin3")?;
    pipeline
        .add_many([&src, &decode])
        .map_err(|e| e.to_string())?;
    src.link(&decode)
        .map_err(|_| "Failed to link source to decoder".to_string())?;

    let font = config
        .subtitle_font
        .as_deref()
        .unwrap_or(DEFAULT_SUBTITLE_FONT);
    let (video_head, overlay) = build_video_branch(&pipeline, subtitle.is_some(), font)?;

    // A subtitle file beside the video is its own small source chain, fed
    // into the same overlay an embedded stream would use.
    if let (Some(SubtitleChoice::External(file)), Some(overlay)) = (subtitle, overlay.as_ref()) {
        attach_external_subtitle(&pipeline, overlay, file)?;
    }

    // Grouped by track so that one decoded stream can feed two outputs
    // instead of being decoded twice.
    let mut roles_by_track: HashMap<u32, Vec<&str>> = HashMap::new();
    if let Some(track) = primary_track {
        roles_by_track.entry(track).or_default().push("primary");
    }
    if let Some(track) = secondary_track {
        roles_by_track.entry(track).or_default().push("secondary");
    }

    let mut audio_heads = HashMap::new();
    for (track, roles) in &roles_by_track {
        audio_heads.insert(*track, build_audio_branch(&pipeline, roles, config)?);
    }

    let wanted: Vec<u32> = roles_by_track.keys().copied().collect();
    let wanted_subtitle = match subtitle {
        Some(SubtitleChoice::Embedded(index)) => Some(*index),
        _ => None,
    };
    let targets = Arc::new(Targets {
        video: video_head,
        audio: audio_heads,
        subtitle: overlay,
    });
    // Written by select-stream on a streaming thread and read by pad-added on
    // another, hence Mutex rather than RefCell.
    let selected: Arc<Mutex<HashMap<String, Target>>> = Arc::new(Mutex::new(HashMap::new()));

    connect_stream_selection(&decode, wanted, wanted_subtitle, selected.clone());
    connect_pad_added(&decode, targets, selected);

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

    Ok(pipeline)
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

    let overlay = make("subtitleoverlay")?;
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
fn attach_external_subtitle(
    pipeline: &gst::Pipeline,
    overlay: &gst::Element,
    file: &Path,
) -> Result<(), String> {
    let src = make("filesrc")?;
    src.set_property("location", file.to_string_lossy().to_string());
    let parse = make("subparse")?;

    pipeline
        .add_many([&src, &parse])
        .map_err(|e| e.to_string())?;
    src.link(&parse)
        .map_err(|_| "Failed to link subtitle file".to_string())?;

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

/// Builds the output chain(s) fed by one audio stream and returns the element
/// its decoded pad should link into.
///
/// With two roles on one track the head is a `tee`, and each branch off it
/// needs its own `queue`: a tee without queues on its branches deadlocks as
/// soon as the two sinks consume at even slightly different rates, which two
/// independent audio devices always do.
fn build_audio_branch(
    pipeline: &gst::Pipeline,
    roles: &[&str],
    config: &Config,
) -> Result<gst::Element, String> {
    let head = if roles.len() > 1 {
        make("tee")?
    } else {
        make("queue")?
    };
    pipeline.add(&head).map_err(|e| e.to_string())?;

    for role in roles {
        let queue = make("queue")?;
        let convert = make("audioconvert")?;
        let resample = make("audioresample")?;
        let sink = build_device_sink(role, config)?;

        pipeline
            .add_many([&queue, &convert, &resample, &sink])
            .map_err(|e| e.to_string())?;
        gst::Element::link_many([&queue, &convert, &resample, &sink])
            .map_err(|_| format!("Failed to link {role} audio branch"))?;
        // Requests a src pad from the tee, or uses the queue's static one.
        head.link(&queue)
            .map_err(|_| format!("Failed to link {role} output"))?;
    }

    Ok(head)
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
    // playback stopped for good. Measured directly — with this set, resuming
    // returns Success and position advances again; without it, the pipeline
    // reports Playing while no buffers reach either sink.
    //
    // Correct as well as expedient: preroll is the video sink's job here, and
    // the audio sinks still honour `sync`, so they stay in step.
    //
    // Linux-only, matching the forced clock below. Windows is verified
    // working as it is, and this pipeline has a history of platform-specific
    // sink behaviour that punishes changing both at once.
    if cfg!(target_os = "linux") && sink.find_property("async").is_some() {
        sink.set_property("async", false);
    }

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
                        .insert(id.to_string(), Target::Audio(track));
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
    decode: &gst::Element,
    targets: Arc<Targets>,
    selected: Arc<Mutex<HashMap<String, Target>>>,
) {
    decode.connect_pad_added(move |_, pad| {
        let Some(id) = pad.stream_id() else {
            eprintln!("Decoded pad has no stream id; ignoring it");
            return;
        };
        let target = selected.lock().unwrap().get(id.as_str()).copied();
        let Some(target) = target else {
            // A stream decodebin3 exposed without being asked to. Leaving it
            // unlinked is correct; it just plays no part.
            return;
        };

        let (head, pad_name) = match target {
            Target::Video => (Some(&targets.video), "sink"),
            Target::Audio(track) => (targets.audio.get(&track), "sink"),
            Target::Subtitle => (targets.subtitle.as_ref(), "subtitle_sink"),
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

/// Position of `id` among the collection's streams of the given type, which
/// is the numbering `--list-tracks` prints and the menu offers.
fn ordinal(collection: &gst::StreamCollection, id: &str, kind: gst::StreamType) -> Option<u32> {
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
