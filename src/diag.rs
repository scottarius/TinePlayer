//! SCAFFOLDING - branch fix/linux-seek-audio only. Must not reach main.
//!
//! Counts buffers at three points down each audio branch so a stall can be
//! located rather than inferred. `TINEPLAYER_SEEK_PROBE=1` turns it on.
//!
//! These numbers say *where* data stopped, never *whether* audio is playing.
//! That distinction is the whole reason this file is careful: four in-process
//! metrics once reported healthy while the audio was audibly gone, and two
//! false "it's fixed" claims came from believing them. Only the recording in
//! `diag/seektest.sh` decides whether sound reached a device. Read these
//! counters alongside it, never instead of it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use gst::prelude::*;
use gstreamer as gst;

/// How far each buffer reaching a sink is from the clock, in milliseconds.
struct Timing {
    segment: Option<gst::FormattedSegment<gst::ClockTime>>,
    min: i64,
    max: i64,
    seen: u64,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            segment: None,
            min: i64::MAX,
            max: i64::MIN,
            seen: 0,
        }
    }
}

/// Where each branch is watched: entering its own chain, immediately before
/// the sink, and at the sink's own pad. A stall between two of these points
/// names the element that stopped passing data.
const POINTS: [(&str, &str, &str); 3] = [
    ("queue", "{role}_queue", "sink"),
    ("vol", "{role}_volume", "src"),
    ("sink", "{role}_out", "sink"),
];

