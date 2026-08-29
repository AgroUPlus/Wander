use super::layout::*;
use super::types::*;
use super::*;
use crate::subsonic::models::Song;
use anyhow::Result;

/// The tail every plugin search shares: clear the spinner, take the results or
/// report the failure. Each plugin owns a different state struct, so this is a
/// macro rather than a method — the shapes match, the types do not.
macro_rules! apply_plugin_search {
    ($app:ident, $plugin:ident, $label:literal, $result:expr, $field:ident $(, $clear:ident)?) => {{
        $app.$plugin.searching = false;
        match $result {
            Ok(items) => {
                $app.$plugin.$field = items;
                $app.$plugin.selection.reset();
                $($app.$plugin.$clear.clear();)?
            }
            Err(err) => {
                $app.push_notification(
                    NotificationLevel::Error,
                    format!(concat!($label, " search error: {}"), err),
                );
            }
        }
    }};
}

impl App {
    /// Every plugin download ends the same way: say what landed where, close the
    /// operation out, and pull the new file into the library — a download is
    /// exactly the moment a new file appears, so it is sent on rather than left
    /// for the next launch to notice.
    fn finish_plugin_download(
        &mut self,
        op_id: &str,
        source: &str,
        title: &str,
        result: Result<std::path::PathBuf, String>,
    ) {
        match result {
            Ok(path) => {
                let short = crate::ui::widgets::truncate(title, 35);
                // A torrent file is a pointer, not the music: hand it to whatever
                // the system uses for those rather than claiming it downloaded.
                let msg = if path.extension().map(|e| e == "torrent").unwrap_or(false) {
                    let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
                    format!(
                        "Downloaded '{short}.torrent' to {} (Opened in system client)",
                        path.display()
                    )
                } else {
                    format!("Downloaded '{short}' to {}", path.display())
                };
                self.push_notification(NotificationLevel::Success, msg);
                self.finish_operation(op_id, OperationStatus::Completed);
                self.rescan_local_library();
                self.sync_library();
            }
            Err(err) => {
                let msg = format!("{source} download failed for '{title}': {err}");
                self.push_notification(NotificationLevel::Error, msg);
                self.finish_operation(op_id, OperationStatus::Failed(err));
            }
        }
    }

    /// Hands a plugin's resolved tracks to the player, keeping the queue that was
    /// there so returning from the detour is possible.
    fn start_plugin_stream(&mut self, source: &str, result: Result<Vec<Song>, String>) {
        match result {
            Ok(songs) => {
                if songs.is_empty() {
                    self.push_notification(
                        NotificationLevel::Warning,
                        format!("No playable audio from {source}"),
                    );
                    return;
                }
                let first_title = songs[0].title.clone();
                let count = songs.len();
                self.snapshot_queue();
                self.player.send(PlayerCommand::PlayNow { songs, index: 0 });
                let msg = format!(
                    "Streaming '{}' ({count} track(s) from {source})",
                    crate::ui::widgets::truncate(&first_title, 35)
                );
                self.push_notification(NotificationLevel::Info, msg);
            }
            Err(err) => {
                self.push_notification(
                    NotificationLevel::Error,
                    format!("{source} streaming error: {err}"),
                );
            }
        }
    }
}

impl App {
    pub fn bootstrap(&mut self) {
        self.load_artists();
        // Before anything else needs them: the mixes below and the Discover shelf both read
        // `stats`, and with centralised statistics on the local file is not where they come from.
        self.refresh_central_stats();
        self.load_albums();
        self.load_playlists();
        // A fresh install has no play history, so Home's mixes fall back to the
        // library's own biggest genres.
        if self.stats.top_genres.is_empty() {
            let library = Arc::clone(&self.library);
            self.spawn_load(async move {
                let mut genres = library.genres().await?;
                genres.sort_by_key(|g| std::cmp::Reverse(g.song_count));
                Ok(LoadEvent::Genres(
                    genres.into_iter().map(|g| g.value).collect(),
                ))
            });
        }
    }

    // ---- async loading -------------------------------------------------

