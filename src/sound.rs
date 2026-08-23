//! A short navigation click, synthesized rather than shipped as an asset
//! so there's no binary file to package or license.

use gst::prelude::*;
use gstreamer as gst;

const SAMPLE_RATE: u32 = 48_000;
const DURATION_MS: u32 = 45;
/// Low and soft: this fires on every navigation step, so it has to sit
/// under the content rather than announce itself.
const FREQUENCY: f64 = 480.0;
const AMPLITUDE: f64 = 0.05;
/// Long enough that the waveform eases in rather than snapping on. A
/// near-instant attack is what makes a tone read as harsh.
const ATTACK_MS: f64 = 6.0;
const DECAY: f64 = 55.0;

/// A gently enveloped sine burst wrapped in a WAV container. Both ends are
/// shaped deliberately: an abrupt start produces a pop on top of the tone,
/// and an abrupt end produces another when the waveform is cut mid-cycle.
fn click_wav() -> Vec<u8> {
    let total = SAMPLE_RATE * DURATION_MS / 1000;
    let attack = (SAMPLE_RATE as f64 * ATTACK_MS / 1000.0).max(1.0);

    let mut pcm: Vec<u8> = Vec::with_capacity(total as usize * 2);
    for n in 0..total {
        let t = n as f64 / SAMPLE_RATE as f64;
        let progress = n as f64 / total as f64;

        // Raised cosine in, exponential decay, then a final taper to
        // guarantee the last sample lands on silence.
        let attack_gain = if (n as f64) < attack {
            0.5 - 0.5 * (std::f64::consts::PI * n as f64 / attack).cos()
        } else {
            1.0
        };
        let envelope = attack_gain * (-t * DECAY).exp() * (1.0 - progress).powf(0.5);

        let value = (t * FREQUENCY * std::f64::consts::TAU).sin() * envelope * AMPLITUDE;
        pcm.extend_from_slice(&((value * i16::MAX as f64) as i16).to_le_bytes());
    }

    let data_len = pcm.len() as u32;
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // PCM header size
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // byte rate
    wav.extend_from_slice(&2u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(&pcm);
    wav
}

/// Holds one pre-rolled pipeline for the lifetime of the application.
///
/// The obvious implementation - build a pipeline per click - is far too
/// slow to sit in a key handler: resolving the output device alone starts
/// and stops a GStreamer device monitor, and doing that per keystroke added
/// something close to a second of input lag. Everything expensive happens
/// once, here, so playing is only a seek and a state change.
pub struct Sounds {
    pipeline: Option<gst::Pipeline>,
    _watch: Option<gst::bus::BusWatchGuard>,
}

impl Sounds {
    pub fn new(enabled: bool, device: Option<String>) -> Self {
        if !enabled {
            return Self {
                pipeline: None,
                _watch: None,
            };
        }
        match Self::build(device) {
            Ok((pipeline, watch)) => Self {
                pipeline: Some(pipeline),
                _watch: Some(watch),
            },
            Err(e) => {
                crate::log!("Navigation sounds unavailable: {e}");
                Self {
                    pipeline: None,
                    _watch: None,
                }
            }
        }
    }

    fn build(device: Option<String>) -> Result<(gst::Pipeline, gst::bus::BusWatchGuard), String> {
        let path = std::env::temp_dir().join("tineplayer-click.wav");
        std::fs::write(&path, click_wav()).map_err(|e| e.to_string())?;

        let pipeline = gst::parse::launch(
            "filesrc name=src ! wavparse ! audioconvert ! audioresample name=out",
        )
        .map_err(|e| e.to_string())?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "not a pipeline".to_string())?;

        pipeline
            .by_name("src")
            .ok_or("missing src")?
            .set_property("location", path.to_string_lossy().to_string());

        // Prefer the output the user is actually listening to; fall back to
        // whatever the system picks if that device has gone away.
        let sink = device
            .as_deref()
            .and_then(|name| crate::devices::find_audio_output_device(name).ok())
            .and_then(|device| device.create_element(None).ok())
            .map(Ok)
            .unwrap_or_else(|| {
                gst::ElementFactory::make("autoaudiosink")
                    .build()
                    .map_err(|e| e.to_string())
            })?;

        pipeline.add(&sink).map_err(|e| e.to_string())?;
        pipeline
            .by_name("out")
            .ok_or("missing out")?
            .link(&sink)
            .map_err(|e| e.to_string())?;

        // Returning to PAUSED after each play leaves the pipeline prerolled
        // and ready, so the next click costs only a seek.
        let bus = pipeline.bus().ok_or("no bus")?;
        let weak = pipeline.downgrade();
        let watch = bus
            .add_watch_local(move |_, msg| {
                use gst::MessageView;
                match msg.view() {
                    gst::MessageView::Eos(_) => {
                        if let Some(pipeline) = weak.upgrade() {
                            let _ = pipeline.set_state(gst::State::Paused);
                            let _ =
                                pipeline.seek_simple(gst::SeekFlags::FLUSH, gst::ClockTime::ZERO);
                        }
                    }
                    MessageView::Error(err) => {
                        crate::log!("Click sound error: {}", err.error());
                    }
                    _ => {}
                }
                glib::ControlFlow::Continue
            })
            .map_err(|e| e.to_string())?;

        pipeline
            .set_state(gst::State::Paused)
            .map_err(|e| e.to_string())?;
        Ok((pipeline, watch))
    }

    pub fn click(&self) {
        let Some(pipeline) = self.pipeline.as_ref() else {
            return;
        };
        // Flushing seek so a click that arrives mid-sound restarts it
        // rather than being dropped.
        let _ = pipeline.seek_simple(gst::SeekFlags::FLUSH, gst::ClockTime::ZERO);
        let _ = pipeline.set_state(gst::State::Playing);
    }
}
