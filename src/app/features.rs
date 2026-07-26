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
}
