//! Turns the audio tap into visualiser bars.
//!
//! Reads mono samples the audio callback published, runs an FFT over a Hann
//! window, and folds the result into log-spaced bands with peak decay so bars
//! rise instantly and fall smoothly instead of flickering.

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

pub struct Spectrum {
    tap: rtrb::Consumer<f32>,
    fft: Arc<dyn Fft<f32>>,
    /// Hann window, precomputed once.
    window: Vec<f32>,
    /// Rolling sample history, always `FFT_SIZE` long.
    history: Vec<f32>,
    scratch: Vec<Complex32>,
    /// Smoothed bar heights in `[0, 1]`.
    bars: Vec<f32>,
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
            scratch: vec![Complex32::new(0.0, 0.0); FFT_SIZE],
            bars: vec![0.0; bars.max(1)],
            sample_rate: sample_rate as f32,
        }
    }

    /// Change how many bands are displayed, e.g. after a resize.
    pub fn resize(&mut self, bars: usize) {
        let bars = bars.max(1);
        if bars != self.bars.len() {
            self.bars = vec![0.0; bars];
        }
    }

    /// Drain the tap and recompute the bars. Returns false when no new audio
    /// arrived, so the caller can decay toward silence.
    pub fn update(&mut self) -> bool {
        let mut received = 0;
        while let Ok(sample) = self.tap.pop() {
            // Keep the most recent FFT_SIZE samples.
            self.history.rotate_left(1);
            self.history[FFT_SIZE - 1] = sample;
            received += 1;
        }

        if received == 0 {
            // Nothing playing: let the bars fall to zero rather than freeze.
            for bar in &mut self.bars {
                *bar *= DECAY;
            }
            return false;
        }

        for (slot, (sample, window)) in self
            .scratch
            .iter_mut()
            .zip(self.history.iter().zip(self.window.iter()))
        {
            *slot = Complex32::new(sample * window, 0.0);
        }
        self.fft.process(&mut self.scratch);

        let bins = FFT_SIZE / 2;
        let bin_hz = self.sample_rate / FFT_SIZE as f32;
        let count = self.bars.len();

        for index in 0..count {
            // Log-spaced bands: musically even, unlike linear FFT bins which
            // would crowd everything audible into the first few bars.
            let lo = band_edge(index, count);
            let hi = band_edge(index + 1, count);
            let lo_bin = ((lo / bin_hz) as usize).clamp(1, bins - 1);
            let hi_bin = ((hi / bin_hz) as usize).clamp(lo_bin + 1, bins);

            let peak = self.scratch[lo_bin..hi_bin]
                .iter()
                .map(|c| c.norm())
                .fold(0.0f32, f32::max);

            // Compress to dB so quiet detail stays visible.
            let db = 20.0 * (peak.max(1e-6)).log10();
            let level = ((db + 60.0) / 60.0).clamp(0.0, 1.0);

            let decayed = self.bars[index] * DECAY;
            self.bars[index] = level.max(decayed);
        }

        true
    }

    pub fn bars(&self) -> &[f32] {
        &self.bars
    }
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
        (0..count)
            .map(|i| (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin())
            .collect()
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
