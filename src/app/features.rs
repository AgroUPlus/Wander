use super::types::*;
use super::*;
use crate::subsonic::models::Song;

/// The local value has to win: `setRating` is fire-and-forget, and the server's
/// copy of the song is not refetched until the list it came from reloads, so
/// without this the stars would flick back to the old value.
pub(crate) fn merge_rating(local: Option<u8>, server: Option<u8>) -> u8 {
    local.or(server).unwrap_or(0).min(5)
}

impl App {
    ///
    /// Negative results are cached too, so a track without lyrics is not
    /// re-requested every time it plays. If no local/server lyrics exist,
    /// queries LRCLIB online if enabled.
    pub(crate) fn sync_lyrics(&mut self, song: Option<&crate::subsonic::models::Song>) {
        let song_id = song.map(|s| s.id.clone());
        if self.lyrics_song == song_id {
            return;
        }
        self.lyrics_song = song_id.clone();
        self.lyrics = Default::default();
        self.lyrics_scroll = 0.0;

        let Some(song) = song.cloned() else {
            self.lyrics_pending = false;
            return;
        };

        self.lyrics_pending = true;
        let song_id = song.id.clone();
        let library = Arc::clone(&self.library);
        let cache = Arc::clone(&self.lyrics_cache);
        let http = self.http.clone();
        let lyrics_config = self.config.lyrics.clone();

        self.spawn_load(async move {
            if let Some(cached) = cache.get(&song_id)
                && (!cached.is_empty() || !lyrics_config.fetch_online)
            {
                return Ok(LoadEvent::Lyrics {
                    song_id,
                    lyrics: Box::new(cached),
                });
            }

            let mut lyrics = library.lyrics(&song_id).await.unwrap_or_default();
            if lyrics.is_empty()
                && lyrics_config.fetch_online
                && let Some(online) = crate::subsonic::online_lyrics::fetch_online_lyrics(
                    &http,
                    &lyrics_config,
                    &song,
                )
                .await
            {
                lyrics = online;
            }

            Ok(LoadEvent::Lyrics {
                song_id,
                lyrics: Box::new(lyrics),
            })
        });
    }

    /// Read the next variant a track offers: another language, a romanisation,
    /// or a translation fetched earlier.
    pub(crate) fn cycle_lyric_variant(&mut self) {
        self.status_message = match self.lyrics.cycle() {
            Some(label) => {
                // The new variant may be a different length, so a scroll
                // position measured in lines no longer means the same thing.
                self.lyrics_scroll = 0.0;
                Some(format!("Lyrics: {label}"))
            }
            None if self.lyrics.is_empty() => Some("No lyrics to switch between".to_string()),
            None => Some("This track only has one version of its lyrics".to_string()),
        };
    }

    /// Send the current lyrics to the configured translation endpoint.
    ///
    /// Deliberately manual: this is the one action in wander that hands the
    /// user's listening to a third party, so it happens on a keypress and only
    /// when they have named the endpoint themselves.
    pub(crate) fn translate_lyrics(&mut self) {
        if !self.config.lyrics.translation_enabled() {
            self.status_message =
                Some("Set [lyrics] translate_url in config.toml to enable translation".to_string());
            return;
        }
        if self.lyrics.is_empty() {
            self.status_message = Some("No lyrics to translate".to_string());
            return;
        }

        let target = self.config.lyrics.translate_to.trim().to_string();
        // Already fetched once: switch to it rather than asking again.
        if let Some(index) = self
            .lyrics
            .variants
            .iter()
            .position(|variant| variant.lang.as_deref() == Some(&format!("{target} (machine)")))
        {
            self.lyrics.active = index;
            self.lyrics_scroll = 0.0;
            self.status_message = Some(format!("Lyrics: {target} (machine)"));
            return;
        }

        let Some(song_id) = self.lyrics_song.clone() else {
            return;
        };
        let source = self.lyrics.active().clone();
        let mut set = self.lyrics.clone();
        let config = self.config.lyrics.clone();
        let http = self.http.clone();
        let cache = Arc::clone(&self.lyrics_cache);

        self.status_message = Some("Translating…".to_string());
        self.spawn_load(async move {
            match crate::subsonic::translate::translate(&http, &config, &source).await {
                Ok(translated) => {
                    set.push_active(translated);
                    // Cached with the rest of the track's variants, so this
                    // costs one request per track ever rather than per play.
                    cache.put(&song_id, &set);
                    Ok(LoadEvent::Lyrics {
                        song_id,
                        lyrics: Box::new(set),
                    })
                }
                Err(error) => Ok(LoadEvent::Error(format!("Translation failed: {error}"))),
            }
        });
    }

