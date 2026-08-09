//! Working out how far a separate audio file runs ahead of or behind the
//! video it belongs to.
//!
//! A separately sourced soundtrack - a described version, a dub, a restored
//! track - is routinely out by seconds, because it was cut against a different
//! master or carries a different amount of leader. Asking someone to find that
//! offset by ear with a slider is asking a lot, and asking it of the person who
//! cannot see the picture is asking far too much.
//!
//! **The envelope, not the waveform.** Two recordings of the same film in
//! different languages share almost no samples: different voices, different
//! takes, different mixes. What they do share is a shape - when it is loud,
//! when it is quiet, where the explosions and the silences fall. Correlating a
//! short-term loudness envelope finds that shape, and a foreign dub lines up
//! against the original as readily as a description track does.
//!
//! **Written from the method, not from anyone's code.** Cross-correlation for
//! a time offset is ordinary signal processing. `describealign` solves the same
//! problem and is GPL-3.0, so none of it is read or transliterated here:
//! TinePlayer is MIT and the packaging keeps it that way deliberately.

// Nothing calls this yet: the measuring is finished and tested, and applying
// the answer is the next piece. It wants a baseline held per output, combined
// with the viewer's own sync setting at the three places that set an offset -
// see the plan. Kept whole rather than wired in halfway, since a half-applied
// offset is worse than none.
#![allow(dead_code)]

use gstreamer as gst;
use gstreamer::prelude::*;
use rustfft::{FftPlanner, num_complex::Complex};

/// How long each envelope frame covers, in milliseconds.
///
/// 10ms is fine enough to place an offset well within a frame of video and
/// coarse enough that a minute of audio is six thousand numbers rather than
/// millions. It also sets the resolution of the answer: an offset is only ever
/// known to the nearest frame.
pub const FRAME_MS: u32 = 10;

/// What the envelopes are built from. Speech and music carry their shape in
/// the low frequencies, and a loudness contour needs no more than this - so
/// the cheapest rate that keeps it is the right one, and decoding is where all
/// the time goes.
const RATE: u32 = 8_000;

/// Decodes one stretch of a source down to mono samples.
///
/// Any container, any codec, local or remote: `uridecodebin` and the converters
/// after it make that somebody else's problem, and the caps make the result
/// exactly one shape so the arithmetic afterwards has no cases in it.
///
/// Returns fewer samples than asked for at the end of a file, and an error only
/// when the file cannot be read at all.
pub fn decode_window(uri: &str, start_s: f64, length_s: f64) -> Result<Vec<f32>, String> {
    use gstreamer_app::AppSink;

    let description = format!(
        "uridecodebin uri=\"{uri}\" ! audioconvert ! audioresample ! \
         audio/x-raw,format=F32LE,channels=1,rate={RATE},layout=interleaved ! \
         appsink name=out sync=false max-buffers=64"
    );
    let pipeline = gst::parse::launch(&description)
        .map_err(|e| format!("Could not build the analysis pipeline: {e}"))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "Analysis pipeline was not a pipeline".to_string())?;

    let sink = pipeline
        .by_name("out")
        .and_then(|element| element.downcast::<AppSink>().ok())
        .ok_or("Analysis pipeline has no appsink")?;

    // Paused first, so the file is open and seekable before asking for a
    // position in it. Seeking a pipeline that has not prerolled is ignored,
    // and the window then quietly comes from the beginning of the file - which
    // looks like an alignment that simply got the wrong answer.
    pipeline
        .set_state(gst::State::Paused)
        .map_err(|_| "Could not open the audio for analysis".to_string())?;
    let (result, _, _) = pipeline.state(gst::ClockTime::from_seconds(10));
    result.map_err(|_| "Timed out opening the audio for analysis".to_string())?;

    if start_s > 0.0 {
        let start = gst::ClockTime::from_nseconds((start_s * 1e9) as u64);
        // Accurate rather than fast: a keyframe-snapped seek can land a second
        // or two away, which is the same size as the answer being measured.
        let _ = pipeline.seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE, start);
    }
    pipeline
        .set_state(gst::State::Playing)
        .map_err(|_| "Could not read the audio for analysis".to_string())?;

    let wanted = (length_s * RATE as f64) as usize;
    let mut samples: Vec<f32> = Vec::with_capacity(wanted);
    while samples.len() < wanted {
        let Ok(sample) = sink.pull_sample() else {
            break; // End of file, or the sink was shut down.
        };
        let Some(buffer) = sample.buffer() else {
            continue;
        };
        let Ok(map) = buffer.map_readable() else {
            continue;
        };
        samples.extend(
            map.as_slice()
                .chunks_exact(4)
                .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
        );
    }

    let _ = pipeline.set_state(gst::State::Null);
    if samples.is_empty() {
        return Err("No audio to analyse".to_string());
    }
    samples.truncate(wanted);
    Ok(samples)
}

