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

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gst::prelude::*;
use gstreamer as gst;

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
