pub mod decoder;
pub mod opus;
pub mod output;
pub mod queue;
pub mod radio;
pub mod spectrum;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::library::{Library, Source};
use crate::subsonic::models::Song;
use decoder::{DecodeOutcome, StreamSource, decode_stream};
use output::{AudioOutput, AudioShared};
use queue::{Queue, Repeat};

/// How many network chunks may sit between the fetcher and the decoder.
const NETWORK_CHANNEL_DEPTH: usize = 16;
const SEEK_STEP: Duration = Duration::from_secs(5);
/// Pressing previous within this window goes to the previous track; after it,
/// previous restarts the current track.
const RESTART_THRESHOLD: Duration = Duration::from_secs(3);
/// Radio mode refills once fewer than this many tracks remain queued.
const RADIO_LOW_WATER: usize = 5;
/// How many tracks to add per refill, and how many to score when choosing.
///
/// Deliberately small: a steady trickle of a few tracks keeps the queue looking
/// like a live stream, where one big batch reads as a playlist that was dumped
/// in and will eventually run out.
const RADIO_BATCH: usize = 5;
const RADIO_CANDIDATES: u32 = 100;
/// How many neighbouring artists to pull tracks from, and how many each.
const RADIO_SIMILAR_ARTISTS: u32 = 6;
const RADIO_ARTIST_TRACKS: u32 = 10;
/// How far back to look when measuring artist/album saturation.
const RADIO_RECENT: usize = 20;
/// How often to check whether a finished track's tail has finished playing.
const DRAIN_POLL: Duration = Duration::from_millis(100);
/// How often radio mode checks whether the queue needs topping up.
///
/// Track-end is not the only way a queue drains — skipping and removing do it
/// too — so a periodic check is what makes the stream feel endless rather than
/// only recovering at the next song boundary.
const RADIO_POLL: Duration = Duration::from_secs(2);
/// How long to wait before trying again after a refill found nothing, so a
/// library with no similarity data is not hammered every [`RADIO_POLL`].
const RADIO_BACKOFF: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub enum PlayerCommand {
    /// Replace the queue with `songs` and start at `index`.
    PlayNow {
        songs: Vec<Song>,
        index: usize,
    },
    Enqueue(Vec<Song>),
    PlayQueueIndex(usize),
    /// Load the current track at `offset` but stay paused, so a restored
    /// session resumes exactly where it left off on the next play press.
    Resume {
        offset: Duration,
    },
    TogglePause,
    Stop,
    Next,
    Prev,
    SeekForward,
    SeekBackward,
    /// Absolute seek, used by click/drag on the seek bar.
    SeekTo(Duration),
    /// Absolute volume in `[0, 1]`, used by click/drag on the volume slider.
    SetVolume(f32),
    AdjustVolume(f32),
    ToggleShuffle,
    ToggleRepeat,
    SetRepeat(Repeat),
    /// Toggle radio mode, which keeps the queue topped up automatically.
    ToggleRadio,
    SetStarred {
        song_id: String,
        starred: bool,
    },
    Remove(usize),
    Clear,
}

/// Snapshot of playback state for the UI.
///
/// A plain mutex is safe here because the realtime audio callback never touches
/// it — only the UI thread and the player task do. Position comes from the
/// lock-free clock in [`AudioShared`] instead.
#[derive(Debug, Default, Clone)]
pub struct Status {
    pub current: Option<Song>,
    pub playing: bool,
    pub buffering: bool,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct PlayerHandle {
    commands: mpsc::UnboundedSender<PlayerCommand>,
    pub shared: Arc<AudioShared>,
    pub queue: Arc<Mutex<Queue>>,
    pub status: Arc<Mutex<Status>>,
}

impl PlayerHandle {
    pub fn send(&self, command: PlayerCommand) {
        // A closed channel only happens once the player task has shut down, at
        // which point dropping the command is the correct behaviour.
        let _ = self.commands.send(command);
    }

    pub fn elapsed(&self) -> Duration {
        self.shared.elapsed()
    }

    pub fn volume(&self) -> f32 {
        self.shared.volume()
    }