/// The loudness shape of a stretch of audio: one root-mean-square value per
/// [`FRAME_MS`] of samples.
///
/// Mean-removed, because what matters is where this is louder or quieter than
/// its own average. Without that, two recordings at different levels correlate
/// on their levels rather than on their shapes, and a quiet passage in both
/// counts as agreement.
pub fn envelope(samples: &[f32], rate: u32) -> Vec<f32> {
    if rate == 0 {
        return Vec::new();
    }
    let per_frame = (rate as usize * FRAME_MS as usize / 1000).max(1);
    let mut frames: Vec<f32> = samples
        .chunks(per_frame)
        .map(|chunk| {
            let sum: f32 = chunk.iter().map(|s| s * s).sum();
            (sum / chunk.len() as f32).sqrt()
        })
        .collect();

    let mean = frames.iter().sum::<f32>() / frames.len().max(1) as f32;
    for frame in &mut frames {
        *frame -= mean;
    }
    frames
}

/// What a single correlation concluded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Match {
    /// How far the second envelope runs behind the first, in frames.
    ///
    /// Positive means the audio file is *late*: its content happens later than
    /// the video's, so it has to be pulled forward to line up.
    pub lag_frames: i64,
    /// How much the winning lag stands out from the rest, from 0 to roughly 1.
    ///
    /// The peak divided by the correlation's own spread. A real alignment is a
    /// spike; two unrelated recordings produce a low ridge with no clear
    /// winner, and that is exactly the case that must not be reported as an
    /// answer.
    pub score: f32,
}

impl Match {
    pub fn millis(&self) -> f64 {
        self.lag_frames as f64 * FRAME_MS as f64
    }
}