    pub(crate) fn spawn_load<F>(&self, future: F)
    where
        F: std::future::Future<Output = Result<LoadEvent>> + Send + 'static,
    {
        let sender = self.loads.clone();
        tokio::spawn(async move {
            let event = match future.await {
                Ok(event) => event,
                Err(err) => LoadEvent::Error(format!("{err:#}")),
            };
            let _ = sender.send(event);
        });
    }

    pub fn load_artists(&self) {
        let library = Arc::clone(&self.library);
        self.spawn_load(async move { Ok(LoadEvent::Artists(library.artists().await?)) });
    }

    pub fn load_albums(&self) {
        let library = Arc::clone(&self.library);
        self.spawn_load(async move {
            Ok(LoadEvent::Albums(
                library.album_list("alphabeticalByName", 500, 0).await?,
            ))
        });
    }

    /// The flat track list. Capped: a full library can run to six figures, and
    /// a terminal list that long is not useful to scroll.
    pub fn load_tracks(&mut self) {
        self.tracks_loaded = true;
        let library = Arc::clone(&self.library);
        self.spawn_load(async move { Ok(LoadEvent::Tracks(library.all_songs(1000, 0).await?)) });
    }

    pub fn load_favorites(&mut self) {
        self.favorites_loaded = true;
        let library = Arc::clone(&self.library);
        self.spawn_load(async move { Ok(LoadEvent::Favorites(library.starred_songs().await?)) });
    }

    pub fn load_playlists(&self) {
        let library = Arc::clone(&self.library);
        self.spawn_load(async move { Ok(LoadEvent::Playlists(library.playlists().await?)) });
    }

    pub fn load_artist_albums(&self, artist_id: String) {
        let library = Arc::clone(&self.library);
        self.spawn_load(async move {
            let albums = library.artist_albums(&artist_id).await?;
            Ok(LoadEvent::ArtistAlbums { artist_id, albums })
        });
    }

    pub fn load_album_songs(&self, album_id: String) {
        let library = Arc::clone(&self.library);
        self.spawn_load(async move {
            let songs = library.album_songs(&album_id).await?;
            Ok(LoadEvent::AlbumSongs { album_id, songs })
        });
    }

    pub fn load_playlist_songs(&self, playlist_id: String) {
        let library = Arc::clone(&self.library);
        self.spawn_load(async move {
            let songs = library.playlist_songs(&playlist_id).await?;
            Ok(LoadEvent::PlaylistSongs { playlist_id, songs })
        });
    }

    pub fn load_cover(&self, cover_id: String) {
        let library = Arc::clone(&self.library);
        let covers = Arc::clone(&self.covers);
        self.spawn_load(async move {
            let bytes = match covers.get(&cover_id, COVER_SIZE) {
                Some(bytes) => bytes,
                None => {
                    let bytes = library.cover_art(&cover_id, COVER_SIZE).await?;
                    covers.put(&cover_id, COVER_SIZE, &bytes);
                    bytes
                }
            };
            let palette = crate::theme::palette::extract(&bytes);
            Ok(LoadEvent::Cover {
                cover_id,
                bytes,
                palette,
            })
        });
    }

