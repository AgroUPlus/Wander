use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Mono samples buffered for the visualiser: about a quarter second at 48 kHz,
/// which is comfortably more than one FFT window.
const TAP_CAPACITY: usize = 12_288;

/// State shared between the audio callback and the rest of the app.
///
/// Every field is atomic: the callback runs on a realtime thread and must never
/// block, allocate, or take a lock.
#[derive(Debug)]
pub struct AudioShared {
    /// Frames actually handed to the device. This is the playback clock.
    frames_played: AtomicU64,
    /// Frames the callback consumed as silence because the ring ran dry.
    underrun_frames: AtomicU64,
    /// Linear gain in `[0, 1]`, stored as f32 bits.
    volume: AtomicU32,
    paused: AtomicBool,
    /// Set to make the callback discard buffered audio. Used on skip and seek
    /// so already-decoded samples from the old position are never heard.
    flush: AtomicBool,
    sample_rate: u32,
    channels: u16,
}

impl AudioShared {
    fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            frames_played: AtomicU64::new(0),
            underrun_frames: AtomicU64::new(0),
            volume: AtomicU32::new(1.0f32.to_bits()),
            paused: AtomicBool::new(false),
            flush: AtomicBool::new(false),
            sample_rate,
            channels,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }

    pub fn set_volume(&self, volume: f32) {
        self.volume
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    /// Ask the audio callback to discard everything currently buffered.
    pub fn request_flush(&self) {
        self.flush.store(true, Ordering::Release);
    }

    /// Frames played since the clock was last reset.
    pub fn frames_played(&self) -> u64 {
        self.frames_played.load(Ordering::Relaxed)
    }

    /// Elapsed playback time. Derived from frames the device actually consumed,
    /// so it reflects what the user hears rather than how far the decoder has
    /// run ahead.
    pub fn elapsed(&self) -> std::time::Duration {
        let frames = self.frames_played();
        std::time::Duration::from_secs_f64(frames as f64 / self.sample_rate as f64)
    }

    /// Reset the clock, e.g. when starting a new track or seeking.
    pub fn reset_clock(&self, to_frames: u64) {
        self.frames_played.store(to_frames, Ordering::Relaxed);
    }

    pub fn underrun_frames(&self) -> u64 {
        self.underrun_frames.load(Ordering::Relaxed)
    }
}

/// Owns the cpal stream. Dropping this stops audio output.
///
/// `cpal::Stream` is not `Send` on every platform, so this value has to stay on
/// the thread that created it — the caller keeps it alive for the process
/// lifetime rather than moving it into the player task.
pub struct AudioOutput {
    /// Held purely to keep the device open; dropping it silences playback.
    #[allow(dead_code)]
    pub stream: cpal::Stream,
    pub shared: Arc<AudioShared>,
    /// Producer half of the PCM ring; the decoder writes interleaved f32 here.
    pub producer: rtrb::Producer<f32>,
    /// Mono copy of what the device actually played, for the visualiser.
    pub tap: rtrb::Consumer<f32>,
}

impl AudioOutput {
    /// Open the default output device and start a stream fed from a ring buffer
    /// holding `buffer_seconds` of audio.
    pub fn new(buffer_seconds: f32) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .context("no default audio output device")?;
        let supported = device
            .default_output_config()
            .context("querying default output config")?;

        let sample_rate = supported.sample_rate();
        let channels = supported.channels();
        let config: cpal::StreamConfig = supported.config();

        let shared = Arc::new(AudioShared::new(sample_rate, channels));

        let capacity = (sample_rate as f32 * buffer_seconds) as usize * channels as usize;
        let (producer, mut consumer) = rtrb::RingBuffer::<f32>::new(capacity.max(4096));

        // Visualiser tap. Deliberately small: it holds a fraction of a second
        // of audio, so the spectrum reflects what is being heard right now
        // rather than lagging behind by the playback buffer's depth.
        let (mut tap_producer, tap) = rtrb::RingBuffer::<f32>::new(TAP_CAPACITY);

        let callback_shared = Arc::clone(&shared);
        let error_shared = Arc::clone(&shared);

