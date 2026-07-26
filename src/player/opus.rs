//! Opus decoding for symphonia, backed by libopus.
//!
//! symphonia 0.6 ships no Opus decoder, and roughly half of a typical
//! Navidrome library is Opus. `symphonia-format-ogg` already demuxes the Ogg
//! container, so only the codec itself is missing: this module plugs
//! libopus into symphonia's decoder registry.

use symphonia::core::audio::{AudioBuffer, AudioSpec, Channels, GenericAudioBufferRef};
use symphonia::core::codecs::CodecInfo;
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{
    AudioCodecParameters, AudioDecoder, AudioDecoderOptions, FinalizeResult,
};
use symphonia::core::codecs::registry::{
    CodecRegistry, RegisterableAudioDecoder, SupportedAudioCodec,
};
use symphonia::core::errors::{Error, Result, decode_error, unsupported_error};
use symphonia::core::packet::PacketRef;

/// Opus always decodes to 48 kHz regardless of the source material's rate.
const OPUS_RATE: u32 = 48_000;
/// Longest Opus frame is 120 ms, i.e. 5760 samples per channel at 48 kHz.
const MAX_FRAME_SAMPLES: usize = 5760;

/// Wrapper asserting `Sync` for libopus's decoder handle.
///
/// `opus::Decoder` holds a raw pointer, so it is `Send` but not `Sync`, while
/// symphonia's `AudioDecoder` trait requires both. This is sound here because
/// the handle is only ever touched through `&mut self` methods — the sole
/// `&self` method, `last_decoded`, reads the PCM buffer and never the decoder.
/// So no two threads can reach libopus concurrently through a shared reference.
struct SyncDecoder(opus::Decoder);

// SAFETY: see the type's documentation.
unsafe impl Sync for SyncDecoder {}