    /// Manually fetch lyrics online (LRCLIB) for the playing track.
    pub(crate) fn fetch_online_lyrics_action(&mut self) {
        let Some(song) = self.player.status().current else {
            self.status_message = Some("No track currently playing".to_string());
            return;
        };

        let song_id = song.id.clone();
        let http = self.http.clone();
        let config = self.config.lyrics.clone();
        let mut set = self.lyrics.clone();

        self.status_message = Some("Searching online lyrics (LRCLIB)…".to_string());
        self.lyrics_pending = true;

        self.spawn_load(async move {
            if let Some(online) =
                crate::subsonic::online_lyrics::fetch_online_lyrics(&http, &config, &song).await
            {
                if set.is_empty() {
                    set = online;
                } else {
                    for variant in online.variants {
                        set.push_active(variant);
                    }
                }
                Ok(LoadEvent::Lyrics {
                    song_id,
                    lyrics: Box::new(set),
                })
            } else {
                Ok(LoadEvent::Error(
                    "No online lyrics found on LRCLIB".to_string(),
                ))
            }
        });
    }

    /// Keep the cover and lyrics in sync with the playing track.
    pub fn sync_cover(&mut self) {
        let current = self.player.status().current;
        self.sync_lyrics(current.as_ref());

        let wanted = current.and_then(|song| song.cover_art.clone());
        if wanted != self.cover_id {
            self.cover_id = wanted.clone();
            self.cover_bytes = None;
            self.cover_dirty = true;
            self.cover_generation += 1;
            if let Some(cover_id) = wanted {
                // The old tint stays up until the new one arrives. Clearing it
                // here instead would flash the preset colours for a frame on
                // every single track change.
                self.load_cover(cover_id);
            } else {
                self.cover_palette = None;
                self.refresh_theme();
            }
            self.prefetch_next_cover();
            // A track change is exactly when the play log gains a line. Only
            // the appended bytes are read, so this stays cheap as the log grows.
            let (new_records, offset) = crate::history::load_since(self.history_bytes);
            if !new_records.is_empty() || offset != self.history_bytes {
                self.history_bytes = offset;
                self.history.extend(new_records);
                self.stats = crate::history::stats(&self.history, 5);
            }
        }
    }

    /// Rebuild the drawn theme from the configured preset and the current
    /// artwork.
    ///
    /// Called whenever either side changes: a new cover, or a new preset.
    pub fn refresh_theme(&mut self) {
        self.theme = match &self.cover_palette {
            Some(palette) => self.config.theme.tinted(palette),
            None => self.config.theme.clone(),
        };
    }

    /// Fetch the next queued track's cover ahead of time so track changes are
    /// visually instant rather than showing a placeholder.
    pub(crate) fn prefetch_next_cover(&self) {
        let next = {
            let queue = self.player.queue.lock().unwrap();
            queue.peek_next().and_then(|song| song.cover_art.clone())
        };
        if let Some(cover_id) = next
            && self.covers.get(&cover_id, COVER_SIZE).is_none()
        {
            let library = Arc::clone(&self.library);
            let covers = Arc::clone(&self.covers);
            tokio::spawn(async move {
                if let Ok(bytes) = library.cover_art(&cover_id, COVER_SIZE).await {
                    covers.put(&cover_id, COVER_SIZE, &bytes);
                }
            });
        }
    }

    // ---- input ---------------------------------------------------------

    /// This session's rating for a song, falling back to the server's.
    pub fn rating_of(&self, song: &Song) -> u8 {
        merge_rating(self.ratings.get(&song.id).copied(), song.user_rating)
    }

    /// Render a rating as stars, or nothing at all when it is unset.
    pub fn rating_stars(&self, song: &Song) -> String {
        let rating = self.rating_of(song);
        if rating == 0 {
            return String::new();
        }
        let star = self.config.glyphs.icon(crate::ui::glyphs::Icon::Star);
        star.repeat(rating as usize)
    }

