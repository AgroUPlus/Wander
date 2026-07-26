//! Turns the audio tap into visualiser bars.
//!
//! Reads mono samples the audio callback published, runs an FFT over a Hann
//! window, and folds the result into log-spaced bands with peak decay so bars
//! rise instantly and fall smoothly instead of flickering.
//!
//! Levels are measured *relative to the track's own recent loudness* rather than
//! to full scale. A fixed window pinned every modern master to the top of the
//! display, where the bars stop moving and the visualiser stops meaning
//! anything; an adapting reference keeps quiet and loud material equally
//! readable.

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// FFT size. 2048 at 48 kHz is ~43 ms — enough resolution for bass without
/// making the display feel laggy.
const FFT_SIZE: usize = 2048;
/// How much of the previous frame's height is retained. Higher falls slower.
const DECAY: f32 = 0.82;
/// Lowest and highest frequencies shown.
const MIN_HZ: f32 = 40.0;
const MAX_HZ: f32 = 16_000.0;

/// How many dB below the tracked reference reaches the bottom of the display.
const RANGE: f32 = 45.0;
/// How fast the reference follows the music, per update (~50 ms). Rising fast
/// stops a sudden loud passage from clipping off the top; falling slowly stops
/// the display pumping between beats.
const REF_ATTACK: f32 = 0.25;
const REF_RELEASE: f32 = 0.02;
/// The reference is clamped so silence cannot wind the gain up until noise
/// fills the display, and so a full-scale master cannot push it past 0 dBFS.
const REF_FLOOR: f32 = -45.0;
const REF_CEIL: f32 = 0.0;
/// Treble carries far less energy than bass at equal perceived loudness, so
/// without a tilt the top half of the spectrum is permanently flat.
const TILT_DB_PER_OCTAVE: f32 = 3.0;
/// How many past frames the waterfall can scroll through. Comfortably more rows
/// than any terminal is tall.
const HISTORY: usize = 96;

pub struct Spectrum {
    tap: rtrb::Consumer<f32>,
    fft: Arc<dyn Fft<f32>>,
    /// Hann window, precomputed once.
    window: Vec<f32>,
    /// Rolling sample history, always `FFT_SIZE` long, written as a ring.
    history: Vec<f32>,
    /// Where the next sample goes; also the oldest sample's index.
    write: usize,
    scratch: Vec<Complex32>,
    /// Smoothed bar heights in `[0, 1]`.
    bars: Vec<f32>,
    /// Past frames, newest first, for the waterfall mode. Kept here rather than
    /// in the renderer so it keeps filling while the pane is hidden or another
    /// mode is on screen.
    frames: std::collections::VecDeque<Vec<f32>>,
    /// Adapting loudness reference, in dB.
    reference: f32,
    sample_rate: f32,
}

impl Spectrum {
    pub fn new(tap: rtrb::Consumer<f32>, sample_rate: u32, bars: usize) -> Self {
        let fft = FftPlanner::new().plan_fft_forward(FFT_SIZE);
        let window = (0..FFT_SIZE)
            .map(|i| {
                let t = i as f32 / (FFT_SIZE - 1) as f32;
                0.5 - 0.5 * (std::f32::consts::TAU * t).cos()
            })
            .collect();

        Self {
            tap,
            fft,
            window,
            history: vec![0.0; FFT_SIZE],
            write: 0,
            scratch: vec![Complex32::new(0.0, 0.0); FFT_SIZE],
            bars: vec![0.0; bars.max(1)],
            frames: std::collections::VecDeque::with_capacity(HISTORY),
            reference: REF_FLOOR,
            sample_rate: sample_rate as f32,
        }
    }

    /// Change how many bands are displayed, e.g. after a resize or a change of
    /// visualiser mode.
    ///
    /// The levels are resampled rather than zeroed: starting from silence made
    /// the display blink out for a frame every time the pane moved a column,
    /// which during a pane tween is every frame.
    pub fn resize(&mut self, bars: usize) {
        let bars = bars.max(1);
        if bars == self.bars.len() {
            return;
        }

        let previous = std::mem::replace(&mut self.bars, vec![0.0; bars]);
        if !previous.is_empty() {
            for (index, slot) in self.bars.iter_mut().enumerate() {
                // Nearest neighbour is plenty: the next update overwrites this
                // within one frame, it only has to not be a hole.
                let source = index * previous.len() / bars;
                *slot = previous[source.min(previous.len() - 1)];
            }
        }
        // Past frames were a different number of bands wide.
        self.frames.clear();
    }