pub fn install(pipeline: &gst::Pipeline) {
    if std::env::var("TINEPLAYER_SEEK_PROBE").as_deref() != Ok("1") {
        return;
    }

    // Name, buffers seen, non-zero bytes, total bytes.
    type Counter = (String, Arc<AtomicU64>, Arc<AtomicU64>, Arc<AtomicU64>);
    let mut counters: Vec<Counter> = Vec::new();
    let mut timings: Vec<(String, Arc<Mutex<Timing>>)> = Vec::new();

    for role in ["primary", "secondary"] {
        for (label, element, pad_name) in POINTS {
            let element_name = element.replace("{role}", role);
            let Some(element) = pipeline.by_name(&element_name) else {
                // Expected for the secondary when only one output is
                // configured, so it is a note rather than a complaint.
                eprintln!("PROBE: no element {element_name}");
                continue;
            };
            let Some(pad) = element.static_pad(pad_name) else {
                eprintln!("PROBE: {element_name} has no {pad_name} pad");
                continue;
            };

            let count = Arc::new(AtomicU64::new(0));
            let nonzero = Arc::new(AtomicU64::new(0));
            let bytes = Arc::new(AtomicU64::new(0));
            let name = format!("{role}/{label}");
            counters.push((
                name.clone(),
                count.clone(),
                nonzero.clone(),
                bytes.clone(),
            ));

            // Events are printed as they happen rather than counted: there are
            // few of them, and which pad saw the flush - and whether it ever
            // saw the matching stop - is the question being asked.
            // EVENT_FLUSH is a probe type of its own: flush events travel
            // downstream but are *not* delivered by EVENT_DOWNSTREAM, so
            // watching only that mask shows no flushes and looks like evidence
            // that none arrived.
            pad.add_probe(
                gst::PadProbeType::BUFFER
                    | gst::PadProbeType::EVENT_DOWNSTREAM
                    | gst::PadProbeType::EVENT_FLUSH,
                move |_, info| {
                    match &info.data {
                        Some(gst::PadProbeData::Buffer(buffer)) => {
                            count.fetch_add(1, Ordering::Relaxed);
                            // The share of non-zero bytes, rather than a real
                            // RMS. It needs no knowledge of sample format,
                            // width, channel count or interleaving - any of
                            // which could be got wrong and produce a
                            // confident wrong number - and it answers the only
                            // question being asked: are these buffers silence?
                            // Digital silence is zero bytes in every PCM
                            // format there is.
                            if let Ok(map) = buffer.map_readable() {
                                let slice = map.as_slice();
                                bytes.fetch_add(slice.len() as u64, Ordering::Relaxed);
                                let live = slice.iter().filter(|b| **b != 0).count();
                                nonzero.fetch_add(live as u64, Ordering::Relaxed);
                            }
                        }
                        Some(gst::PadProbeData::Event(event)) => {
                            use gst::EventType::*;
                            if matches!(
                                event.type_(),
                                FlushStart | FlushStop | Segment | Eos | StreamStart | Gap
                            ) {
                                eprintln!("PROBE {name} event {:?}", event.type_());
                            }
                        }
                        _ => {}
                    }
                    gst::PadProbeReturn::Ok
                },
            );
        }
    }

    if counters.is_empty() {
        eprintln!("PROBE: nothing to watch");
        return;
    }

    // How far ahead of, or behind, the clock each buffer is when it reaches
    // the sink. This is the question PulseAudio could not answer: its Buffer
    // Latency and Sink Latency read 0 under PipeWire's compatibility layer at
    // all times, including while audio was plainly playing, so they measure
    // nothing here.
    //
    // A buffer's running time against the pipeline's own running time says it
    // directly. Near zero is normal. Large and growing means the sink is
    // writing far ahead of now, so the device plays silence while it waits.
    // Negative means the buffers are late, which is the other way a sink ends
    // up rendering nothing anyone hears.
    for role in ["primary", "secondary"] {
        let Some(sink) = pipeline.by_name(&format!("{role}_out")) else {
            continue;
        };
        let Some(pad) = sink.static_pad("sink") else {
            continue;
        };
        let state: Arc<Mutex<Timing>> = Arc::default();
        timings.push((role.to_string(), state.clone()));
        let element = sink.clone();
        pad.add_probe(
            gst::PadProbeType::BUFFER | gst::PadProbeType::EVENT_DOWNSTREAM,
            move |_, info| {
                let Ok(mut state) = state.lock() else {
                    return gst::PadProbeReturn::Ok;
                };
                match &info.data {
                    // The segment is what turns a timestamp into a running
                    // time, and a flushing seek replaces it - so it has to be
                    // taken from the stream rather than assumed.
                    Some(gst::PadProbeData::Event(event)) => {
                        if let gst::EventView::Segment(segment) = event.view()
                            && let Some(segment) = segment
                                .segment()
                                .downcast_ref::<gst::ClockTime>()
                        {
                            state.segment = Some(segment.clone());
                        }
                    }
                    Some(gst::PadProbeData::Buffer(buffer)) => {
                        let Some(segment) = state.segment.as_ref() else {
                            return gst::PadProbeReturn::Ok;
                        };
                        let (Some(pts), Some(clock), Some(base)) =
                            (buffer.pts(), element.clock(), element.base_time())
                        else {
                            return gst::PadProbeReturn::Ok;
                        };
                        let (Some(buffer_running), Some(now)) =
                            (segment.to_running_time(pts), clock.time())
                        else {
                            return gst::PadProbeReturn::Ok;
                        };
                        let ahead = buffer_running.nseconds() as i64
                            - (now.nseconds() as i64 - base.nseconds() as i64);
                        let ms = ahead / 1_000_000;
                        state.min = state.min.min(ms);
                        state.max = state.max.max(ms);
                        state.seen += 1;
                    }
                    _ => {}
                }
                gst::PadProbeReturn::Ok
            },
        );
    }

    // The sinks' own account of what they did with those buffers. A sink that
    // accepts data at full rate and renders none of it is dropping it, which
    // looks identical from upstream and identical to the recording. This is
    // the one number that tells them apart - and it is still the sink talking
    // about itself, so it settles nothing on its own.
    let sinks: Vec<(String, gst::Element)> = ["primary", "secondary"]
        .iter()
        .filter_map(|role| {
            pipeline
                .by_name(&format!("{role}_out"))
                .map(|sink| (role.to_string(), sink))
        })
        .filter(|(_, sink)| sink.find_property("stats").is_some())
        .collect();

    // Per-second deltas rather than totals. A branch that has stopped reads as
    // a run of zeros, which lines up directly against the seek log and the
    // recorded timeline.
    // Wall clock, so this lines up with the recording and the PulseAudio
    // sampler. Elapsed time from three separate processes cannot be compared,
    // which is how corking briefly looked like the cause.
    let mut last: Vec<(u64, u64, u64)> = vec![(0, 0, 0); counters.len()];
    glib::timeout_add_seconds_local(1, move || {
        let wall = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let mut line = format!("PROBE wall {wall:.1}");
        for (i, (name, count, nonzero, bytes)) in counters.iter().enumerate() {
            let now = (
                count.load(Ordering::Relaxed),
                nonzero.load(Ordering::Relaxed),
                bytes.load(Ordering::Relaxed),
            );
            let buffers = now.0 - last[i].0;
            let live = now.1 - last[i].1;
            let total = now.2 - last[i].2;
            // "--" rather than 0% when nothing arrived at all: no buffers and
            // buffers full of zeroes are different faults.
            let share = if total == 0 {
                "  --".to_string()
            } else {
                format!("{:3.0}%", 100.0 * live as f64 / total as f64)
            };
            line.push_str(&format!(" {name}={buffers:3}/{share}"));
            last[i] = now;
        }
        for (role, state) in &timings {
            let Ok(mut state) = state.lock() else { continue };
            if state.seen == 0 {
                line.push_str(&format!(" {role}[ahead=--]"));
            } else {
                line.push_str(&format!(
                    " {role}[ahead={}..{}ms]",
                    state.min, state.max
                ));
            }
            state.min = i64::MAX;
            state.max = i64::MIN;
            state.seen = 0;
        }
        for (role, sink) in &sinks {
            let stats = sink.property::<gst::Structure>("stats");
            let rendered = stats.get::<u64>("rendered").unwrap_or(0);
            let dropped = stats.get::<u64>("dropped").unwrap_or(0);
            line.push_str(&format!(" {role}[rendered={rendered} dropped={dropped}]"));
        }
        eprintln!("{line}");
        glib::ControlFlow::Continue
    });
}
