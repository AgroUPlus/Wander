use anyhow::{Context, Result, anyhow};
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use tokio::sync::mpsc;

use super::output::{AudioShared, remap_channels, resample_interleaved};

/// Bridges reqwest's async byte stream to the blocking `Read` that symphonia
/// requires.
///
/// The network side pushes chunks into a bounded channel; `read` blocks on that
/// channel from inside `spawn_blocking`, so no tokio worker thread is ever
/// parked. The bound provides backpressure so we don't buffer a whole album in
/// memory when the decoder is ahead.
pub struct StreamSource {
    receiver: mpsc::Receiver<io::Result<Vec<u8>>>,
    handle: tokio::runtime::Handle,
    /// Bytes received but not yet consumed by the current `read` call.
    leftover: Vec<u8>,
    cursor: usize,
    finished: bool,
}

impl StreamSource {
    pub fn new(
        receiver: mpsc::Receiver<io::Result<Vec<u8>>>,
        handle: tokio::runtime::Handle,
    ) -> Self {
        Self {
            receiver,
            handle,
            leftover: Vec::new(),
            cursor: 0,
            finished: false,
        }
    }
}

impl Read for StreamSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        while self.cursor >= self.leftover.len() {
            if self.finished {
                return Ok(0);
            }
            match self.handle.block_on(self.receiver.recv()) {
                Some(Ok(chunk)) => {
                    self.leftover = chunk;
                    self.cursor = 0;
                }
                Some(Err(err)) => return Err(err),
                None => {
                    self.finished = true;
                    return Ok(0);
                }
            }
        }

        let available = &self.leftover[self.cursor..];
        let take = available.len().min(buf.len());
        buf[..take].copy_from_slice(&available[..take]);
        self.cursor += take;
        Ok(take)
    }
}

impl Seek for StreamSource {
    fn seek(&mut self, _pos: SeekFrom) -> io::Result<u64> {
        // A live HTTP body cannot be seeked. Seeking is implemented one level
        // up by re-issuing the request with a byte range, which builds a fresh
        // StreamSource.
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "network stream is not seekable",
        ))
    }
}

impl MediaSource for StreamSource {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

/// Why a decode run ended. The player uses this to decide what to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeOutcome {
    /// The track played to its end.
    Finished,
    /// The run was cancelled (skip, stop, seek, or a new track).
    Cancelled,
}