impl std::ops::Deref for SyncDecoder {
    type Target = opus::Decoder;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for SyncDecoder {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub struct OpusDecoder {
    inner: SyncDecoder,
    params: AudioCodecParameters,
    buffer: AudioBuffer<f32>,
    /// Interleaved scratch that libopus writes into.
    scratch: Vec<f32>,
    channels: usize,
    /// Encoder delay still to be discarded, in frames.
    pre_skip: u64,
    /// Original pre-skip, so `reset` can restore it after a seek.
    initial_pre_skip: u64,
}

impl OpusDecoder {
    pub fn try_new(params: &AudioCodecParameters, _opts: &AudioDecoderOptions) -> Result<Self> {
        let channels = params
            .channels
            .as_ref()
            .map(|c| c.count())
            .filter(|c| *c > 0)
            .unwrap_or(2);

        // libopus itself only decodes mono or stereo streams.
        let opus_channels = match channels {
            1 => opus::Channels::Mono,
            2 => opus::Channels::Stereo,
            other => {
                return unsupported_error(Box::leak(
                    format!("opus: {other}-channel streams are not supported").into_boxed_str(),
                ));
            }
        };

        let inner = opus::Decoder::new(OPUS_RATE, opus_channels).map_err(|err| {
            Error::DecodeError(Box::leak(
                format!("opus: could not create decoder: {err}").into_boxed_str(),
            ))
        })?;

        let pre_skip = parse_pre_skip(params.extra_data.as_deref());

        let spec = AudioSpec::new(OPUS_RATE, channel_layout(channels));

        Ok(Self {
            inner: SyncDecoder(inner),
            params: params.clone(),
            buffer: AudioBuffer::new(spec, MAX_FRAME_SAMPLES),
            scratch: vec![0.0; MAX_FRAME_SAMPLES * channels],
            channels,
            pre_skip,
            initial_pre_skip: pre_skip,
        })
    }

    fn decode_into_buffer(&mut self, data: &[u8]) -> Result<()> {
        self.buffer.clear();

        let decoded = self
            .inner
            .decode_float(data, &mut self.scratch, false)
            .map_err(|_| Error::DecodeError("opus: malformed packet"))?;

        if decoded == 0 {
            return Ok(());
        }

        // Discard the encoder delay at the start of the stream, otherwise
        // playback begins with a short burst of silence/artifacts.
        let skip = self.pre_skip.min(decoded as u64) as usize;
        self.pre_skip -= skip as u64;
        let frames = decoded - skip;
        if frames == 0 {
            return Ok(());
        }

        let channels = self.channels;
        let scratch = &self.scratch;
        self.buffer
            .render_with(Some(frames), |frame, planes| {
                let base = (skip + frame) * channels;
                for (channel, plane) in planes.iter_mut().enumerate() {
                    plane[frame] = scratch[base + channel];
                }
                Ok(())
            })
            .map(|_| ())
    }
}

impl AudioDecoder for OpusDecoder {
    fn reset(&mut self) {
        // Called after a seek: drop libopus's inter-packet state and restore
        // the pre-skip so a restarted stream is handled the same as a fresh one.
        let _ = self.inner.reset_state();
        self.pre_skip = self.initial_pre_skip;
        self.buffer.clear();
    }

    fn codec_info(&self) -> &CodecInfo {
        &Self::supported_codecs()[0].info
    }

    fn codec_params(&self) -> &AudioCodecParameters {
        &self.params
    }

    fn decode_ref(&mut self, packet: &PacketRef<'_>) -> Result<GenericAudioBufferRef<'_>> {
        match self.decode_into_buffer(packet.data) {
            Ok(()) => Ok(self.last_decoded()),
            Err(err) => {
                // The trait requires the buffer be empty when decoding fails.
                self.buffer.clear();
                Err(err)
            }
        }
    }

    fn finalize(&mut self) -> FinalizeResult {
        FinalizeResult::default()
    }

    fn last_decoded(&self) -> GenericAudioBufferRef<'_> {
        GenericAudioBufferRef::F32(&self.buffer)
    }
}

impl RegisterableAudioDecoder for OpusDecoder {
    fn try_registry_new(
        params: &AudioCodecParameters,
        opts: &AudioDecoderOptions,
    ) -> Result<Box<dyn AudioDecoder>> {
        Ok(Box::new(Self::try_new(params, opts)?))
    }

    fn supported_codecs() -> &'static [SupportedAudioCodec] {
        &[SupportedAudioCodec {
            id: CODEC_ID_OPUS,
            info: CodecInfo {
                short_name: "opus",
                long_name: "Opus (libopus)",
                profiles: &[],
            },
        }]
    }
}

/// The default symphonia registry plus our Opus decoder.
///
/// Built once and shared; constructing a registry per track would re-register
/// every codec on every play.
pub fn registry() -> &'static CodecRegistry {
    static REGISTRY: std::sync::OnceLock<CodecRegistry> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut registry);
        registry.register_audio_decoder::<OpusDecoder>();
        registry
    })
}

/// Map a channel count to a symphonia channel layout.
fn channel_layout(count: usize) -> Channels {
    match count {
        1 => Channels::Positioned(symphonia::core::audio::Position::FRONT_LEFT),
        _ => Channels::Positioned(
            symphonia::core::audio::Position::FRONT_LEFT
                | symphonia::core::audio::Position::FRONT_RIGHT,
        ),
    }
}

/// Read the encoder delay from an `OpusHead` identification header.
///
/// Layout: magic "OpusHead" (8 bytes), version (1), channel count (1), then
/// pre-skip as a little-endian u16. Anything malformed means no skip, which is
/// audibly harmless.
fn parse_pre_skip(extra_data: Option<&[u8]>) -> u64 {
    let data = extra_data.unwrap_or_default();
    if data.len() < 12 || !data.starts_with(b"OpusHead") {
        return 0;
    }
    u16::from_le_bytes([data[10], data[11]]) as u64
}