    pub fn is_paused(&self) -> bool {
        self.shared.is_paused()
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
}

/// Spawn the player task.
///
/// Returns the handle plus the live [`AudioOutput`]; the caller must keep the
/// latter alive, because dropping it stops the audio device.
pub fn spawn(
    library: Arc<dyn Library>,
    buffer_seconds: f32,
) -> Result<(PlayerHandle, AudioOutput)> {
    let mut output = AudioOutput::new(buffer_seconds).context("initialising audio output")?;
    let shared = Arc::clone(&output.shared);
    let queue = Arc::new(Mutex::new(Queue::default()));
    let status = Arc::new(Mutex::new(Status::default()));

    // The producer moves into the player task; the stream stays with the caller.
    let (placeholder, _) = rtrb::RingBuffer::<f32>::new(1);
    let producer = std::mem::replace(&mut output.producer, placeholder);

    let (tx, rx) = mpsc::unbounded_channel();

    let task = PlayerTask {
        library,
        producer: Some(producer),
        shared: Arc::clone(&shared),
        queue: Arc::clone(&queue),
        status: Arc::clone(&status),
        decode: None,
        cancel: Arc::new(AtomicBool::new(false)),
        net_handle: None,
        transcode_retry: None,
        draining: false,
        radio_pool: Vec::new(),
        radio_pool_seed: None,
        radio_pool_similar: std::collections::HashSet::new(),
        radio_retry_after: None,
    };

    tokio::spawn(task.run(rx));

    Ok((
        PlayerHandle {
            commands: tx,
            shared,
            queue,
            status,
        },
        output,
    ))
}

/// What a decode run hands back: the ring producer, so the next track can reuse
/// it, plus how the run ended.
type DecodeResult = (rtrb::Producer<f32>, Result<DecodeOutcome>);

struct PlayerTask {
    library: Arc<dyn Library>,
    producer: Option<rtrb::Producer<f32>>,
    shared: Arc<AudioShared>,
    queue: Arc<Mutex<Queue>>,
    status: Arc<Mutex<Status>>,
    decode: Option<JoinHandle<DecodeResult>>,
    cancel: Arc<AtomicBool>,
    net_handle: Option<JoinHandle<()>>,
    /// Song we have already retried via server transcode, so a file our
    /// decoders reject is retried exactly once rather than looping.
    transcode_retry: Option<String>,
    /// The decoder has finished, but buffered audio is still being played out.
    draining: bool,
    /// Candidates gathered for radio mode but not yet queued.
    ///
    /// Refills are small and frequent, and the pools they draw on are several
    /// network round-trips wide. Keeping the leftovers means a refill usually
    /// costs nothing but scoring, and only a change of seed artist — or an
    /// exhausted pool — goes back to the library.
    radio_pool: Vec<Song>,
    /// Artist the cached pool was gathered for.
    radio_pool_seed: Option<String>,
    /// Similar-artist names found for that seed, cached alongside the pool so
    /// refills served from it still score artist affinity the same way.
    radio_pool_similar: std::collections::HashSet<String>,
    /// Set when a refill came back empty; suppresses the periodic retry until
    /// it passes.
    radio_retry_after: Option<Instant>,
}

impl PlayerTask {
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<PlayerCommand>) {
        loop {
            // While draining, poll the ring buffer; otherwise this branch never
            // fires and the task simply blocks on the other two.
            let drain_tick = async {
                if self.draining {
                    tokio::time::sleep(DRAIN_POLL).await;
                } else {
                    std::future::pending::<()>().await;
                }
            };

            // Radio mode watches the queue continuously rather than only at
            // track boundaries, so skipping or removing tracks can never drain
            // it faster than it refills.
            let radio_on = self.queue.lock().unwrap().radio;
            let radio_tick = async {
                if radio_on {
                    tokio::time::sleep(RADIO_POLL).await;
                } else {
                    std::future::pending::<()>().await;
                }
            };

            // Wait for a user command, the current track to end, or the tail of
            // a finished track to finish playing.
            let finished = tokio::select! {
                command = commands.recv() => {
                    match command {
                        Some(command) => {
                            self.handle(command).await;
                            continue;
                        }
                        // Every handle was dropped: the app is shutting down.
                        None => return,
                    }
                }
                result = await_decode(&mut self.decode) => result,
                _ = drain_tick => {
                    if self.tail_is_playing() {
                        continue;
                    }
                    self.draining = false;
                    self.advance_after_track_end().await;
                    continue;
                }
                _ = radio_tick => {
                    self.top_up_radio().await;
                    continue;
                }
            };

            self.decode = None;
            self.producer = Some(finished.0);
            match finished.1 {
                // The decoder is done, but up to `buffer_seconds` of audio is
                // still queued for the device. Advancing now would flush it and
                // cut the end off every track.
                Ok(DecodeOutcome::Finished) => {
                    self.draining = true;
                    self.on_track_finished().await;
                }
                Ok(DecodeOutcome::Cancelled) => {}
                Err(err) => self.recover_or_report(err).await,
            }
        }
    }

