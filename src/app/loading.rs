use super::layout::*;
use super::types::*;
use super::*;
use crate::subsonic::models::{Album, Artist, Playlist, Song};
use anyhow::Result;

impl App {
    pub fn bootstrap(&mut self) {
        self.load_artists();
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
            LoadEvent::ConnectionTested(result) => self.connection_status = Some(result),
            LoadEvent::LocalScanned { songs, albums } => {
                self.scan_status = Some(format!("{songs} songs, {albums} albums"));
                self.status_message =
                    Some(format!("Local library: {songs} songs in {albums} albums"));
                // The scan may have added or removed tracks, so anything drawn
                // from the old index is now stale.
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
                    // Artwork with no usable colour leaves the preset standing
                    // rather than inventing an accent for it.
                    if palette.is_some() {
                        self.cover_palette = palette;
                        self.refresh_theme();
                    }
                }
            }
            LoadEvent::Lyrics { song_id, lyrics } => {
                // Discard results for a track that is no longer playing.
                if self.lyrics_song.as_deref() == Some(song_id.as_str()) {
                    self.lyrics_cache.put(&song_id, &lyrics);
                    self.lyrics = *lyrics;
                    self.lyrics_pending = false;
                    self.lyrics_scroll = 0.0;
                }
            }
            LoadEvent::ArchiveResults(result) => {
                self.archive_plugin.searching = false;
                match result {
                    Ok(items) => {
                        self.archive_plugin.results = items;
                        self.archive_plugin.selection.reset();
                        // A new result set makes the old track lists dead
                        // weight; dropping them keeps the cache bounded by one
                        // search rather than by the session.
                        self.archive_plugin.files.clear();
                    }
                    Err(err) => {
                        self.status_message = Some(format!("Archive search error: {err}"));
                    }
                }
            }
            LoadEvent::JamendoResults(result) => {
                self.jamendo_plugin.searching = false;
                match result {
                    Ok(tracks) => {
                        self.jamendo_plugin.results = tracks;
                        self.jamendo_plugin.selection.reset();
                    }
                    Err(err) => {
                        self.status_message = Some(format!("Jamendo search error: {err}"));
                    }
                }
            }
            LoadEvent::JamendoDownloadFinished { title, result } => {
                self.jamendo_plugin.working = false;
                match result {
                    Ok(path) => {
                        self.status_message = Some(format!(
                            "Downloaded '{}' to {}",
                            crate::ui::widgets::truncate(&title, 35),
                            path.display()
                        ));
                        self.rescan_local_library();
                    }
                    Err(err) => {
                        self.status_message =
                            Some(format!("Jamendo download failed for '{title}': {err}"));
                    }
                }
            }
            LoadEvent::ArchiveItemFiles { identifier, files } => {
                self.archive_plugin.pending.remove(&identifier);
                self.archive_plugin.files.insert(identifier, files);
            }
            LoadEvent::ArchiveStreamReady(result) => {
                self.archive_plugin.working = false;
                match result {
                    Ok(songs) => {
                        if songs.is_empty() {
                            self.status_message =
                                Some("No playable audio in this Archive item".to_string());
                            return;
                        }
                        let first_title = songs[0].title.clone();
                        let count = songs.len();
                        self.snapshot_queue();
                        self.player.send(PlayerCommand::PlayNow { songs, index: 0 });
                        self.status_message = Some(format!(
                            "Streaming '{}' ({count} track(s) from archive.org)",
                            crate::ui::widgets::truncate(&first_title, 35)
                        ));
                    }
                    Err(err) => {
                        self.status_message = Some(format!("Archive streaming error: {err}"));
                    }
                }
            }
            LoadEvent::ArchiveDownloadFinished { title, result } => {
                self.archive_plugin.working = false;
                match result {
                    Ok(path) => {
                        self.status_message = Some(format!(
                            "Downloaded '{}' to {}",
                            crate::ui::widgets::truncate(&title, 35),
                            path.display()
                        ));
                        self.rescan_local_library();
                    }
                    Err(err) => {
                        self.status_message =
                            Some(format!("Archive download failed for '{title}': {err}"));
                    }
                }
            }
            LoadEvent::PluginStatus(message) => {
                self.status_message = Some(message);
            }
            #[cfg(feature = "nyaa")]
            LoadEvent::NyaaResults(result) => {
                self.nyaa_plugin.searching = false;
                match result {
                    Ok(items) => {
                        self.nyaa_plugin.results = items;
                        self.nyaa_plugin.selection.reset();
                    }
                    Err(err) => {
                        self.status_message = Some(format!("Nyaa search error: {err}"));
                    }
                }
            }
            #[cfg(feature = "nyaa")]
            LoadEvent::NyaaStreamReady(result) => {
                self.nyaa_plugin.downloading = false;
                match result {
                    Ok(songs) => {
                        if songs.is_empty() {
                            self.status_message = Some("No audio tracks found to stream".to_string());
                            return;
                        }
                        let first_title = songs[0].title.clone();
                        let count = songs.len();
                        self.snapshot_queue();
                        self.player.send(PlayerCommand::PlayNow {
                            songs,
                            index: 0,
                        });
                        self.status_message = Some(format!(
                            "Streaming '{}' ({count} track(s) queued in Wander)",
                            crate::ui::widgets::truncate(&first_title, 35)
                        ));
                    }
                    Err(err) => {
                        self.status_message = Some(format!("Streaming error: {err}"));
                    }
                }
            }
            #[cfg(feature = "nyaa")]
            LoadEvent::NyaaDownloadFinished { title, result } => {
                self.nyaa_plugin.downloading = false;
                match result {
                    Ok(path) => {
                        let is_torrent = path.extension().map(|e| e == "torrent").unwrap_or(false);
                        if is_torrent {
                            let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
                            self.status_message = Some(format!(
                                "Downloaded '{}.torrent' to {} (Opened in system client)",
                                crate::ui::widgets::truncate(&title, 30),
                                path.display()
                            ));
                        } else {
                            self.status_message = Some(format!(
                                "Downloaded '{}' to {}",
                                crate::ui::widgets::truncate(&title, 35),
                                path.display()
                            ));
                        }
                        self.rescan_local_library();
                    }
                    Err(err) => {
                        self.status_message = Some(format!("Download failed for '{title}': {err}"));
                    }
                }
            }
            LoadEvent::Error(message) => {
                self.status_message = Some(message);
            }
        }
    }
}