#[allow(dead_code)]
fn unreachable_decode_error() -> Result<()> {
    decode_error("unreachable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pre_skip_from_opus_head() {
        let mut head = b"OpusHead".to_vec();
        head.push(1); // version
        head.push(2); // channels
        head.extend_from_slice(&312u16.to_le_bytes());
        assert_eq!(parse_pre_skip(Some(&head)), 312);
    }

    #[test]
    fn missing_or_malformed_header_means_no_skip() {
        assert_eq!(parse_pre_skip(None), 0);
        assert_eq!(parse_pre_skip(Some(b"")), 0);
        assert_eq!(parse_pre_skip(Some(b"OpusHead")), 0, "truncated header");
        assert_eq!(parse_pre_skip(Some(b"NotOpus_\x01\x02\x00\x01")), 0);
    }

    #[test]
    fn registry_resolves_an_opus_decoder() {
        let params = AudioCodecParameters {
            codec: CODEC_ID_OPUS,
            sample_rate: Some(OPUS_RATE),
            channels: Some(channel_layout(2)),
            ..Default::default()
        };
        let decoder = registry().make_audio_decoder(&params, &AudioDecoderOptions::default());
        assert!(decoder.is_ok(), "opus must be resolvable from the registry");
    }

    #[test]
    fn registry_still_resolves_a_builtin_codec() {
        use symphonia::core::codecs::audio::well_known::CODEC_ID_FLAC;
        let params = AudioCodecParameters {
            codec: CODEC_ID_FLAC,
            sample_rate: Some(44_100),
            channels: Some(channel_layout(2)),
            bits_per_sample: Some(16),
            ..Default::default()
        };
        // The invariant that matters is parity: our registry must resolve
        // exactly what symphonia's default one does. Comparing against the
        // default avoids asserting anything about whether these particular
        // params are sufficient to *construct* a FLAC decoder.
        let ours = registry()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .is_ok();
        let theirs = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .is_ok();
        assert_eq!(
            ours, theirs,
            "registering Opus must not change how built-in codecs resolve"
        );
    }

    #[test]
    fn decodes_a_real_opus_packet_to_pcm() {
        // Encode a tone with libopus, then decode it back through our decoder:
        // proves the full libopus round trip works, without a fixture file.
        let mut encoder =
            opus::Encoder::new(OPUS_RATE, opus::Channels::Stereo, opus::Application::Audio)
                .unwrap();
        let frames = 960; // 20 ms at 48 kHz
        let pcm: Vec<f32> = (0..frames * 2)
            .map(|i| ((i / 2) as f32 * 0.05).sin() * 0.5)
            .collect();
        let mut packet = vec![0u8; 4000];
        let len = encoder.encode_float(&pcm, &mut packet).unwrap();
        packet.truncate(len);

        let params = AudioCodecParameters {
            codec: CODEC_ID_OPUS,
            sample_rate: Some(OPUS_RATE),
            channels: Some(channel_layout(2)),
            ..Default::default()
        };
        let mut decoder = OpusDecoder::try_new(&params, &AudioDecoderOptions::default()).unwrap();
        decoder.decode_into_buffer(&packet).unwrap();

        let mut out: Vec<f32> = Vec::new();
        decoder.last_decoded().copy_to_vec_interleaved(&mut out);
        assert_eq!(
            out.len(),
            frames * 2,
            "should decode one 20 ms stereo frame"
        );
        assert!(
            out.iter().any(|s| s.abs() > 0.01),
            "decoded audio should not be silence"
        );
    }

    #[test]
    fn pre_skip_is_restored_on_reset() {
        let mut head = b"OpusHead".to_vec();
        head.extend_from_slice(&[1, 2]);
        head.extend_from_slice(&312u16.to_le_bytes());
        let params = AudioCodecParameters {
            codec: CODEC_ID_OPUS,
            sample_rate: Some(OPUS_RATE),
            channels: Some(channel_layout(2)),
            extra_data: Some(head.into_boxed_slice()),
            ..Default::default()
        };
        let mut decoder = OpusDecoder::try_new(&params, &AudioDecoderOptions::default()).unwrap();
        assert_eq!(decoder.pre_skip, 312);
        decoder.pre_skip = 0;
        decoder.reset();
        assert_eq!(
            decoder.pre_skip, 312,
            "seek must re-apply the encoder delay"
        );
    }
}