    /// Whether audio the device has not played yet is still buffered.
    ///
    /// Paused playback drains nothing, so this stays true and the track does
    /// not advance until the user resumes — which is what they would expect.
    fn tail_is_playing(&self) -> bool {
        let Some(producer) = self.producer.as_ref() else {
            return false;
        };
        tail_is_playing(producer.buffer().capacity(), producer.slots())
    }

    async fn handle(&mut self, command: PlayerCommand) {
        match command {
            PlayerCommand::PlayNow { songs, index } => {
                {
                    let mut queue = self.queue.lock().unwrap();
                    queue.clear();
                    queue.extend(songs);
                    queue.play_at(index);
                }
                self.start_current().await;
            }
            PlayerCommand::Enqueue(songs) => {
                // Enqueuing into an idle player should start playback.
                let start = {
                    let mut queue = self.queue.lock().unwrap();
                    let was_empty = queue.is_empty();
                    queue.extend(songs);
                    if was_empty {
                        queue.play_at(0);
                    }
                    was_empty || self.status.lock().unwrap().current.is_none()
                };
                if start {
                    self.start_current().await;
                }
            }
            PlayerCommand::PlayQueueIndex(index) => {
                self.queue.lock().unwrap().play_at(index);
                self.start_current().await;
                // Jumping near the end leaves little ahead of the new position.
                self.top_up_radio().await;
            }
            PlayerCommand::Resume { offset } => {
                let current = self.queue.lock().unwrap().current().cloned();
                if let Some(song) = current {
                    // Clamp inside the track: a stale offset from a crash could
                    // otherwise point past the end.
                    let offset = clamp_into_track(offset, song.duration);
                    self.start_song(song, offset).await;
                    // `start_song` always unpauses, so pause afterwards.
                    self.shared.set_paused(true);
                    self.status.lock().unwrap().playing = false;
                }
            }
            PlayerCommand::TogglePause => {
                if self.status.lock().unwrap().current.is_none()
                    || (self.decode.is_none() && !self.draining)
                {
                    let current = self.queue.lock().unwrap().current().cloned();
                    if let Some(song) = current {
                        self.start_song(song, Duration::ZERO).await;
                        return;
                    } else {
                        let first = {
                            let mut queue = self.queue.lock().unwrap();
                            if !queue.is_empty() {
                                queue.play_at(0);
                                queue.current().cloned()
                            } else {
                                None
                            }
                        };
                        if let Some(song) = first {
                            self.start_song(song, Duration::ZERO).await;
                            return;
                        }
                    }
                }
                let paused = !self.shared.is_paused();
                self.shared.set_paused(paused);
                self.status.lock().unwrap().playing = !paused;
            }
            PlayerCommand::Stop => self.halt().await,
            PlayerCommand::Next => {
                // Skipping drains the queue just as surely as playing does, so
                // radio has to be given the chance to keep ahead of it.
                if self.queue.lock().unwrap().radio {
                    self.ensure_radio_has_next().await;
                }
                let next = self.queue.lock().unwrap().next().cloned();
                match next {
                    Some(_) => self.start_current().await,
                    None => self.halt().await,
                }
            }
            PlayerCommand::Prev => {
                if self.shared.elapsed() > RESTART_THRESHOLD {
                    self.start_current().await;
                } else {
                    let prev = self.queue.lock().unwrap().prev().cloned();
                    if prev.is_some() {
                        self.start_current().await;
                    }
                }
            }
            PlayerCommand::SeekForward => {
                self.seek_to(self.shared.elapsed() + SEEK_STEP).await;
            }
            PlayerCommand::SeekBackward => {
                self.seek_to(self.shared.elapsed().saturating_sub(SEEK_STEP))
                    .await;
            }
            PlayerCommand::SeekTo(target) => self.seek_to(target).await,
            PlayerCommand::SetVolume(volume) => self.shared.set_volume(volume),
            PlayerCommand::AdjustVolume(delta) => {
                self.shared.set_volume(self.shared.volume() + delta);
            }
            PlayerCommand::ToggleShuffle => self.queue.lock().unwrap().toggle_shuffle(),
            PlayerCommand::ToggleRadio => {
                let enabled = {
                    let mut queue = self.queue.lock().unwrap();
                    queue.radio = !queue.radio;
                    queue.radio
                };
                // Fill immediately so switching it on has a visible effect.
                if enabled {
                    self.force_top_up_radio().await;
                }
            }
            PlayerCommand::SetStarred { song_id, starred } => {
                let library = Arc::clone(&self.library);
                tokio::spawn(async move {
                    let _ = library.set_starred(&song_id, starred).await;
                });
            }
            PlayerCommand::ToggleRepeat => self.queue.lock().unwrap().toggle_repeat(),
            PlayerCommand::SetRepeat(repeat) => self.queue.lock().unwrap().set_repeat(repeat),
            PlayerCommand::Remove(index) => {
                let (removed_current, still_playing) = {
                    let mut queue = self.queue.lock().unwrap();
                    let was_current = queue.current_index() == Some(index);
                    queue.remove(index);
                    (was_current, queue.current().is_some())
                };
                if removed_current {
                    if still_playing {
                        self.start_current().await;
                    } else {
                        self.halt().await;
                    }
                }
                self.top_up_radio().await;
            }
            PlayerCommand::Clear => {
                self.queue.lock().unwrap().clear();
                self.halt().await;
            }
        }
    }