    /// Drain the tap and recompute the bars. Returns false when no new audio
    /// arrived, so the caller can decay toward silence.
    pub fn update(&mut self) -> bool {
        let mut received = 0;
        while let Ok(sample) = self.tap.pop() {
            // A plain ring write. Rotating the whole history per sample, as
            // this once did, was ~2000 moves for every sample of ~2400 per
            // frame — all of it on the render path.
            self.history[self.write] = sample;
            self.write = (self.write + 1) % FFT_SIZE;
            received += 1;
        }

        if received == 0 {
            // Nothing playing: let the bars fall to zero rather than freeze,
            // and let the reference sink back so the next quiet track is not
            // measured against the last loud one.
            for bar in &mut self.bars {
                *bar *= DECAY;
            }
            self.reference += (REF_FLOOR - self.reference) * REF_RELEASE;
            self.record();
            return false;
        }

        // Oldest sample first: the window has to line up with real time, which
        // the ring's raw order does not.
        let ordered = ring_order(&self.history, self.write);
        for (slot, (sample, window)) in self.scratch.iter_mut().zip(ordered.zip(self.window.iter()))
        {
            *slot = Complex32::new(sample * window, 0.0);
        }
        self.fft.process(&mut self.scratch);

        let bins = FFT_SIZE / 2;
        let bin_hz = self.sample_rate / FFT_SIZE as f32;
        let count = self.bars.len();
        // Hann's coherent gain is 0.5, so a full-scale sine lands at ~1.0 here
        // whatever the FFT size — un-normalised magnitudes made the dB figures
        // (and therefore the whole scale) depend on FFT_SIZE and bin count.
        let scale = 2.0 / FFT_SIZE as f32;

        // First pass: band energies in dB, tilted.
        let mut levels = Vec::with_capacity(count);
        let mut loudest = f32::NEG_INFINITY;
        for index in 0..count {
            // Log-spaced bands: musically even, unlike linear FFT bins which
            // would crowd everything audible into the first few bars.
            let lo = band_edge(index, count);
            let hi = band_edge(index + 1, count);
            let lo_bin = ((lo / bin_hz) as usize).clamp(1, bins - 1);
            let hi_bin = ((hi / bin_hz) as usize).clamp(lo_bin + 1, bins);

            // RMS rather than the loudest bin: a band's height should reflect
            // how much is in it, not whether one bin happened to spike.
            let band = &self.scratch[lo_bin..hi_bin];
            let mean_sq =
                band.iter().map(|c| (c.norm() * scale).powi(2)).sum::<f32>() / band.len() as f32;
            let db = 10.0 * mean_sq.max(1e-12).log10() + tilt_db(lo);
            levels.push(db);
            loudest = loudest.max(db);
        }

        // Follow the music's own level, fast up and slow down.
        let rate = if loudest > self.reference {
            REF_ATTACK
        } else {
            REF_RELEASE
        };
        self.reference += (loudest - self.reference) * rate;
        self.reference = self.reference.clamp(REF_FLOOR, REF_CEIL);

        let floor = self.reference - RANGE;
        for (index, db) in levels.into_iter().enumerate() {
            let level = ((db - floor) / RANGE).clamp(0.0, 1.0);
            let decayed = self.bars[index] * DECAY;
            self.bars[index] = level.max(decayed);
        }

        self.record();
        true
    }

    /// Append this frame to the scrolling history, dropping the oldest.
    fn record(&mut self) {
        if self.frames.len() == HISTORY {
            // Reuse the retired row's allocation rather than freeing one and
            // asking for another on every single frame.
            let mut oldest = self.frames.pop_back().unwrap_or_default();
            oldest.clear();
            oldest.extend_from_slice(&self.bars);
            self.frames.push_front(oldest);
        } else {
            self.frames.push_front(self.bars.clone());
        }
    }

    /// Past frames, newest first.
    pub fn history(&self) -> impl Iterator<Item = &[f32]> {
        self.frames.iter().map(|frame| frame.as_slice())
    }