/// Finds the lag that best lines `b` up against `a`.
///
/// Every lag is examined, not a window of likely ones: an FFT correlation
/// costs the same whether the answer turns out to be 40 milliseconds or 40
/// seconds, so there is nothing to gain by guessing a range and something real
/// to lose - the offsets that matter most are the large ones.
pub fn best_lag(a: &[f32], b: &[f32]) -> Option<Match> {
    if a.len() < 2 || b.len() < 2 {
        return None;
    }
    let n = (a.len() + b.len()).next_power_of_two();
    let mut planner = FftPlanner::new();

    let mut fa = padded(a, n);
    let mut fb = padded(b, n);
    planner.plan_fft_forward(n).process(&mut fa);
    planner.plan_fft_forward(n).process(&mut fb);

    // Multiplying by the conjugate is correlation rather than convolution:
    // the same operation with one of the two reversed in time.
    let mut product: Vec<Complex<f32>> = fa
        .iter()
        .zip(fb.iter())
        .map(|(x, y)| x * y.conj())
        .collect();
    planner.plan_fft_inverse(n).process(&mut product);

    let correlation: Vec<f32> = product.iter().map(|c| c.re).collect();
    let (index, peak) = correlation
        .iter()
        .enumerate()
        .max_by(|(_, x), (_, y)| x.total_cmp(y))
        .map(|(i, v)| (i, *v))?;
    if peak <= 0.0 {
        return None;
    }

    // The normalized correlation coefficient: the peak divided by what a
    // perfect match between these two would have scored. That makes it a
    // number between 0 and 1 which means the same thing for a loud film and a
    // quiet one, and which can be compared against a threshold written down
    // once. An ad-hoc peak-to-average ratio was tried first and could not
    // separate a real one-frame shift from two unrelated recordings.
    //
    // Divided by `n` as well, because an inverse FFT here is unnormalized:
    // what comes back is the correlation multiplied by the transform length.
    let energy = |values: &[f32]| values.iter().map(|v| v * v).sum::<f32>();
    let perfect = (energy(a) * energy(b)).sqrt();
    let score = if perfect > 0.0 {
        ((peak / n as f32) / perfect).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // The second half of a circular correlation is the negative lags.
    let raw = if index <= n / 2 {
        index as i64
    } else {
        index as i64 - n as i64
    };
    // Negated so the sign means something to a reader: correlating a against b
    // peaks where a lines up with b shifted *back*, which is the opposite of
    // how anyone describes the answer. Positive here means the audio file runs
    // late and has to be pulled forward.
    Some(Match {
        lag_frames: -raw,
        score,
    })
}

/// How long each of the three sampled stretches is.
///
/// A minute is long enough to hold several distinctive loud and quiet passages
/// even in a talky scene, and short enough that three of them decode in
/// seconds rather than minutes. Decoding two whole films to align them would
/// take about a minute of work to answer a question a few seconds of it can.
const WINDOW_S: f64 = 60.0;

/// Below this, the correlation is not an answer. Two unrelated recordings
/// always produce a highest point somewhere, and reporting it would be worse
/// than reporting nothing: silent misalignment is the failure that matters,
/// because the person who cannot see the picture has no way to tell narration
/// is drifting until it is badly wrong.
const CONFIDENT: f32 = 0.35;

/// How far the three windows may disagree and still count as one answer.
/// Beyond a fifth of a second the two are not simply shifted.
const AGREEMENT_MS: f64 = 200.0;

/// What aligning a pairing concluded.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// One shift lines the whole film up. This is the answer to apply.
    Offset { millis: f64, confidence: f32 },
    /// The windows disagree in a straight line: the two run at different
    /// rates, as a PAL speedup does. No single offset fixes that, so none is
    /// offered - the drift is reported instead of averaged away.
    RateMismatch { drift_ms_per_hour: f64 },
    /// Nothing worth believing. Different edits, an unrelated file, or audio
    /// too quiet or too uniform to match on.
    Unsure,
}