    /// Nudge the target track's rating, starting from whatever it already is.
    pub(crate) fn adjust_rating(&mut self, delta: i8) {
        let Some(song) = self.target_songs().into_iter().next() else {
            return;
        };
        let rating = (self.rating_of(&song) as i8 + delta).clamp(0, 5) as u8;
        self.ratings.insert(song.id.clone(), rating);
        self.status_message = Some(if rating == 0 {
            format!("Cleared rating for {}", song.title)
        } else {
            format!(
                "{} {}",
                self.config
                    .glyphs
                    .icon(crate::ui::glyphs::Icon::Star)
                    .repeat(rating as usize),
                song.title
            )
        });
        let library = Arc::clone(&self.library);
        let id = song.id.clone();
        tokio::spawn(async move {
            let _ = library.set_rating(&id, rating).await;
        });
    }

    pub(crate) fn move_queued_track(&mut self, delta: isize) {
        if self.tab != Tab::Queue && self.focus != Pane::Queue {
            return;
        }
        let index = self.queue_sel.index;
        let moved = self.player.queue.lock().unwrap().move_song(index, delta);
        // Follow the track, so repeated presses keep moving the same one.
        self.queue_sel.index = moved;
        self.save_queue_state();
    }

    /// Start a Home mix: seed the queue and let radio mode carry it onward.
    pub(crate) fn start_mix(&mut self, index: usize) {
        use crate::ui::home::MixKind;
        let mixes = crate::ui::home::mixes(self);
        let Some(mix) = mixes.get(index).cloned() else {
            return;
        };
        self.status_message = Some(format!("Starting {}…", mix.name));

        let library = Arc::clone(&self.library);
        // Familiar artists get de-emphasised for Discover, so it can surprise.
        let known: std::collections::HashSet<String> = self
            .stats
            .top_artists
            .iter()
            .map(|(a, _)| a.to_lowercase())
            .collect();
        self.spawn_load(async move {
            let songs = match mix.kind {
                MixKind::Genre(genre) => library.songs_by_genre(&genre, 100, 0).await?,
                MixKind::Favorites => library.starred_songs().await?,
                MixKind::Discover => library
                    .random_songs(200, None)
                    .await?
                    .into_iter()
                    .filter(|song| {
                        song.play_count == 0
                            && !known.contains(&song.artist_or_unknown().to_lowercase())
                    })
                    .collect(),
                MixKind::Surprise => library.random_songs(100, None).await?,
            };
            Ok(LoadEvent::Mix {
                name: mix.name,
                songs,
            })
        });
    }

    // ---- Plugin jobs ----------------------------------------------------

    /// Stop whatever a plugin is currently fetching.
    pub(crate) fn cancel_plugin_job(&mut self) {
        if let Some(job) = self.plugin_job.take() {
            job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            job.handle.abort();
        }
    }