    pub fn bars(&self) -> &[f32] {
        &self.bars
    }

    /// The most recent `count` samples, oldest first — the raw waveform, for
    /// the oscilloscope mode. Reuses the FFT's own history, so no second tap.
    pub fn waveform(&self, count: usize) -> impl Iterator<Item = f32> + '_ {
        let count = count.min(FFT_SIZE);
        ring_order(&self.history, self.write)
            .skip(FFT_SIZE - count)
            .copied()
    }
}

/// A ring buffer's contents in time order, oldest entry first.
fn ring_order(history: &[f32], write: usize) -> impl Iterator<Item = &f32> {
    history[write..].iter().chain(history[..write].iter())
}

/// Treble boost relative to the lowest band, in dB.
fn tilt_db(hz: f32) -> f32 {
    TILT_DB_PER_OCTAVE * (hz / MIN_HZ).max(1.0).log2()
}

/// Frequency at a band boundary, spaced logarithmically from MIN_HZ to MAX_HZ.
fn band_edge(index: usize, count: usize) -> f32 {
    let t = index as f32 / count as f32;
    MIN_HZ * (MAX_HZ / MIN_HZ).powf(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spectrum_with(samples: &[f32], bars: usize) -> Spectrum {
        let (mut producer, consumer) = rtrb::RingBuffer::<f32>::new(FFT_SIZE * 4);
        for &s in samples {
            let _ = producer.push(s);
        }
        Spectrum::new(consumer, 48_000, bars)
    }

    fn tone(hz: f32, count: usize) -> Vec<f32> {
        tone_at(hz, count, 1.0)
    }

    fn tone_at(hz: f32, count: usize, amplitude: f32) -> Vec<f32> {
        (0..count)
            .map(|i| amplitude * (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin())
            .collect()
    }

    /// Feed the same tone repeatedly so the loudness reference can settle, and
    /// report the tallest bar.
    fn settled_peak(amplitude: f32) -> f32 {
        let samples = tone_at(1000.0, FFT_SIZE, amplitude);
        let (mut producer, consumer) = rtrb::RingBuffer::<f32>::new(FFT_SIZE * 2);
        let mut spectrum = Spectrum::new(consumer, 48_000, 24);
        for _ in 0..40 {
            for &s in &samples {
                let _ = producer.push(s);
            }
            spectrum.update();
        }
        spectrum.bars().iter().copied().fold(0.0f32, f32::max)
    }

    #[test]
    fn bands_are_log_spaced_and_ascending() {
        let edges: Vec<f32> = (0..=16).map(|i| band_edge(i, 16)).collect();
        assert!(edges.windows(2).all(|w| w[1] > w[0]), "edges must ascend");
        assert!((edges[0] - MIN_HZ).abs() < 0.001);
        assert!((edges[16] - MAX_HZ).abs() < 1.0);
    }

    #[test]
    fn a_low_tone_lights_up_a_low_band() {
        let mut spectrum = spectrum_with(&tone(100.0, FFT_SIZE * 2), 16);
        assert!(spectrum.update(), "should consume samples");

        let bars = spectrum.bars();
        let loudest = bars
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        assert!(
            loudest < 5,
            "100 Hz should peak in a low band, got {loudest}"
        );
    }

    #[test]
    fn a_high_tone_lights_up_a_high_band() {
        let mut spectrum = spectrum_with(&tone(8000.0, FFT_SIZE * 2), 16);
        spectrum.update();

        let bars = spectrum.bars();
        let loudest = bars
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        assert!(
            loudest > 10,
            "8 kHz should peak in a high band, got {loudest}"
        );
    }

    #[test]
    fn bars_stay_within_unit_range() {
        // Full-scale input must not produce out-of-range bar heights.
        let mut spectrum = spectrum_with(&tone(1000.0, FFT_SIZE * 2), 24);
        spectrum.update();
        assert!(
            spectrum.bars().iter().all(|b| (0.0..=1.0).contains(b)),
            "bars must be normalised"
        );
    }

    #[test]
    fn silence_decays_the_bars_toward_zero() {
        let mut spectrum = spectrum_with(&tone(1000.0, FFT_SIZE * 2), 8);
        spectrum.update();
        let before: f32 = spectrum.bars().iter().sum();
        assert!(before > 0.0);

        // No new audio: bars should fall, not freeze.
        for _ in 0..20 {
            assert!(!spectrum.update(), "no samples left to consume");
        }
        let after: f32 = spectrum.bars().iter().sum();
        assert!(
            after < before * 0.5,
            "bars should decay ({before} -> {after})"
        );
    }

    #[test]
    fn quiet_and_loud_material_fill_a_similar_amount_of_the_display() {
        // The whole point of the adapting reference: a track mastered 20 dB
        // quieter must not draw 20 dB shorter bars.
        let loud = settled_peak(1.0);
        let quiet = settled_peak(0.1);
        assert!(loud > 0.5, "a loud tone should reach the display: {loud}");
        assert!(
            (loud - quiet).abs() < 0.15,
            "levels should be comparable after the reference settles: {loud} vs {quiet}"
        );
    }

    #[test]
    fn a_loud_tone_does_not_peg_every_bar_to_the_top() {
        // The old fixed window pushed everything to full height, at which point
        // the bars stop conveying anything.
        let samples = tone_at(1000.0, FFT_SIZE * 2, 1.0);
        let mut spectrum = spectrum_with(&samples, 24);
        spectrum.update();
        let pegged = spectrum.bars().iter().filter(|b| **b >= 0.999).count();
        assert!(pegged <= 2, "{pegged} of 24 bars saturated");
    }

    #[test]
    fn the_reference_sinks_back_during_silence() {
        let mut spectrum = spectrum_with(&tone(1000.0, FFT_SIZE * 2), 8);
        spectrum.update();
        let after_audio = spectrum.reference;
        for _ in 0..200 {
            spectrum.update();
        }
        assert!(
            spectrum.reference < after_audio,
            "reference should release toward the floor ({after_audio} -> {})",
            spectrum.reference
        );
        assert!(spectrum.reference >= REF_FLOOR, "and never below the floor");
    }

    #[test]
    fn the_tilt_lifts_treble_and_leaves_the_lowest_band_alone() {
        assert_eq!(tilt_db(MIN_HZ), 0.0);
        assert!((tilt_db(MIN_HZ * 2.0) - TILT_DB_PER_OCTAVE).abs() < 1e-4);
        assert!((tilt_db(MIN_HZ * 4.0) - 2.0 * TILT_DB_PER_OCTAVE).abs() < 1e-4);
        // Below the display's range there is nothing to correct.
        assert_eq!(tilt_db(MIN_HZ / 4.0), 0.0);
    }

    #[test]
    fn the_waveform_reports_the_most_recent_samples_in_order() {
        let ramp: Vec<f32> = (0..FFT_SIZE + 100).map(|i| i as f32).collect();
        let mut spectrum = spectrum_with(&ramp, 8);
        spectrum.update();

        let tail: Vec<f32> = spectrum.waveform(4).collect();
        let expected: Vec<f32> = ramp[ramp.len() - 4..].to_vec();
        assert_eq!(tail, expected, "must be the newest samples, oldest first");
        assert_eq!(spectrum.waveform(FFT_SIZE * 4).count(), FFT_SIZE);
    }

    /// A pane that moves a column at a time must not make the display blink.
    #[test]
    fn resize_carries_the_levels_across_rather_than_blanking_them() {
        let mut spectrum = spectrum_with(&tone(1000.0, FFT_SIZE * 2), 16);
        spectrum.update();
        let before: f32 = spectrum.bars().iter().sum();
        assert!(before > 0.0, "needs something to carry across");

        for count in [17, 40, 12, 13] {
            spectrum.resize(count);
            assert_eq!(spectrum.bars().len(), count);
            assert!(
                spectrum.bars().iter().sum::<f32>() > 0.0,
                "resizing to {count} blanked the display"
            );
        }
    }

    #[test]
    fn resize_changes_band_count_without_panicking() {
        let mut spectrum = spectrum_with(&tone(440.0, FFT_SIZE), 8);
        spectrum.update();
        spectrum.resize(32);
        assert_eq!(spectrum.bars().len(), 32);
        spectrum.update();
        spectrum.resize(1);
        assert_eq!(spectrum.bars().len(), 1);
        spectrum.update();
    }
}