/// Works out how far an audio file runs behind the video it belongs to.
///
/// Three stretches rather than one, spread across the running time, because a
/// single agreement can be luck and three cannot. What the three say together
/// is also the only way to tell a shift from a stretch: agreement means an
/// offset, a straight-line disagreement means the two run at different rates,
/// and scatter means there is no answer here to give.
///
/// `duration_s` is the video's running time, used only to place the windows.
pub fn align(video_uri: &str, audio_uri: &str, duration_s: f64) -> Verdict {
    // Away from both ends: the first minute is titles and the last is credits,
    // and both are where two releases differ most.
    let usable = (duration_s - WINDOW_S).max(0.0);
    let starts = [0.15, 0.5, 0.8].map(|fraction| usable * fraction);

    let mut matches = Vec::new();
    for start in starts {
        let Ok(video) = decode_window(video_uri, start, WINDOW_S) else {
            continue;
        };
        let Ok(audio) = decode_window(audio_uri, start, WINDOW_S) else {
            continue;
        };
        if let Some(found) = best_lag(&envelope(&video, RATE), &envelope(&audio, RATE)) {
            matches.push((start, found));
        }
    }

    let confident: Vec<(f64, Match)> = matches
        .iter()
        .copied()
        .filter(|(_, found)| found.score >= CONFIDENT)
        .collect();
    if confident.len() < 2 {
        return Verdict::Unsure;
    }

    let spread = confident
        .iter()
        .map(|(_, found)| found.millis())
        .fold(f64::NEG_INFINITY, f64::max)
        - confident
            .iter()
            .map(|(_, found)| found.millis())
            .fold(f64::INFINITY, f64::min);

    if spread <= AGREEMENT_MS {
        let millis = median(
            &confident
                .iter()
                .map(|(_, m)| m.millis())
                .collect::<Vec<_>>(),
        );
        let confidence = confident
            .iter()
            .map(|(_, found)| found.score)
            .fold(f32::INFINITY, f32::min);
        return Verdict::Offset { millis, confidence };
    }

    // Disagreement that grows with position is a rate difference rather than
    // noise. Measured between the furthest-apart windows, which is where it
    // shows most clearly.
    // Two entries at least, checked above.
    let (first_start, first_match) = confident[0];
    let (last_start, last_match) = confident[confident.len() - 1];
    let elapsed_hours = (last_start - first_start) / 3600.0;
    if elapsed_hours > 0.0 {
        return Verdict::RateMismatch {
            drift_ms_per_hour: (last_match.millis() - first_match.millis()) / elapsed_hours,
        };
    }
    Verdict::Unsure
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

fn padded(values: &[f32], n: usize) -> Vec<Complex<f32>> {
    let mut out = vec![Complex::new(0.0, 0.0); n];
    for (slot, value) in out.iter_mut().zip(values) {
        *slot = Complex::new(*value, 0.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shape with enough going on to correlate against: quiet stretches,
    /// loud stretches, and no repeating period that could align with itself.
    fn shape(length: usize) -> Vec<f32> {
        (0..length)
            .map(|i| {
                let i = i as f32;
                (i * 0.37).sin() * (i * 0.011).cos() + (i * 0.0007 * i).sin() * 0.5
            })
            .collect()
    }

    #[test]
    fn finds_a_known_shift() {
        let base = shape(4000);
        for delay in [1i64, 25, 300, -300, -7] {
            let shifted: Vec<f32> = if delay >= 0 {
                std::iter::repeat_n(0.0, delay as usize)
                    .chain(base.iter().copied())
                    .collect()
            } else {
                base[(-delay) as usize..].to_vec()
            };
            let found = best_lag(&base, &shifted).expect("a lag");
            assert_eq!(found.lag_frames, delay, "delay {delay} was misread");
            assert!(
                found.score > 0.3,
                "delay {delay} scored only {}",
                found.score
            );
        }
    }

    /// The case that must never be reported as an answer: two recordings with
    /// nothing to do with each other still produce a highest point somewhere.
    #[test]
    fn unrelated_audio_scores_low() {
        let a = shape(4000);
        let b: Vec<f32> = (0..4000)
            .map(|i| ((i * 7919) % 1000) as f32 / 500.0 - 1.0)
            .collect();
        let found = best_lag(&a, &b).expect("a lag");
        assert!(
            found.score < 0.3,
            "unrelated audio scored {} and would have been believed",
            found.score
        );
    }

    #[test]
    fn envelope_follows_loudness() {
        let rate = 8000;
        // A second of quiet, then a second of noise.
        let mut samples = vec![0.0f32; rate as usize];
        samples.extend((0..rate).map(|i| if i % 2 == 0 { 0.8 } else { -0.8 }));
        let frames = envelope(&samples, rate);
        assert_eq!(frames.len(), 200);
        // Mean-removed, so quiet is below zero and loud above it.
        assert!(frames[10] < 0.0);
        assert!(frames[150] > 0.0);
    }

    #[test]
    fn nothing_to_correlate_is_not_an_answer() {
        assert!(best_lag(&[], &[1.0, 2.0]).is_none());
        assert!(best_lag(&[1.0], &[1.0]).is_none());
        assert!(envelope(&[0.1, 0.2], 0).is_empty());
    }
}