    /// Everything that can be done while the track's tail is still playing.
    ///
    /// Run at decode-end rather than at advance-time so the scrobble and any
    /// radio fetch overlap with the last seconds of audio, instead of adding
    /// their latency as a silent gap between tracks.
    async fn on_track_finished(&mut self) {
        // A completed play is what feeds the server's play counts and
        // recently-played, which smart shuffle then draws on.
        let finished = self.queue.lock().unwrap().current().cloned();
        if let Some(song) = finished {
            // Also logged locally: the server keeps counts, not a play log, and
            // the Home tab's statistics need the timeline.
            crate::history::append(&crate::history::PlayRecord::from_song(&song));
            let library = Arc::clone(&self.library);
            tokio::spawn(async move {
                let _ = library.scrobble(&song.id, true).await;
            });
        }

        self.force_top_up_radio().await;
    }

    async fn advance_after_track_end(&mut self) {
        if self.queue.lock().unwrap().radio {
            self.ensure_radio_has_next().await;
        }
        let next = self
            .queue
            .lock()
            .unwrap()
            .advance_after_track_end()
            .cloned();
        match next {
            Some(_) => self.start_current().await,
            None => self.halt().await,
        }
    }

    /// Guarantee radio mode has something to move on to.
    ///
    /// This has to run *before* the queue advances: running off the end clears
    /// the position, and with it the seed that everything is chosen from — so
    /// recovering afterwards would mean restarting the queue from the top
    /// instead of continuing the stream.
    async fn ensure_radio_has_next(&mut self) {
        {
            let queue = self.queue.lock().unwrap();
            // Repeat-one never moves on, so there is nothing to have ready.
            if queue.remaining() > 0 || queue.repeat == Repeat::One {
                return;
            }
        }
        self.force_top_up_radio().await;
        if self.queue.lock().unwrap().remaining() > 0 {
            return;
        }

        // Similarity found nothing usable — an obscure seed on a library with
        // no similarity service, most likely. Reach for the library at large
        // rather than stopping, which is what radio mode promises.
        let fallback = self.radio_fallback_songs().await;
        if fallback.is_empty() {
            return;
        }
        self.queue.lock().unwrap().extend(fallback);
    }

    /// Top up regardless of any back-off from an earlier empty refill.
    ///
    /// For the moments the user is watching — a track ending, radio being
    /// switched on, the queue actually running out — where one wasted request
    /// is much cheaper than visibly doing nothing.
    async fn force_top_up_radio(&mut self) {
        self.radio_retry_after = None;
        self.top_up_radio().await;
    }