    pub fn apply(&mut self, event: LoadEvent) {
        match event {
            LoadEvent::Artists(artists) => {
                self.artists = artists;
                self.artist_sel.clamp(self.artists.len());
                if let Some(artist) = self.artists.get(self.artist_sel.index) {
                    self.load_artist_albums(artist.id.clone());
                }
            }
            LoadEvent::ArtistAlbums { artist_id, albums } => {
                // Ignore results for an artist the user has already moved off.
                if self.artists.get(self.artist_sel.index).map(|a| &a.id) == Some(&artist_id) {
                    self.artist_albums = albums;
                    self.artist_album_sel.reset();
                    self.artist_songs.clear();
                    if let Some(album) = self.artist_albums.first() {
                        self.load_album_songs(album.id.clone());
                    }
                }
            }
            LoadEvent::AlbumSongs { album_id, songs } => {
                if self
                    .artist_albums
                    .get(self.artist_album_sel.index)
                    .map(|a| &a.id)
                    == Some(&album_id)
                {
                    self.artist_songs = songs.clone();
                    self.artist_song_sel.reset();
                }
                if self.albums.get(self.album_sel.index).map(|a| &a.id) == Some(&album_id) {
                    self.album_songs = songs;
                    self.album_song_sel.reset();
                }
            }
            LoadEvent::Albums(albums) => {
                self.albums = albums;
                self.album_sel.clamp(self.albums.len());
                if let Some(album) = self.albums.first() {
                    self.load_album_songs(album.id.clone());
                }
            }
            LoadEvent::Tracks(songs) => {
                self.tracks = songs;
                self.track_sel.clamp(self.tracks.len());
            }
            LoadEvent::Favorites(songs) => {
                self.favorites = songs;
                self.favorite_sel.clamp(self.favorites.len());
            }
            LoadEvent::Playlists(playlists) => {
                self.playlists = playlists;
                self.playlist_sel.clamp(self.playlists.len());
                if let Some(playlist) = self.playlists.first() {
                    self.load_playlist_songs(playlist.id.clone());
                }
            }
            LoadEvent::PlaylistSongs { playlist_id, songs } => {
                if self.playlists.get(self.playlist_sel.index).map(|p| &p.id) == Some(&playlist_id)
                {
                    self.playlist_songs = songs;
                    self.playlist_song_sel.reset();
                }
            }
            LoadEvent::Genres(genres) => self.library_genres = genres,
            LoadEvent::SyncFinished(result) => self.on_sync_finished(result),
            LoadEvent::SyncOffer(missing) => self.on_sync_offer(missing),
            LoadEvent::SyncFetched(result) => self.on_sync_fetched(result),
            LoadEvent::Reclaimable(tracks) => self.reclaimable = tracks,
            LoadEvent::SyncProgress { fraction, detail } => {
                self.update_operation_progress("library-sync", fraction, Some(detail))
            }
            LoadEvent::ShareDomain(domain) => self.agro_share_domain = domain,
            LoadEvent::AgroStatus(status) => self.agro_status = status,
            LoadEvent::DropArrived {
                from,
                title,
                artist,
            } => {
                self.status_message = Some(if artist.is_empty() {
                    format!("{from} sent you “{title}”")
                } else {
                    format!("{from} sent you “{title}” by {artist}")
                });
                // The message names the drop; this fetches the list behind it, so opening the tab
                // shows the thing that was just announced.
                self.refresh_social();
            }
            LoadEvent::Social {
                friends,
                feed,
                inbox,
                recap,
            } => {
                // Both cursors are clamped for the same reason the jam's is: these lists are other
                // people's, and a friend leaving or a drop being archived elsewhere can shorten
                // them under a selection that then paints nothing and answers no keys.
                self.social_sel = self.social_sel.min(friends.len().saturating_sub(1));
                self.inbox_sel = self.inbox_sel.min(inbox.len().saturating_sub(1));
                self.friends = friends;
                self.social_feed = feed;
                self.inbox = inbox;
                self.recap = recap;
            }
            LoadEvent::Jam(jam) => {
                let ended = jam.is_none() && self.jam.is_some();
                // Keep the cursor inside the queue: other people remove tracks too, and a
                // selection past the end paints nothing and answers no keys.
                let len = jam.as_ref().map(|j| j.queue.len()).unwrap_or(0);
                self.jam_sel = self.jam_sel.min(len.saturating_sub(1));
                self.jam = jam;
                // The room ending under us — the creator left, or it was wound up — hands the
                // borrowed queue back rather than leaving this device on a room that is gone.
                if ended {
                    self.return_queue_after_jam();
                } else {
                    self.follow_jam_now_playing();
                }
            }
            LoadEvent::Stats(stats) => self.stats = stats,
            LoadEvent::ShareCreated(result) => {
                if let Some(Overlay::Share(state)) = self.overlay.as_mut() {
                    state.pending = false;
                    if let Ok(url) = result.as_ref() {
                        copy_to_clipboard(url);
                    }
                    state.result = Some(result);
                }
            }
            LoadEvent::Mix { name, songs } => {
                if songs.is_empty() {
                    self.status_message = Some(format!("{name} found no tracks"));
                } else {
                    let count = songs.len();
                    self.snapshot_queue();
                    self.player.send(PlayerCommand::PlayNow { songs, index: 0 });
                    // Radio mode is what makes a mix endless rather than a
                    // one-shot playlist.
                    if !self.player.queue.lock().unwrap().radio {
                        self.player.send(PlayerCommand::ToggleRadio);
                    }
                    self.status_message = Some(format!("{name}: {count} tracks, radio on"));
                }
            }
            LoadEvent::PaletteSongs { generation, songs } => {
                self.apply_palette_songs(generation, songs)
            }
            // Handed to the renderer, which owns the protocol.
            LoadEvent::CoverResized(response) => self.cover_resized = Some(response),
            LoadEvent::ConnectionTested(result) => {
                self.connection_status = Some(result.clone());
                self.push_notification(NotificationLevel::Info, format!("Server test: {result}"));
                self.finish_operation("server-ping", OperationStatus::Completed);
            }
            LoadEvent::LocalScanned { songs, albums } => {
                self.scan_status = Some(format!("{songs} songs, {albums} albums"));
                let msg = format!("Local library: {songs} songs in {albums} albums");
                self.push_notification(NotificationLevel::Info, msg);
                self.finish_operation("local-scan", OperationStatus::Completed);
                self.invalidate_library();
            }
            LoadEvent::Cover {
                cover_id,
                bytes,
                palette,
            } => {
                if self.cover_id.as_deref() == Some(cover_id.as_str()) {
                    self.cover_bytes = Some(bytes);
                    self.cover_dirty = true;
                    self.cover_generation += 1;
                    if palette.is_some() {
                        self.cover_palette = palette;
                        self.refresh_theme();
                    }
                }
            }
            LoadEvent::Lyrics { song_id, lyrics } => {
                if self.lyrics_song.as_deref() == Some(song_id.as_str()) {
                    self.lyrics_cache.put(&song_id, &lyrics);
                    self.lyrics = *lyrics;
                    self.lyrics_pending = false;
                    self.lyrics_scroll = 0.0;
                }
            }
            LoadEvent::ArchiveResults(result) => {
                apply_plugin_search!(self, archive_plugin, "Archive", result, results, files)
            }
            LoadEvent::JamendoResults(result) => {
                apply_plugin_search!(self, jamendo_plugin, "Jamendo", result, results)
            }
            LoadEvent::JamendoDownloadFinished { title, result } => {
                self.jamendo_plugin.working = false;
                self.finish_plugin_download("jamendo-dl", "Jamendo", &title, result);
            }
            LoadEvent::ArchiveItemFiles { identifier, files } => {
                self.archive_plugin.pending.remove(&identifier);
                self.archive_plugin.files.insert(identifier, files);
            }
            LoadEvent::ArchiveStreamReady(result) => {
                self.archive_plugin.working = false;
                self.start_plugin_stream("archive.org", result);
            }
            LoadEvent::ArchiveDownloadFinished { title, result } => {
                self.archive_plugin.working = false;
                self.finish_plugin_download("archive-dl", "Archive", &title, result);
            }
            LoadEvent::PluginStatus(message) => {
                self.status_message = Some(message.clone());
                // Update running operation progress details if any exist
                if let Some(op) = self
                    .operations
                    .iter_mut()
                    .find(|o| o.status == OperationStatus::Running)
                {
                    op.details = Some(message);
                }
            }
            #[cfg(feature = "nyaa")]
            LoadEvent::NyaaResults(result) => {
                apply_plugin_search!(self, nyaa_plugin, "Nyaa", result, results)
            }
            #[cfg(feature = "nyaa")]
            LoadEvent::NyaaStreamReady(result) => {
                self.nyaa_plugin.downloading = false;
                self.start_plugin_stream("Nyaa", result);
            }
            #[cfg(feature = "nyaa")]
            LoadEvent::NyaaDownloadFinished { title, result } => {
                self.nyaa_plugin.downloading = false;
                self.finish_plugin_download("nyaa-dl", "Nyaa", &title, result);
            }
            LoadEvent::Error(message) => {
                self.push_notification(NotificationLevel::Error, message);
            }
        }
    }
}