    /// Run a plugin fetch, cancelling any fetch already in flight.
    pub(crate) fn start_plugin_job<F>(
        &mut self,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
        future: F,
    ) where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.cancel_plugin_job();
        self.plugin_job = Some(PluginJob {
            handle: tokio::spawn(future),
            cancel,
        });
    }

    // ---- Online source switching ---------------------------------------

    /// Move to the next enabled online plugin. A no-op when only one is on.
    pub(crate) fn cycle_online_source(&mut self) {
        let sources = OnlineSource::available(&self.config);
        if sources.len() < 2 {
            self.status_message =
                Some("Only one online plugin is enabled (see Settings ▸ Plugins)".to_string());
            return;
        }
        let current = sources
            .iter()
            .position(|s| *s == self.online_source)
            .unwrap_or(0);
        self.set_online_source(sources[(current + 1) % sources.len()]);
    }

    /// Put the cursor in the active plugin's search box.
    pub(crate) fn focus_online_search(&mut self) {
        match self.online_source {
            #[cfg(feature = "nyaa")]
            OnlineSource::Nyaa => self.nyaa_plugin.editing_query = true,
            OnlineSource::Archive => self.archive_plugin.editing_query = true,
            OnlineSource::Jamendo => self.jamendo_plugin.editing_query = true,
        }
    }

    /// Step the active plugin's filter — the box each one puts beside its
    /// search field. The nyaa box has always invited a click; now there is
    /// something behind it.
    pub(crate) fn cycle_online_filter(&mut self) {
        match self.online_source {
            #[cfg(feature = "nyaa")]
            OnlineSource::Nyaa => self.cycle_nyaa_category(),
            OnlineSource::Archive => self.cycle_archive_collection(),
            OnlineSource::Jamendo => self.cycle_jamendo_format(),
        }
    }

    /// Show a particular online plugin in the Online tab.
    pub(crate) fn set_online_source(&mut self, source: OnlineSource) {
        self.online_source = source;
        // The plugins share Pane::Online, so a stale cursor from the previous
        // source would point past the end of this one's results.
        let len = self.pane_len(Pane::Online);
        self.selection_mut(Pane::Online).clamp(len);
        self.status_message = Some(format!("Online source: {}", source.title()));
    }

    // ---- Internet Archive Plugin ---------------------------------------

    pub(crate) fn search_archive(&mut self, query: String) {
        if query.trim().is_empty() {
            self.status_message = Some("Enter a search query first".to_string());
            return;
        }
        self.archive_plugin.query = query.clone();
        self.archive_plugin.searching = true;
        self.status_message = Some(format!("Searching archive.org for '{}'...", query));

        let http = self.http.clone();
        let collection = crate::plugins::archive::ArchiveCollection::from_code(
            &self.config.plugins.archive.collection,
        );
        let loads = self.loads.clone();

        tokio::spawn(async move {
            let res = crate::plugins::archive::api::search_archive(&http, &query, collection)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = loads.send(LoadEvent::ArchiveResults(res));
        });
    }

    /// Fetch the highlighted item's metadata, once, in the background.
    ///
    /// Called every frame from the main loop: the length column has to fill in
    /// as the cursor moves, and there is no single place a selection changes
    /// (keys, mouse clicks and a fresh search all move it). The cache and the
    /// in-flight set keep this to one request per item ever, and the result is
    /// what streaming and downloading use, so it is never wasted work.
    pub fn sync_archive_metadata(&mut self) {
        if self.tab != Tab::Online || self.online_source != OnlineSource::Archive {
            return;
        }
        let Some(identifier) = self
            .archive_plugin
            .selected_item()
            .map(|item| item.identifier.clone())
        else {
            return;
        };
        if self.archive_plugin.files.contains_key(&identifier)
            || self.archive_plugin.pending.contains(&identifier)
        {
            return;
        }

        self.archive_plugin.pending.insert(identifier.clone());
        let http = self.http.clone();
        let loads = self.loads.clone();
        tokio::spawn(async move {
            let files = crate::plugins::archive::api::item_files(&http, &identifier)
                .await
                .ok();
            let _ = loads.send(LoadEvent::ArchiveItemFiles { identifier, files });
        });
    }

    pub(crate) fn cycle_archive_collection(&mut self) {
        use crate::plugins::archive::ArchiveCollection;
        let collections = ArchiveCollection::ALL;
        let current = ArchiveCollection::from_code(&self.config.plugins.archive.collection);
        let idx = collections.iter().position(|c| *c == current).unwrap_or(0);
        let next = collections[(idx + 1) % collections.len()];
        self.config.plugins.archive.collection = next.code().to_string();
        let _ = self.config.save();
        self.status_message = Some(format!("Archive collection set to: {}", next.label()));

        if !self.archive_plugin.query.is_empty() {
            let query = self.archive_plugin.query.clone();
            self.search_archive(query);
        }
    }

    /// Queue an Archive item's tracks and start playing.
    ///
    /// Archive serves plain range-capable HTTPS, so the player streams the
    /// files straight from the URL — nothing is written to disk.
    pub(crate) fn stream_selected_archive_item(&mut self) {
        let Some(item) = self.archive_plugin.selected_item().cloned() else {
            self.status_message = Some("No item selected to stream".to_string());
            return;
        };

        self.archive_plugin.working = true;
        self.status_message = Some(format!(
            "Resolving tracks for '{}'...",
            crate::ui::widgets::truncate(&item.title, 35)
        ));

        let http = self.http.clone();
        let loads = self.loads.clone();
        // Usually already fetched for the length column, so Enter plays
        // without a round trip.
        let cached = self
            .archive_plugin
            .files
            .get(&item.identifier)
            .cloned()
            .flatten();

        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.start_plugin_job(cancel, async move {
            let (files, cover_art) = match cached {
                Some(files) => (files, None),
                None => {
                    match crate::plugins::archive::api::item_files_and_cover(&http, &item.identifier)
                        .await
                    {
                        Ok(res) => res,
                        Err(err) => {
                            let _ = loads.send(LoadEvent::ArchiveStreamReady(Err(format!("{err:#}"))));
                            return;
                        }
                    }
                }
            };

            let songs: Vec<Song> = files
                .iter()
                .enumerate()
                .map(|(idx, file)| Song {
                    id: file.stream_url(&item.identifier),
                    title: file.title.clone(),
                    album: Some(item.title.clone()),
                    album_id: None,
                    artist: Some(if item.creator.is_empty() {
                        "Internet Archive".to_string()
                    } else {
                        item.creator.clone()
                    }),
                    artist_id: None,
                    // Use real uploaded cover art if available, otherwise None
                    // so the UI renders a clean placeholder instead of waveform lines.
                    cover_art: cover_art.clone(),
                    duration: file.duration as u32,
                    bit_rate: 0,
                    track: Some(file.track.unwrap_or((idx + 1) as u32)),
                    year: item.year.parse().ok(),
                    genre: None,
                    suffix: file.suffix(),
                    content_type: None,
                    size: file.size,
                    starred: None,
                    user_rating: None,
                    play_count: 0,
                    genres: Vec::new(),
                    moods: Vec::new(),
                })
                .collect();

            let _ = loads.send(LoadEvent::ArchiveStreamReady(Ok(songs)));
        });
    }

    pub(crate) fn download_selected_archive_item(&mut self) {
        let Some(item) = self.archive_plugin.selected_item().cloned() else {
            self.status_message = Some("No item selected to download".to_string());
            return;
        };

        let target_dir = self.online_download_dir(
            self.config.plugins.archive.download_dir.clone(),
        );

        self.archive_plugin.working = true;
        let title_short = crate::ui::widgets::truncate(&item.title, 35);
        self.push_notification(NotificationLevel::Info, format!("Started downloading '{title_short}'"));
        self.add_operation(Operation {
            id: "archive-dl".into(),
            title: item.title.clone(),
            kind: OperationKind::Download,
            progress: None,
            status: OperationStatus::Running,
            details: Some("Downloading from archive.org...".into()),
            started_at: std::time::Instant::now(),
        });

        let http = self.http.clone();
        let loads = self.loads.clone();
        let cached = self
            .archive_plugin
            .files
            .get(&item.identifier)
            .cloned()
            .flatten();

        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.start_plugin_job(cancel, async move {
            let result = async {
                let files = match cached {
                    Some(files) => files,
                    None => crate::plugins::archive::api::item_files(&http, &item.identifier).await?,
                };
                crate::plugins::archive::downloader::download_archive_item(
                    &http,
                    &item,
                    &files,
                    &target_dir,
                    |_, _, _| {},
                )
                .await
            }
            .await
            .map_err(|e| format!("{e:#}"));

            let _ = loads.send(LoadEvent::ArchiveDownloadFinished {
                title: item.title,
                result,
            });
        });
    }

    /// Where an online plugin saves files: its own setting, else the first
    /// local library root (so downloads show up in the library), else Music.
    pub(crate) fn online_download_dir(
        &self,
        configured: Option<std::path::PathBuf>,
    ) -> std::path::PathBuf {
        if let Some(dir) = configured {
            return dir;
        }
        if let Some(first_local) = self.config.local.paths.first() {
            return first_local.clone();
        }
        directories::UserDirs::new()
            .and_then(|ud| ud.audio_dir().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::path::PathBuf::from("./downloads"))
    }

    // ---- Jamendo Plugin -------------------------------------------------

    pub(crate) fn search_jamendo(&mut self, query: String) {
        if query.trim().is_empty() {
            self.status_message = Some("Enter a search query first".to_string());
            return;
        }
        self.jamendo_plugin.query = query.clone();
        self.jamendo_plugin.searching = true;
        self.status_message = Some(format!("Searching Jamendo for '{}'...", query));

        let http = self.http.clone();
        let loads = self.loads.clone();
        let client_id = self.config.plugins.jamendo.client_id.clone();
        let format =
            crate::plugins::jamendo::JamendoFormat::from_code(&self.config.plugins.jamendo.format);

        tokio::spawn(async move {
            let res =
                crate::plugins::jamendo::api::search_jamendo(&http, &client_id, &query, format)
                    .await
                    .map_err(|e| format!("{e:#}"));
            let _ = loads.send(LoadEvent::JamendoResults(res));
        });
    }

    pub(crate) fn cycle_jamendo_format(&mut self) {
        use crate::plugins::jamendo::JamendoFormat;
        let formats = JamendoFormat::ALL;
        let current = JamendoFormat::from_code(&self.config.plugins.jamendo.format);
        let idx = formats.iter().position(|f| *f == current).unwrap_or(0);
        let next = formats[(idx + 1) % formats.len()];
        self.config.plugins.jamendo.format = next.code().to_string();
        let _ = self.config.save();
        self.status_message = Some(format!("Jamendo format set to: {}", next.label()));

        // The stream URLs embed the format, so old results point at the old one.
        if !self.jamendo_plugin.query.is_empty() {
            let query = self.jamendo_plugin.query.clone();
            self.search_jamendo(query);
        }
    }

    /// Convert one search result into a queueable track.
    fn jamendo_song(&self, track: &crate::plugins::jamendo::JamendoTrack) -> Song {
        let format =
            crate::plugins::jamendo::JamendoFormat::from_code(&self.config.plugins.jamendo.format);
        Song {
            id: track.audio.clone(),
            title: track.name.clone(),
            album: (!track.album_name.is_empty()).then(|| track.album_name.clone()),
            album_id: None,
            artist: Some(track.artist_name.clone()),
            artist_id: None,
            cover_art: track.image.clone(),
            duration: track.duration,
            bit_rate: 0,
            track: None,
            year: track.release_year,
            genre: None,
            suffix: Some(format.suffix().to_string()),
            content_type: None,
            size: 0,
            starred: None,
            user_rating: None,
            play_count: 0,
            genres: Vec::new(),
            moods: Vec::new(),
        }
    }

    /// Play the highlighted track, and queue the rest of the results behind it.
    ///
    /// A search returns a whole listful of songs; queueing only the one that
    /// was highlighted would throw the other ninety-nine away.
    pub(crate) fn stream_selected_jamendo_track(&mut self) {
        if self.jamendo_plugin.results.is_empty() {
            self.status_message = Some("No track selected to play".to_string());
            return;
        }

        let index = self
            .jamendo_plugin
            .selection
            .index
            .min(self.jamendo_plugin.results.len() - 1);
        let songs: Vec<Song> = self
            .jamendo_plugin
            .results
            .iter()
            .map(|track| self.jamendo_song(track))
            .collect();
        let title = songs[index].title.clone();
        let count = songs.len();

        self.snapshot_queue();
        self.player.send(PlayerCommand::PlayNow { songs, index });
        self.status_message = Some(format!(
            "Playing '{}' ({count} track(s) queued from Jamendo)",
            crate::ui::widgets::truncate(&title, 35)
        ));
    }

    pub(crate) fn download_selected_jamendo_track(&mut self) {
        let Some(track) = self.jamendo_plugin.selected_track().cloned() else {
            self.status_message = Some("No track selected to download".to_string());
            return;
        };
        let Some(url) = track.audiodownload.clone() else {
            self.status_message =
                Some("This artist does not allow downloads of this track".to_string());
            return;
        };

        let target_dir =
            self.online_download_dir(self.config.plugins.jamendo.download_dir.clone());
        let format =
            crate::plugins::jamendo::JamendoFormat::from_code(&self.config.plugins.jamendo.format);
        let file_name = format!(
            "{} - {}.{}",
            crate::plugins::sanitize_filename(&track.artist_name),
            crate::plugins::sanitize_filename(&track.name),
            format.suffix()
        );

        self.jamendo_plugin.working = true;
        let title_short = crate::ui::widgets::truncate(&track.name, 35);
        self.push_notification(NotificationLevel::Info, format!("Started downloading '{title_short}'"));
        self.add_operation(Operation {
            id: "jamendo-dl".into(),
            title: track.name.clone(),
            kind: OperationKind::Download,
            progress: None,
            status: OperationStatus::Running,
            details: Some("Downloading from Jamendo...".into()),
            started_at: std::time::Instant::now(),
        });

        let http = self.http.clone();
        let loads = self.loads.clone();
        let title = track.name.clone();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        self.start_plugin_job(cancel, async move {
            let result = async {
                tokio::fs::create_dir_all(&target_dir).await?;
                let response = http.get(&url).send().await?;
                if !response.status().is_success() {
                    anyhow::bail!("Jamendo returned HTTP {}", response.status());
                }
                let bytes = response.bytes().await?;
                let target = target_dir.join(&file_name);
                tokio::fs::write(&target, &bytes).await?;
                Ok::<std::path::PathBuf, anyhow::Error>(target)
            }
            .await
            .map_err(|e| format!("{e:#}"));

            let _ = loads.send(LoadEvent::JamendoDownloadFinished { title, result });
        });
    }

    // ---- Nyaa Online Plugin --------------------------------------------

    #[cfg(feature = "nyaa")]
    pub(crate) fn search_nyaa(&mut self, query: String) {
        if query.trim().is_empty() {
            self.status_message = Some("Enter a search query first".to_string());
            return;
        }
        self.nyaa_plugin.query = query.clone();
        self.nyaa_plugin.searching = true;
        self.status_message = Some(format!("Searching nyaa.si for '{}'...", query));

        let http = self.http.clone();
        let category = self.config.plugins.nyaa.category.clone();
        let loads = self.loads.clone();

        tokio::spawn(async move {
            let res = crate::plugins::nyaa::api::search_nyaa(&http, &query, &category)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = loads.send(LoadEvent::NyaaResults(res));
        });
    }

    #[cfg(feature = "nyaa")]
    pub(crate) fn cycle_nyaa_category(&mut self) {
        use crate::plugins::nyaa::api::NyaaCategory;
        let categories = NyaaCategory::ALL;
        let current_code = &self.config.plugins.nyaa.category;
        let current_idx = categories
            .iter()
            .position(|c| c.code() == current_code)
            .unwrap_or(0);
        let next_idx = (current_idx + 1) % categories.len();
        let next_cat = categories[next_idx];
        self.config.plugins.nyaa.category = next_cat.code().to_string();
        let _ = self.config.save();
        self.status_message = Some(format!("Nyaa category set to: {}", next_cat.label()));

        if !self.nyaa_plugin.query.is_empty() {
            let query = self.nyaa_plugin.query.clone();
            self.search_nyaa(query);
        }
    }

    #[cfg(feature = "nyaa")]
    pub(crate) fn download_selected_nyaa_item(&mut self) {
        let Some(item) = self.nyaa_plugin.selected_item().cloned() else {
            self.status_message = Some("No item selected to download".to_string());
            return;
        };

        let target_dir = if let Some(ref dir) = self.config.plugins.nyaa.download_dir {
            dir.clone()
        } else if let Some(first_local) = self.config.local.paths.first() {
            first_local.clone()
        } else {
            let user_dirs = directories::UserDirs::new();
            if let Some(ud) = user_dirs {
                ud.audio_dir()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| std::path::PathBuf::from("./downloads"))
            } else {
                std::path::PathBuf::from("./downloads")
            }
        };

        self.nyaa_plugin.downloading = true;
        let title_short = crate::ui::widgets::truncate(&item.title, 35);
        self.push_notification(NotificationLevel::Info, format!("Started downloading '{title_short}'"));
        self.add_operation(Operation {
            id: "nyaa-dl".into(),
            title: item.title.clone(),
            kind: OperationKind::Download,
            progress: None,
            status: OperationStatus::Running,
            details: Some("Downloading torrent / file...".into()),
            started_at: std::time::Instant::now(),
        });

        let http = self.http.clone();
        let loads = self.loads.clone();

        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.start_plugin_job(cancel, async move {
            let res = crate::plugins::nyaa::downloader::download_nyaa_item(
                &http,
                &item,
                &target_dir,
            )
            .await
            .map_err(|e| format!("{e:#}"));

            let _ = loads.send(LoadEvent::NyaaDownloadFinished {
                title: item.title,
                result: res,
            });
        });
    }

    #[cfg(feature = "nyaa")]
    pub(crate) fn stream_selected_nyaa_item(&mut self) {
        let Some(item) = self.nyaa_plugin.selected_item().cloned() else {
            self.status_message = Some("No item selected to stream".to_string());
            return;
        };

        self.nyaa_plugin.downloading = true;
        self.status_message = Some(format!(
            "Buffering/Streaming '{}'...",
            crate::ui::widgets::truncate(&item.title, 35)
        ));

        let http = self.http.clone();
        let loads = self.loads.clone();

        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel_job = std::sync::Arc::clone(&cancel);
        self.start_plugin_job(cancel_job, async move {
            let cache_dir = crate::paths::cache_dir()
                .map(|p| p.join("stream_cache"))
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp/wander_stream"));

            let download_res = crate::plugins::nyaa::downloader::download_nyaa_item(
                &http,
                &item,
                &cache_dir,
            )
            .await;

            match download_res {
                Ok(path) => {
                    let is_torrent = path.extension().map(|e| e == "torrent").unwrap_or(false);
                    let audio_paths = if is_torrent {
                        let progress = loads.clone();
                        let title = item.title.clone();
                        let report = move |bytes: u64| {
                            let _ = progress.send(LoadEvent::PluginStatus(format!(
                                "Downloading '{}' — {:.1} MB so far...",
                                crate::ui::widgets::truncate(&title, 30),
                                bytes as f64 / (1024.0 * 1024.0)
                            )));
                        };
                        match crate::plugins::nyaa::downloader::extract_torrent_audio(
                            &path,
                            &cache_dir,
                            &cancel,
                            report,
                        )
                        .await
                        {
                            Ok(paths) => paths,
                            Err(e) => {
                                let _ = loads.send(LoadEvent::NyaaStreamReady(Err(format!("{e:#}"))));
                                return;
                            }
                        }
                    } else {
                        vec![path]
                    };

                    // The files are already on disk, so their own tags are
                    // the best source for the length and the track names —
                    // a torrent's filenames rarely are.
                    //
                    // Parsing them is blocking work on a two-worker runtime
                    // that also drives the UI, so it goes to a blocking thread.
                    let probe_paths = audio_paths.clone();
                    let probed: Vec<crate::plugins::ProbedTags> =
                        tokio::task::spawn_blocking(move || {
                            probe_paths
                                .iter()
                                .map(|p| crate::plugins::probe_audio(p))
                                .collect()
                        })
                        .await
                        .unwrap_or_default();
                    let songs: Vec<Song> = audio_paths
                        .into_iter()
                        .enumerate()
                        .map(|(idx, p)| {
                            let probed = probed.get(idx).cloned().unwrap_or_default();
                            let track_title = probed.title.clone().unwrap_or_else(|| {
                                p.file_stem()
                                    .map(|s| s.to_string_lossy().to_string())
                                    .unwrap_or_else(|| format!("Track {}", idx + 1))
                            });
                            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);

                            Song {
                                // Cached under the plugin's own prefix, not
                                // "local:": it plays off disk, but it is not
                                // part of the user's library.
                                id: format!(
                                    "{}{}",
                                    crate::library::ONLINE_PREFIX,
                                    p.display()
                                ),
                                title: track_title,
                                album: Some(probed.album.clone().unwrap_or_else(|| item.title.clone())),
                                album_id: None,
                                artist: Some(
                                    probed.artist.clone().unwrap_or_else(|| "Nyaa.si".to_string()),
                                ),
                                artist_id: None,
                                // Points at the cached file itself, which is
                                // where its embedded artwork lives.
                                cover_art: Some(format!(
                                    "{}{}",
                                    crate::library::ONLINE_PREFIX,
                                    p.display()
                                )),
                                duration: probed.duration,
                                bit_rate: probed.bit_rate,
                                track: Some(probed.track.unwrap_or((idx + 1) as u32)),
                                year: probed.year,
                                genre: Some("Anime / OST".to_string()),
                                suffix: p.extension().map(|e| e.to_string_lossy().to_string()),
                                content_type: None,
                                size,
                                starred: None,
                                user_rating: None,
                                play_count: 0,
                                genres: Vec::new(),
                                moods: Vec::new(),
                            }
                        })
                        .collect();

                    let _ = loads.send(LoadEvent::NyaaStreamReady(Ok(songs)));
                }
                Err(err) => {
                    let _ = loads.send(LoadEvent::NyaaStreamReady(Err(format!("{err:#}"))));
                }
            }
        });
    }
}