    /// In radio mode, extend the queue with tracks that suit what is playing.
    ///
    /// Runs before advancing so there is always something to move on to.
    async fn top_up_radio(&mut self) {
        if let Some(retry_after) = self.radio_retry_after {
            if Instant::now() < retry_after {
                return;
            }
            self.radio_retry_after = None;
        }

        let (radio, remaining, seed, queued, recent) = {
            let queue = self.queue.lock().unwrap();
            (
                queue.radio,
                queue.remaining(),
                queue.current().cloned(),
                queue.ids(),
                queue.recent(RADIO_RECENT).to_vec(),
            )
        };
        if !radio || remaining >= RADIO_LOW_WATER {
            return;
        }

        // Nothing is playing, so there is nothing to be similar *to*. Rather
        // than doing nothing — which looks like radio mode is broken — start
        // the stream from the library itself and let the next refill, which
        // now has a seed, take over.
        let Some(seed) = seed else {
            self.seed_radio_from_nothing().await;
            return;
        };

        let mut context = radio::Context::from_recent(&recent);

        // Reuse what the last gather turned up. Refills are small, so most of
        // them are satisfied entirely from here and cost no network at all.
        let pool_seed = seed.artist_id.clone().unwrap_or_else(|| seed.id.clone());
        if self.radio_pool_seed.as_ref() != Some(&pool_seed) {
            self.radio_pool.clear();
            self.radio_pool_similar.clear();
        }
        self.radio_pool.retain(|song| !queued.contains(&song.id));
        context
            .similar_artists
            .extend(self.radio_pool_similar.iter().cloned());
        if self.radio_pool.len() >= RADIO_BATCH * 2 {
            let candidates = std::mem::take(&mut self.radio_pool);
            self.pick_and_extend(&seed, candidates, &queued, &context);
            return;
        }

        let mut candidates: Vec<Song> = std::mem::take(&mut self.radio_pool);
        self.radio_pool_seed = Some(pool_seed);

        // 1. Server-side similarity. Best signal when it exists, but Navidrome
        //    returns nothing without Last.fm, so it can never be the only pool.
        candidates.extend(
            self.library
                .similar_songs(&seed.id, RADIO_CANDIDATES)
                .await
                .unwrap_or_default(),
        );

        // 2. Neighbouring artists' best-known tracks. This is what makes the
        //    stream travel outward instead of circling one discography.
        if let Some(artist_id) = seed.artist_id.as_deref() {
            let neighbours = self
                .library
                .similar_artists(artist_id, RADIO_SIMILAR_ARTISTS)
                .await
                .unwrap_or_default();
            for artist in neighbours.iter().take(RADIO_SIMILAR_ARTISTS as usize) {
                context.similar_artists.insert(artist.name.to_lowercase());
                self.radio_pool_similar.insert(artist.name.to_lowercase());
                candidates.extend(
                    self.library
                        .top_songs(&artist.name, RADIO_ARTIST_TRACKS)
                        .await
                        .unwrap_or_default(),
                );
            }
        }

        // 3. The seed's own genres.
        for genre in seed.genre_names().into_iter().take(2) {
            candidates.extend(
                self.library
                    .songs_by_genre(&genre, RADIO_CANDIDATES, 0)
                    .await
                    .unwrap_or_default(),
            );
        }

        // 4. A dose of the familiar, so the stream is not all discovery.
        candidates.extend(self.library.starred_songs().await.unwrap_or_default());

        // 5. Safety net: whatever the library has, so an obscure seed with no
        //    similar artists and a rare genre still cannot starve the queue.
        if candidates.len() < RADIO_BATCH * 2 {
            let genre = seed.genre_names().into_iter().next();
            candidates.extend(
                self.library
                    .random_songs(RADIO_CANDIDATES, genre.as_deref())
                    .await
                    .unwrap_or_default(),
            );
            candidates.extend(
                self.library
                    .random_songs(RADIO_CANDIDATES, None)
                    .await
                    .unwrap_or_default(),
            );
        }

        self.pick_and_extend(&seed, candidates, &queued, &context);
    }

    /// Score `candidates`, queue the best few, and keep the rest for next time.
    fn pick_and_extend(
        &mut self,
        seed: &Song,
        candidates: Vec<Song>,
        queued: &std::collections::HashSet<String>,
        context: &radio::Context,
    ) {
        let picked = radio::pick(seed, candidates.clone(), queued, context, RADIO_BATCH);
        if picked.is_empty() {
            // Every pool came back empty or fully excluded. Say so, because a
            // silently unchanged queue is indistinguishable from a bug — and
            // back off, so the periodic check does not repeat this every
            // couple of seconds.
            self.radio_pool.clear();
            self.radio_retry_after = Some(Instant::now() + RADIO_BACKOFF);
            self.notice("Radio mode found nothing new to add".to_string());
            return;
        }

        // Whatever was not chosen stays around as the next refill's pool.
        self.radio_pool = radio::leftovers(candidates, &picked, queued);

        self.queue.lock().unwrap().extend(picked);
    }

    /// Whatever the library can offer unprompted, for when similarity has
    /// nothing to say: starred tracks plus a random draw, shuffled.
    ///
    /// Already-queued tracks are excluded, so this can also be used to extend a
    /// running stream rather than only to start one.
    async fn radio_fallback_songs(&mut self) -> Vec<Song> {
        let mut candidates = self.library.starred_songs().await.unwrap_or_default();
        candidates.extend(
            self.library
                .random_songs(RADIO_CANDIDATES, None)
                .await
                .unwrap_or_default(),
        );

        // De-duplicate and shuffle, so an unseeded start is not the same tracks
        // in the same order every time.
        let queued = self.queue.lock().unwrap().ids();
        let mut seen = std::collections::HashSet::new();
        candidates.retain(|song| !queued.contains(&song.id) && seen.insert(song.id.clone()));
        for i in (1..candidates.len()).rev() {
            candidates.swap(i, rand::random_range(0..=i));
        }
        candidates.truncate(RADIO_BATCH);
        candidates
    }

