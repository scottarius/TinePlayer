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

    let mut counters: Vec<(String, Arc<AtomicU64>)> = Vec::new();

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
            let name = format!("{role}/{label}");
            counters.push((name.clone(), count.clone()));

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
                        Some(gst::PadProbeData::Buffer(_)) => {
                            count.fetch_add(1, Ordering::Relaxed);
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

    // Per-second deltas rather than totals. A branch that has stopped reads as
    // a run of zeros, which lines up directly against the seek log and the
    // recorded timeline.
    let started = std::time::Instant::now();
    let mut last: Vec<u64> = vec![0; counters.len()];
    glib::timeout_add_seconds_local(1, move || {
        let mut line = format!("PROBE t={:5.1}", started.elapsed().as_secs_f64());
        for (i, (name, count)) in counters.iter().enumerate() {
            let now = count.load(Ordering::Relaxed);
            line.push_str(&format!(" {name}={:3}", now - last[i]));
            last[i] = now;
        }
        eprintln!("{line}");
        glib::ControlFlow::Continue
    });
}