        let stream = device
            .build_output_stream(
                config,
                move |output: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let channels = callback_shared.channels as usize;

                    // A skip or seek happened: drop everything still queued so
                    // the listener never hears audio from the old position.
                    if callback_shared.flush.swap(false, Ordering::AcqRel) {
                        // Discard in one chunk rather than popping sample by
                        // sample, to keep this realtime callback bounded.
                        let queued = consumer.slots();
                        if let Ok(chunk) = consumer.read_chunk(queued) {
                            chunk.commit_all();
                        }
                        output.fill(0.0);
                        return;
                    }

                    if callback_shared.paused.load(Ordering::Relaxed) {
                        output.fill(0.0);
                        return;
                    }

                    let gain = f32::from_bits(callback_shared.volume.load(Ordering::Relaxed));
                    let mut filled = 0;
                    for slot in output.iter_mut() {
                        match consumer.pop() {
                            Ok(sample) => {
                                *slot = sample * gain;
                                filled += 1;
                            }
                            // Ring is dry: emit silence rather than stuttering.
                            Err(_) => *slot = 0.0,
                        }
                    }

                    let frames = filled / channels;
                    callback_shared
                        .frames_played
                        .fetch_add(frames as u64, Ordering::Relaxed);

                    // Feed the visualiser a mono downmix. `push` is allowed to
                    // fail: dropping samples when the UI is slow is invisible,
                    // whereas blocking here would be audible.
                    //
                    // Undo the volume gain first: the visualiser is showing the
                    // *music*, and bars that shrank when the user turned the
                    // volume down read as a bug. A muted output has nothing to
                    // recover, so leave it alone rather than divide by ~zero.
                    let tap_scale = if gain > 1e-4 { 1.0 / gain } else { 0.0 };
                    for frame in output[..filled].chunks_exact(channels) {
                        let mono = frame.iter().sum::<f32>() / channels as f32 * tap_scale;
                        if tap_producer.push(mono).is_err() {
                            break;
                        }
                    }
                    let silent = (output.len() - filled) / channels;
                    if silent > 0 {
                        callback_shared
                            .underrun_frames
                            .fetch_add(silent as u64, Ordering::Relaxed);
                    }
                },
                move |err| {
                    // Nothing useful to do from the audio thread; record it so
                    // the UI can surface a device problem.
                    let _ = &error_shared;
                    eprintln!("audio stream error: {err}");
                },
                None,
            )
            .context("building audio output stream")?;

        stream.play().context("starting audio stream")?;

        Ok(Self {
            stream,
            shared,
            producer,
            tap,
        })
    }
}

/// Resample interleaved f32 audio from `from_rate` to `to_rate`.
///
/// Linear interpolation is adequate for the common 44.1k -> 48k case; if
/// artifacts become audible this is the single place to swap in `rubato`.
pub fn resample_interleaved(
    input: &[f32],
    channels: usize,
    from_rate: u32,
    to_rate: u32,
    output: &mut Vec<f32>,
) {
    if channels == 0 || input.is_empty() {
        return;
    }
    if from_rate == to_rate {
        output.extend_from_slice(input);
        return;
    }

    let in_frames = input.len() / channels;
    if in_frames == 0 {
        return;
    }
    let ratio = to_rate as f64 / from_rate as f64;
    let out_frames = (in_frames as f64 * ratio).round() as usize;

    for frame in 0..out_frames {
        let src = frame as f64 / ratio;
        let index = src.floor() as usize;
        let frac = (src - index as f64) as f32;
        let next = (index + 1).min(in_frames - 1);
        for channel in 0..channels {
            let a = input[index * channels + channel];
            let b = input[next * channels + channel];
            output.push(a + (b - a) * frac);
        }
    }
}

/// Map a decoded frame's channel count onto the device's channel count.
///
/// Mono sources are duplicated across all outputs; extra source channels beyond
/// what the device has are dropped.
pub fn remap_channels(input: &[f32], from: usize, to: usize, output: &mut Vec<f32>) {
    if from == 0 || to == 0 {
        return;
    }
    if from == to {
        output.extend_from_slice(input);
        return;
    }
    for frame in input.chunks_exact(from) {
        for channel in 0..to {
            output.push(if from == 1 {
                frame[0]
            } else {
                frame[channel.min(from - 1)]
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_is_a_passthrough_at_matching_rates() {
        let input = vec![0.1, 0.2, 0.3, 0.4];
        let mut out = Vec::new();
        resample_interleaved(&input, 2, 48_000, 48_000, &mut out);
        assert_eq!(out, input);
    }

    #[test]
    fn upsampling_stretches_frame_count_by_the_rate_ratio() {
        // 100 stereo frames at 44.1k -> ~48k should yield ~109 frames.
        let input: Vec<f32> = (0..200).map(|i| i as f32).collect();
        let mut out = Vec::new();
        resample_interleaved(&input, 2, 44_100, 48_000, &mut out);
        let frames = out.len() / 2;
        assert_eq!(frames, 109);
        assert_eq!(out.len() % 2, 0, "output must stay frame-aligned");
    }

    #[test]
    fn resampling_preserves_signal_endpoints() {
        let input = vec![0.0, 0.0, 1.0, 1.0];
        let mut out = Vec::new();
        resample_interleaved(&input, 2, 44_100, 48_000, &mut out);
        assert!((out[0] - 0.0).abs() < 1e-6, "starts at the first sample");
        assert!(
            out.iter().all(|s| (-0.001..=1.001).contains(s)),
            "no overshoot"
        );
    }

    #[test]
    fn mono_is_duplicated_across_stereo_outputs() {
        let mut out = Vec::new();
        remap_channels(&[0.5, -0.5], 1, 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5, -0.5, -0.5]);
    }

    #[test]
    fn extra_source_channels_are_dropped() {
        let mut out = Vec::new();
        remap_channels(&[1.0, 2.0, 3.0, 4.0], 4, 2, &mut out);
        assert_eq!(out, vec![1.0, 2.0]);
    }

    #[test]
    fn volume_is_clamped_to_unit_range() {
        let shared = AudioShared::new(48_000, 2);
        shared.set_volume(2.5);
        assert_eq!(shared.volume(), 1.0);
        shared.set_volume(-1.0);
        assert_eq!(shared.volume(), 0.0);
    }

    #[test]
    fn elapsed_time_follows_the_frame_clock() {
        let shared = AudioShared::new(48_000, 2);
        shared.reset_clock(48_000);
        assert_eq!(shared.elapsed().as_secs(), 1);
    }
}