    /// Start a radio stream with no track playing, using whatever the library
    /// can offer unprompted.
    async fn seed_radio_from_nothing(&mut self) {
        let candidates = self.radio_fallback_songs().await;
        if candidates.is_empty() {
            self.notice("Radio mode found no tracks to start from".to_string());
            return;
        }

        let start = {
            let mut queue = self.queue.lock().unwrap();
            let was_idle = queue.current().is_none();
            queue.extend(candidates);
            if was_idle {
                queue.play_at(0);
            }
            was_idle
        };
        if start {
            self.start_current().await;
        }
    }

    /// Stop playback entirely and reset the clock.
    async fn halt(&mut self) {
        self.draining = false;
        self.stop_decoding().await;
        self.shared.reset_clock(0);
        let mut status = self.status.lock().unwrap();
        status.playing = false;
        status.current = None;
    }

    async fn seek_to(&mut self, target: Duration) {
        let current = self.queue.lock().unwrap().current().cloned();
        let Some(song) = current else { return };
        let target = clamp_into_track(target, song.duration);
        self.start_song(song, target).await;
    }

    async fn start_current(&mut self) {
        let current = self.queue.lock().unwrap().current().cloned();
        if let Some(song) = current {
            self.start_song(song, Duration::ZERO).await;
        }
    }

    /// Stop any running decode and begin `song` at `offset`.
    async fn start_song(&mut self, song: Song, offset: Duration) {
        self.draining = false;
        self.stop_decoding().await;

        {
            let mut status = self.status.lock().unwrap();
            status.current = Some(song.clone());
            status.playing = true;
            status.buffering = true;
            status.error = None;
        }
        // Tell the server what is playing now, so its "now playing" view and
        // any connected clients reflect this session. Fire-and-forget: a
        // scrobble failure must never interrupt playback.
        {
            let library = Arc::clone(&self.library);
            let id = song.id.clone();
            tokio::spawn(async move {
                let _ = library.scrobble(&id, false).await;
            });
        }

        self.shared.set_paused(false);
        // The clock *is* the playback position, so it starts at the offset.
        self.shared
            .reset_clock((offset.as_secs_f64() * self.shared.sample_rate() as f64) as u64);

        let Some(producer) = self.producer.take() else {
            self.report("audio ring buffer unavailable".to_string());
            return;
        };

        // A fresh flag per run, so cancelling the old decode cannot cancel this one.
        self.cancel = Arc::new(AtomicBool::new(false));

        // Seeking a raw (untranscoded) stream restarts from the beginning,
        // since timeOffset only applies to transcoded output.
        let force_transcode = self.transcode_retry.as_deref() == Some(song.id.as_str());
        let source = match self.library.open(&song, offset, force_transcode).await {
            Ok(source) => source,
            Err(err) => {
                // Hand the ring producer back before bailing, or the next track
                // would find it missing and fail too.
                self.producer = Some(producer);
                self.report(format!("{err:#}"));
                return;
            }
        };

        // The clock was set to the seek target optimistically; if the source
        // could not honour it, correct it now rather than reporting a position
        // the audio never reached.
        if let Source::Http { starts_at, .. } = &source
            && *starts_at != offset
        {
            self.shared
                .reset_clock((starts_at.as_secs_f64() * self.shared.sample_rate() as f64) as u64);
        }

        // A local file is seekable, so the decoder can start at the offset
        // itself; an HTTP body is not, which is why the server is asked to
        // start there instead.
        let seek_to = match &source {
            Source::File(_) => offset,
            Source::Http { .. } => Duration::ZERO,
        };

        let media: Box<dyn symphonia::core::io::MediaSource> = match source {
            Source::File(path) => match std::fs::File::open(&path) {
                Ok(file) => Box::new(file),
                Err(err) => {
                    self.producer = Some(producer);
                    self.report(format!("opening {}: {err}", path.display()));
                    return;
                }
            },
            Source::Http { url, http, .. } => {
                let (bytes_tx, bytes_rx) = mpsc::channel(NETWORK_CHANNEL_DEPTH);
                let cancel_net = Arc::clone(&self.cancel);

                // Network pump: fetch the body and feed chunks to the decoder,
                // resuming with a byte range if the connection breaks partway.
                //
                // Without the resume, a dropped connection reached the decoder
                // as a clean end-of-file, which is indistinguishable from a
                // track that simply ended — so playback silently skipped to the
                // next song mid-track instead of recovering.
                let net_handle = tokio::spawn(async move {
                    let mut received: u64 = 0;
                    let mut total: Option<u64> = None;
                    let mut attempt = 0u32;

                    loop {
                        if cancel_net.load(Ordering::Relaxed) {
                            return;
                        }

                        let mut request = http.get(&url);
                        if received > 0 {
                            request = request.header("Range", format!("bytes={received}-"));
                        }

                        let response = match request.send().await {
                            Ok(response) => response,
                            Err(err) => {
                                if resume_again(&mut attempt, received).await {
                                    continue;
                                }
                                let _ = bytes_tx
                                    .send(Err(std::io::Error::other(format!(
                                        "stream request failed: {err}"
                                    ))))
                                    .await;
                                return;
                            }
                        };

                        if !response.status().is_success() {
                            let _ = bytes_tx
                                .send(Err(std::io::Error::other(format!(
                                    "stream returned HTTP {}",
                                    response.status()
                                ))))
                                .await;
                            return;
                        }

                        // A server that ignores Range replies 200 with the whole
                        // body; the bytes already delivered have to be dropped
                        // rather than fed to the decoder a second time.
                        let mut to_discard = if received > 0
                            && response.status() != reqwest::StatusCode::PARTIAL_CONTENT
                        {
                            received
                        } else {
                            0
                        };

                        if total.is_none() && received == 0 {
                            total = response.content_length();
                        }

                        let mut stream = response.bytes_stream();
                        let mut broke = false;
                        while let Some(chunk) = stream.next().await {
                            if cancel_net.load(Ordering::Relaxed) {
                                return;
                            }
                            let mut bytes = match chunk {
                                Ok(bytes) => bytes.to_vec(),
                                Err(_) => {
                                    broke = true;
                                    break;
                                }
                            };

                            if to_discard > 0 {
                                let drop_now = to_discard.min(bytes.len() as u64);
                                bytes.drain(..drop_now as usize);
                                to_discard -= drop_now;
                                if bytes.is_empty() {
                                    continue;
                                }
                            }

                            received += bytes.len() as u64;
                            // A send error means the decoder is gone.
                            if bytes_tx.send(Ok(bytes)).await.is_err() {
                                return;
                            }
                        }

                        // Short of the advertised length: the body was cut off,
                        // so pick up where it stopped instead of pretending the
                        // track ended here.
                        let truncated = matches!(total, Some(len) if received < len);
                        if (broke || truncated) && resume_again(&mut attempt, received).await {
                            continue;
                        }

                        if broke || truncated {
                            let _ = bytes_tx
                                .send(Err(std::io::Error::other(
                                    "stream ended early and could not be resumed",
                                )))
                                .await;
                        }
                        return;
                    }
                });
                self.net_handle = Some(net_handle);

                Box::new(StreamSource::new(
                    bytes_rx,
                    tokio::runtime::Handle::current(),
                ))
            }
        };

        let shared = Arc::clone(&self.shared);
        let cancel = Arc::clone(&self.cancel);
        let hint = song.suffix.clone();

        // Decoding blocks, so it must never run on a reactor thread.
        self.decode = Some(tokio::task::spawn_blocking(move || {
            let mut producer = producer;
            let outcome = decode_stream(
                media,
                &mut producer,
                &shared,
                &cancel,
                hint.as_deref(),
                seek_to,
            );
            (producer, outcome)
        }));

        self.status.lock().unwrap().buffering = false;
    }

