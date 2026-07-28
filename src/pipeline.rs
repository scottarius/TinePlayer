use std::path::Path;

use gst::prelude::*;
use gstreamer as gst;

use crate::config::Config;
use crate::devices::find_audio_output_device;

/// Structural description only — no dynamic text (file paths, device
/// identifiers) is embedded in the pipeline string. GStreamer's pipeline
/// mini-language treats characters like "(" ")" as real syntax (bin
/// grouping), and real movie filenames commonly contain them
/// (e.g. "Movie (2019).mkv"), which breaks naive string interpolation.
/// The file path is set as a real property below instead, which sidesteps
/// the parser entirely.
///
/// A `queue` immediately after each of matroskademux's branch points is
/// required, not optional: a single demuxer feeding multiple downstream
/// branches needs a queue on each branch to give it its own thread,
/// otherwise a slow/blocked branch (e.g. a decodebin still autoplugging)
/// can stall the demuxer's single push thread and starve the other
/// branches indefinitely with no error.
///
/// `primary_track`/`secondary_track` of `None` means no audio track is
/// assigned to that output at all (e.g. no secondary device configured, or
/// the user explicitly chose "None") — that branch is simply omitted from
/// the pipeline entirely, rather than built and left unused.
///
/// Each audio branch stops at a named `audioresample` — the actual sink
/// element is created via `Device::create_element()` below for genuine
/// cross-platform device targeting (pulsesink on Linux, wasapi2sink on
/// Windows) instead of hardcoding a sink factory name plus a
/// platform-specific device-identifier string.
///
/// The video branch always ends in `gtk4paintablesink`, on every platform.
/// It renders into a `GdkPaintable` that the GTK window displays as an
/// ordinary widget, rather than creating its own OS window — which is what
/// lets the application own the window (and therefore its decorations and
/// keyboard input) instead of relaying input back out of a sink-created
/// window. Caller reads the sink's `paintable` property to attach it.
pub fn build_pipeline(
    path: &Path,
    primary_track: Option<u32>,
    secondary_track: Option<u32>,
    config: &Config,
) -> Result<gst::Pipeline, String> {
    let mut description = String::from(
        "filesrc name=src ! matroskademux name=d \
         d.video_0 ! queue ! decodebin ! videoconvert ! gtk4paintablesink name=vsink",
    );
    if let Some(track) = primary_track {
        description.push_str(&format!(
            " d.audio_{track} ! queue ! decodebin ! audioconvert ! audioresample name=primary_resample"
        ));
    }
    if let Some(track) = secondary_track {
        description.push_str(&format!(
            " d.audio_{track} ! queue ! decodebin ! audioconvert ! audioresample name=secondary_resample"
        ));
    }

    let pipeline = gst::parse::launch(&description)
        .map_err(|e| format!("Failed to build pipeline: {e}"))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "Parsed pipeline was not a gst::Pipeline".to_string())?;

    pipeline
        .by_name("src")
        .ok_or("missing src element")?
        .set_property("location", path.to_string_lossy().to_string());

    if primary_track.is_some() {
        attach_sink(&pipeline, "primary", config.primary_sink.as_deref())?;
    }
    if secondary_track.is_some() {
        attach_sink(&pipeline, "secondary", config.secondary_sink.as_deref())?;
    }

    // With two audio sinks, GStreamer's default clock-election would pick
    // one of them (whichever it finds last, sink to source) as the master
    // clock for the whole pipeline. On Linux this caused a real bug: our
    // two sinks are on genuinely independent hardware clock domains (e.g.
    // HDMI audio vs. a USB headset), and PipeWire auto-suspends an idle
    // device after a few seconds — if the elected clock's device got
    // suspended mid-pause, the whole pipeline stalled on resume, including
    // the *other* sink. Forcing the system clock fixed that.
    //
    // Windows-only note: this is deliberately Linux-only. WASAPI doesn't
    // have PipeWire's aggressive idle-suspend behavior, so the problem this
    // works around may not even occur here — and forcing a clock a sink
    // didn't choose can make it hold or drop buffers instead of writing
    // them (audio sinks use the pipeline clock to decide *when* to submit
    // each buffer to the device), which matches an observed symptom on
    // Windows of video playing fine while audio was completely silent.
    if cfg!(target_os = "linux") {
        pipeline.use_clock(Some(&gst::SystemClock::obtain()));
    }

    Ok(pipeline)
}

/// Creates the real audio sink element for `sink_name` (a device display
/// name from config) and links it onto `<prefix>_resample`, which must
/// already exist in `pipeline` (i.e. the caller only calls this when that
/// branch was actually included in the pipeline description).
fn attach_sink(
    pipeline: &gst::Pipeline,
    prefix: &str,
    sink_name: Option<&str>,
) -> Result<(), String> {
    let sink_name = sink_name.ok_or_else(|| format!("{prefix}_sink not set in config"))?;
    let device = find_audio_output_device(sink_name)?;
    let sink = device
        .create_element(Some(&format!("{prefix}_out")))
        .map_err(|e| format!("Failed to create element for {prefix} device: {e}"))?;

    pipeline.add(&sink).map_err(|e| e.to_string())?;

    let resample = pipeline
        .by_name(&format!("{prefix}_resample"))
        .ok_or_else(|| format!("missing {prefix}_resample"))?;
    resample
        .link(&sink)
        .map_err(|e| format!("Failed to link {prefix} sink: {e}"))?;

    Ok(())
}