/// Decode `source` to completion, writing interleaved f32 into `producer`.
///
/// Runs synchronously and is meant to be called inside `spawn_blocking`. It
/// converts the decoded audio to the device's sample rate and channel layout,
/// and returns early when `cancel` is set so skipping feels immediate.
pub fn decode_stream(
    source: Box<dyn MediaSource>,
    producer: &mut rtrb::Producer<f32>,
    shared: &Arc<AudioShared>,
    cancel: &Arc<AtomicBool>,
    hint_ext: Option<&str>,
    // `seek_to`: where to start. Only a seekable source (a local file) can
    // honour a non-zero value; an HTTP stream is positioned by the server
    // instead, so callers pass zero for one.
    seek_to: Duration,
) -> Result<DecodeOutcome> {
    let mss = MediaSourceStream::new(source, MediaSourceStreamOptions::default());

    let mut hint = Hint::new();
    if let Some(ext) = hint_ext {
        hint.with_extension(ext);
    }

    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .context("could not determine the audio format of the stream")?;

    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| anyhow!("stream contains no audio track"))?;
    let track_id = track.id;
    let codec_params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| anyhow!("audio track is missing codec parameters"))?
        .clone();

    // Our registry, not symphonia's global one: it adds the Opus decoder that
    // symphonia does not ship, which roughly half this library needs.
    let mut decoder = super::opus::registry()
        .make_audio_decoder(&codec_params, &AudioDecoderOptions::default())
        .context("no decoder available for this audio codec")?;

    // Seek before decoding anything. A failure is not fatal: playing the track
    // from the start beats refusing to play it at all.
    if !seek_to.is_zero() {
        let _ = format.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: symphonia::core::units::Time::try_new(
                    seek_to.as_secs() as i64,
                    seek_to.subsec_nanos(),
                )
                .unwrap_or_default(),
                track_id: Some(track_id),
            },
        );
        decoder.reset();
    }

    let device_rate = shared.sample_rate();
    let device_channels = shared.channels() as usize;

    // Reused across packets so the hot loop does not allocate.
    let mut interleaved: Vec<f32> = Vec::new();
    let mut remapped: Vec<f32> = Vec::new();
    let mut resampled: Vec<f32> = Vec::new();

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(DecodeOutcome::Cancelled);
        }

        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            // Clean end of stream.
            Ok(None) => return Ok(DecodeOutcome::Finished),
            Err(symphonia::core::errors::Error::IoError(err))
                if err.kind() == io::ErrorKind::UnexpectedEof =>
            {
                return Ok(DecodeOutcome::Finished);
            }
            Err(err) => return Err(err).context("reading the next audio packet"),
        };

        if packet.track_id != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // A corrupt packet is worth skipping, not worth aborting the track.
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(symphonia::core::errors::Error::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(err) => return Err(err).context("decoding an audio packet"),
        };

        let (source_rate, source_channels) = {
            let spec = decoded.spec();
            (spec.rate(), spec.channels().count())
        };

        interleaved.clear();
        copy_interleaved(&decoded, &mut interleaved);

        remapped.clear();
        remap_channels(
            &interleaved,
            source_channels,
            device_channels,
            &mut remapped,
        );

        resampled.clear();
        resample_interleaved(
            &remapped,
            device_channels,
            source_rate,
            device_rate,
            &mut resampled,
        );

        // Push into the ring, waiting when it is full. This is the backpressure
        // that keeps decoding paced to playback instead of racing ahead.
        for &sample in &resampled {
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Ok(DecodeOutcome::Cancelled);
                }
                match producer.push(sample) {
                    Ok(()) => break,
                    Err(_) => std::thread::sleep(Duration::from_millis(2)),
                }
            }
        }
    }
}

fn copy_interleaved(decoded: &GenericAudioBufferRef<'_>, out: &mut Vec<f32>) {
    decoded.copy_to_vec_interleaved(out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::runtime::Runtime;

    fn source_from(chunks: Vec<Vec<u8>>) -> (StreamSource, Runtime) {
        let runtime = Runtime::new().unwrap();
        let (tx, rx) = mpsc::channel(8);
        for chunk in chunks {
            tx.blocking_send(Ok(chunk)).unwrap();
        }
        drop(tx);
        (StreamSource::new(rx, runtime.handle().clone()), runtime)
    }

    #[test]
    fn reassembles_chunks_into_a_continuous_byte_stream() {
        let (mut source, _rt) = source_from(vec![b"hello ".to_vec(), b"world".to_vec()]);
        let mut all = Vec::new();
        source.read_to_end(&mut all).unwrap();
        assert_eq!(all, b"hello world");
    }

    #[test]
    fn serves_reads_smaller_than_a_chunk() {
        let (mut source, _rt) = source_from(vec![b"abcdef".to_vec()]);
        let mut buf = [0u8; 2];
        source.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ab");
        source.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"cd");
    }

    #[test]
    fn reports_eof_once_the_sender_is_dropped() {
        let (mut source, _rt) = source_from(vec![]);
        let mut buf = [0u8; 4];
        assert_eq!(source.read(&mut buf).unwrap(), 0);
        assert_eq!(source.read(&mut buf).unwrap(), 0, "EOF is sticky");
    }

    #[test]
    fn propagates_network_errors() {
        let runtime = Runtime::new().unwrap();
        let (tx, rx) = mpsc::channel(8);
        tx.blocking_send(Err(io::Error::other("connection reset")))
            .unwrap();
        drop(tx);
        let mut source = StreamSource::new(rx, runtime.handle().clone());
        let mut buf = [0u8; 4];
        assert!(source.read(&mut buf).is_err());
    }

    #[test]
    fn advertises_itself_as_unseekable() {
        let (mut source, _rt) = source_from(vec![b"abc".to_vec()]);
        assert!(!source.is_seekable());
        assert_eq!(source.byte_len(), None);
        assert!(source.seek(SeekFrom::Start(0)).is_err());
    }
}