    /// Cancel the running decode, reclaim the ring producer, and drop buffered
    /// audio so the next track starts cleanly.
    async fn stop_decoding(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(net_handle) = self.net_handle.take() {
            net_handle.abort();
        }
        if let Some(handle) = self.decode.take()
            && let Ok((producer, _)) = handle.await
        {
            self.producer = Some(producer);
        }
        self.shared.request_flush();
    }

    /// Handle a decode failure.
    ///
    /// Some files our decoders reject (unusual MP4 layouts, for instance) play
    /// fine once the server re-encodes them, so the first failure for a track
    /// retries via transcode rather than dropping the user out of playback.
    /// A second failure is reported, so a genuinely broken file cannot loop.
    async fn recover_or_report(&mut self, err: anyhow::Error) {
        let current = self.queue.lock().unwrap().current().cloned();
        let Some(song) = current else {
            self.report(format!("{err:#}"));
            return;
        };

        if self.transcode_retry.as_deref() == Some(song.id.as_str()) {
            self.report(format!("{err:#}"));
            return;
        }

        self.transcode_retry = Some(song.id.clone());
        self.status.lock().unwrap().error = None;
        // Resume where playback stopped rather than restarting the track.
        let resume = self.shared.elapsed();
        self.start_song(song, resume).await;
    }

    fn report(&self, message: String) {
        let mut status = self.status.lock().unwrap();
        status.error = Some(message);
        status.playing = false;
        status.buffering = false;
    }

    /// Surface a message without touching playback.
    ///
    /// Unlike [`Self::report`], this is for things the user should know about
    /// that are not failures of the current track — a radio refill that found
    /// nothing happens while music is still playing perfectly well.
    fn notice(&self, message: String) {
        self.status.lock().unwrap().error = Some(message);
    }
}

/// Whether a broken stream is worth another byte-range request.
///
/// Bounded so a server that keeps closing the connection surfaces an error
/// rather than retrying for ever, and only when some bytes have arrived, since
/// a stream that fails at offset zero is not a connection blip.
async fn resume_again(attempt: &mut u32, received: u64) -> bool {
    const MAX_RESUMES: u32 = 5;
    if received == 0 || *attempt >= MAX_RESUMES {
        return false;
    }
    *attempt += 1;
    tokio::time::sleep(Duration::from_millis(250 * u64::from(*attempt))).await;
    true
}

/// Clamp a seek target so it lands inside the track.
///
/// A `duration` of zero means *unknown*, not *empty*: Subsonic omits the field
/// on some entries, and a file whose header cannot be read is indexed without
/// one. There is no end to clamp against in that case, so the target is left
/// alone — clamping it to zero instead turned every seek on such a track into a
/// restart, and made resuming one start from the beginning.
fn clamp_into_track(target: Duration, duration: u32) -> Duration {
    if duration == 0 {
        return target;
    }
    target.min(Duration::from_secs(duration.saturating_sub(1) as u64))
}

/// Await the current decode task, or never resolve when nothing is decoding.
///
/// The `pending` branch is what lets the player's `select!` wait on commands
/// alone while idle.
/// Whether enough audio remains queued for the device that advancing now would
/// audibly cut the track short.
///
/// A little slack is allowed: the callback consumes in chunks, so insisting on
/// an exactly empty ring would wait for a state that never quite arrives.
fn tail_is_playing(capacity: usize, free_slots: usize) -> bool {
    let buffered = capacity.saturating_sub(free_slots);
    buffered > capacity / 100
}

async fn await_decode(slot: &mut Option<JoinHandle<DecodeResult>>) -> DecodeResult {
    match slot {
        Some(handle) => match handle.await {
            Ok(result) => result,
            // The blocking task panicked; hand back a dummy producer so the
            // player keeps running and surface the failure.
            Err(err) => (
                rtrb::RingBuffer::new(1).0,
                Err(anyhow::anyhow!("decoder task failed: {err}")),
            ),
        },
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeking_stays_inside_a_track_of_known_length() {
        // One second short of the end, so the seek cannot land past the last
        // packet and immediately trigger an end-of-stream.
        assert_eq!(
            clamp_into_track(Duration::from_secs(500), 180),
            Duration::from_secs(179)
        );
        assert_eq!(
            clamp_into_track(Duration::from_secs(30), 180),
            Duration::from_secs(30)
        );
    }

    /// A duration of zero means the length is unknown, which is common enough:
    /// some servers omit it, and a file with an unreadable header has none.
    /// Clamping against it restarted the track on every seek and on resume.
    #[test]
    fn seeking_a_track_of_unknown_length_is_not_clamped_to_zero() {
        assert_eq!(
            clamp_into_track(Duration::from_secs(42), 0),
            Duration::from_secs(42),
            "an unknown duration must not collapse the seek target"
        );
        assert_eq!(
            clamp_into_track(Duration::from_secs(3600), 0),
            Duration::from_secs(3600)
        );
    }

    /// A one-second track still has a valid position: zero.
    #[test]
    fn a_one_second_track_clamps_to_its_start() {
        assert_eq!(clamp_into_track(Duration::from_secs(5), 1), Duration::ZERO);
    }

    /// The decoder finishes writing long before the device finishes playing:
    /// the ring holds `buffer_seconds` of audio. Advancing at decode-end flushed
    /// that tail, cutting the last seconds off every track.
    #[test]
    fn a_full_buffer_counts_as_still_playing() {
        let capacity = 480_000; // ~5 s of stereo at 48 kHz
        assert!(tail_is_playing(capacity, 0), "completely full");
        assert!(tail_is_playing(capacity, capacity / 2), "half full");
    }

    #[test]
    fn an_empty_buffer_releases_the_advance() {
        let capacity = 480_000;
        assert!(!tail_is_playing(capacity, capacity), "completely drained");
        // Within the slack, so a chunked callback cannot stall the advance.
        assert!(!tail_is_playing(capacity, capacity - capacity / 200));
    }

    #[test]
    fn a_degenerate_buffer_does_not_stall_playback() {
        assert!(!tail_is_playing(0, 0));
        // More free slots than capacity should never underflow into "playing".
        assert!(!tail_is_playing(10, 100));
    }
}
