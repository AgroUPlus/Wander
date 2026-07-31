use super::types::*;
use super::*;
use crate::subsonic::models::Song;
use crossterm::event::KeyEvent;

impl App {
    pub fn open_share(&mut self) {
        let songs = self.target_songs();
        if songs.is_empty() {
            self.status_message = Some("Nothing to share".to_string());
            return;
        }
        // Say why up front rather than opening a dialog that can only fail:
        // only the server can mint a share URL, so a local file or a track a
        // plugin pulled off the internet has nothing to share.
        if let Some(song) = songs
            .iter()
            .find(|song| crate::library::SongSource::of(&song.id) != crate::library::SongSource::Server)
        {
            let source = crate::library::SongSource::of(&song.id);
            self.status_message = Some(format!(
                "{} tracks cannot be shared — there is no server to serve them",
                source.label()
            ));
            return;
        }
        if !self.library.capabilities().share {
            self.status_message = Some("Sharing needs a configured server".to_string());
            return;
        }
        self.overlay = Some(Overlay::Share(ShareState::new(songs)));
    }

    pub fn open_playlist_picker(&mut self) {
        let songs = self.target_songs();
        if songs.is_empty() {
            self.status_message = Some("Nothing to add".to_string());
            return;
        }
        self.overlay = Some(Overlay::Playlist(PlaylistState::new(
            songs,
            self.playlists.clone(),
        )));
    }

    /// Route a key to the open overlay. Returns false when no overlay is open,
    /// so the caller falls through to the normal keymap.
    pub(crate) fn handle_overlay_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        use crossterm::event::{KeyCode, KeyModifiers};
        let Some(overlay) = self.overlay.as_mut() else {
            return false;
        };

        match overlay {
            Overlay::Setup(state) => {
                use crate::ui::overlay::SETUP_CHOICES;
                match key.code {
                    KeyCode::Esc => self.overlay = None,
                    KeyCode::Up | KeyCode::Char('k') => {
                        state.selected = state.selected.saturating_sub(1)
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        state.selected = (state.selected + 1).min(SETUP_CHOICES.len() - 1)
                    }
                    KeyCode::Enter => {
                        let choice = state.selected;
                        self.overlay = None;
                        self.begin_setup(choice);
                    }
                    _ => {}
                }
                return true;
            }
            Overlay::Share(state) => match key.code {
                KeyCode::Esc => self.overlay = None,
                // Once the link exists there is nothing left to edit.
                _ if state.result.is_some() => {
                    if key.code == KeyCode::Enter {
                        self.overlay = None;
                    }
                }
                KeyCode::Up => state.field = state.field.saturating_sub(1),
                KeyCode::Down | KeyCode::Tab => state.field = (state.field + 1).min(2),
                KeyCode::Left | KeyCode::Right => {
                    let forward = key.code == KeyCode::Right;
                    match state.field {
                        1 => {
                            let len = crate::ui::overlay::EXPIRIES.len() as isize;
                            let delta = if forward { 1 } else { -1 };
                            state.expiry = (state.expiry as isize + delta).rem_euclid(len) as usize;
                        }
                        2 => state.downloadable = !state.downloadable,
                        _ => {}
                    }
                }
                KeyCode::Backspace if state.field == 0 => {
                    state.description.pop();
                }
                KeyCode::Char(c)
                    if state.field == 0 && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    state.description.push(c);
                }
                KeyCode::Enter if !state.pending => self.submit_share(),
                _ => {}
            },
            Overlay::Playlist(state) => match key.code {
                KeyCode::Esc => {
                    if state.creating {
                        state.creating = false;
                    } else {
                        self.overlay = None;
                    }
                }
                KeyCode::Backspace if state.creating => {
                    state.new_name.pop();
                }
                KeyCode::Enter => self.submit_playlist_add(),
                KeyCode::Char(c) if state.creating => state.new_name.push(c),
                KeyCode::Up | KeyCode::Char('k') => {
                    state.selected = state.selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let last = state.playlists.len().saturating_sub(1);
                    state.selected = (state.selected + 1).min(last);
                }
                KeyCode::Char('n') => {
                    state.creating = true;
                    state.new_name.clear();
                }
                _ => {}
            },
            Overlay::Palette(state) => match key.code {
                KeyCode::Esc => self.overlay = None,
                KeyCode::Enter => self.run_palette_choice(),
                KeyCode::Up => state.selected = state.selected.saturating_sub(1),
                KeyCode::Down => {
                    let last = state.matches.len().saturating_sub(1);
                    state.selected = (state.selected + 1).min(last);
                }
                KeyCode::Backspace => {
                    state.query.pop();
                    state.refilter();
                    self.search_for_palette();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    state.query.push(c);
                    state.selected = 0;
                    state.refilter();
                    self.search_for_palette();
                }
                _ => {}
            },
        }
        true
    }

    pub(crate) fn submit_share(&mut self) {
        let Some(Overlay::Share(state)) = self.overlay.as_mut() else {
            return;
        };
        state.pending = true;
        let ids: Vec<String> = state.songs.iter().map(|s| s.id.clone()).collect();
        let description = if state.description.is_empty() {
            state.label.clone()
        } else {
            state.description.clone()
        };
        let expires = state.expires_ms();
        let downloadable = state.downloadable;
        let library = Arc::clone(&self.library);
        self.spawn_load(async move {
            let result = library
                .create_share(&ids, &description, expires, downloadable)
                .await
                .map(|share| share.url)
                .map_err(|err| format!("{err:#}"));
            Ok(LoadEvent::ShareCreated(result))
        });
    }

    pub(crate) fn submit_playlist_add(&mut self) {
        let Some(Overlay::Playlist(state)) = self.overlay.as_ref() else {
            return;
        };
        let ids: Vec<String> = state.songs.iter().map(|s| s.id.clone()).collect();
        let library = Arc::clone(&self.library);

        if state.creating {
            let name = state.new_name.trim().to_string();
            if name.is_empty() {
                return;
            }
            self.status_message = Some(format!("Creating playlist “{name}”…"));
            self.spawn_load(async move {
                library.create_playlist(&name, &ids).await?;
                Ok(LoadEvent::Playlists(library.playlists().await?))
            });
        } else {
            let Some(playlist) = state.playlists.get(state.selected) else {
                return;
            };
            let (id, name) = (playlist.id.clone(), playlist.name.clone());
            let count = ids.len();
            self.status_message = Some(format!("Added {count} track(s) to {name}"));
            self.spawn_load(async move {
                library.add_to_playlist(&id, &ids).await?;
                Ok(LoadEvent::Playlists(library.playlists().await?))
            });
        }
        self.overlay = None;
    }

    /// Drop the focused track from the playlist it is shown in.
    pub(crate) fn remove_from_playlist(&mut self) {
        let Some(playlist) = self.playlists.get(self.playlist_sel.index) else {
            return;
        };
        let index = self.playlist_song_sel.index;
        if index >= self.playlist_songs.len() {
            return;
        }
        // Optimistic: the list is redrawn from the server response anyway, but
        // removing locally first keeps the UI from feeling laggy.
        let removed = self.playlist_songs.remove(index);
        self.playlist_song_sel.clamp(self.playlist_songs.len());
        self.status_message = Some(format!("Removed {} from {}", removed.title, playlist.name));

        let id = playlist.id.clone();
        let library = Arc::clone(&self.library);
        self.spawn_load(async move {
            library.remove_from_playlist(&id, &[index]).await?;
            let songs = library.playlist_songs(&id).await?;
            Ok(LoadEvent::PlaylistSongs {
                playlist_id: id,
                songs,
            })
        });
    }

    // ---- palette, undo ---------------------------------------------------

    /// Open the fuzzy "go to anything" palette.
    ///
    /// Built from what is already loaded, so it opens instantly; typing then
    /// folds in server-side search results for tracks not in memory.
    pub fn open_palette(&mut self) {
        use crate::ui::overlay::{PaletteItem, PaletteKind, PaletteState, PaletteTarget};

        let mut items: Vec<PaletteItem> = Vec::new();

        for artist in &self.artists {
            items.push(PaletteItem {
                kind: PaletteKind::Artist,
                label: artist.name.clone(),
                detail: "artist".to_string(),
                target: PaletteTarget::Reveal {
                    kind: PaletteKind::Artist,
                    id: artist.id.clone(),
                },
            });
        }
        for album in &self.albums {
            items.push(PaletteItem {
                kind: PaletteKind::Album,
                label: album.name.clone(),
                detail: album.artist.clone().unwrap_or_else(|| "album".to_string()),
                target: PaletteTarget::Reveal {
                    kind: PaletteKind::Album,
                    id: album.id.clone(),
                },
            });
        }
        for playlist in &self.playlists {
            items.push(PaletteItem {
                kind: PaletteKind::Playlist,
                label: playlist.name.clone(),
                detail: format!("{} tracks", playlist.song_count),
                target: PaletteTarget::Reveal {
                    kind: PaletteKind::Playlist,
                    id: playlist.id.clone(),
                },
            });
        }
        // Commands last: a name match should beat a keybinding description.
        for (keys, description) in self
            .keymap
            .describe_grouped()
            .iter()
            .flat_map(|(_, entries)| entries.iter())
        {
            if let Some(action) = self.keymap.action_for_description(description) {
                items.push(PaletteItem {
                    kind: PaletteKind::Command,
                    label: description.clone(),
                    detail: keys.clone(),
                    target: PaletteTarget::Command(action),
                });
            }
        }

        let mut state = PaletteState {
            items,
            ..Default::default()
        };
        state.refilter();
        self.overlay = Some(Overlay::Palette(state));
    }

    /// Fold server-side track results into the open palette.
    pub(crate) fn apply_palette_songs(&mut self, generation: u64, songs: Vec<Song>) {
        use crate::ui::overlay::{PaletteItem, PaletteKind, PaletteTarget};
        let Some(Overlay::Palette(state)) = self.overlay.as_mut() else {
            return;
        };
        // Typing moved on; these results are for a query that no longer exists.
        if generation != state.generation {
            return;
        }
        state.items.retain(|item| item.kind != PaletteKind::Song);
        let songs_len = songs.len();
        for (index, song) in songs.iter().enumerate() {
            state.items.push(PaletteItem {
                kind: PaletteKind::Song,
                label: song.title.clone(),
                detail: song.artist_or_unknown().to_string(),
                target: PaletteTarget::Songs {
                    songs: songs.clone(),
                    index,
                },
            });
            // One clone of the list per row is wasteful beyond a screenful.
            if index + 1 >= 50.min(songs_len) {
                break;
            }
        }
        state.refilter();
    }

    pub(crate) fn search_for_palette(&mut self) {
        let Some(Overlay::Palette(state)) = self.overlay.as_mut() else {
            return;
        };
        state.generation += 1;
        let generation = state.generation;
        let query = state.query.trim().to_string();
        if query.len() < 2 {
            return;
        }
        let library = Arc::clone(&self.library);
        self.spawn_load(async move {
            let results = library.search(&query, 50).await?;
            Ok(LoadEvent::PaletteSongs {
                generation,
                songs: results.song,
            })
        });
    }

    pub(crate) fn run_palette_choice(&mut self) {
        use crate::ui::overlay::{PaletteKind, PaletteTarget};
        let Some(Overlay::Palette(state)) = self.overlay.as_ref() else {
            return;
        };
        let Some(target) = state.chosen().map(|item| item.target.clone()) else {
            return;
        };
        self.overlay = None;

        match target {
            PaletteTarget::Songs { songs, index } => {
                self.snapshot_queue();
                self.player.send(PlayerCommand::PlayNow { songs, index });
            }
            PaletteTarget::Reveal { kind, id } => match kind {
                PaletteKind::Artist => {
                    if let Some(pos) = self.artists.iter().position(|a| a.id == id) {
                        self.go_to_library(LibraryMode::Artists);
                        self.select_in(Pane::Artists, pos);
                    }
                }
                PaletteKind::Album => {
                    if let Some(pos) = self.albums.iter().position(|a| a.id == id) {
                        self.go_to_library(LibraryMode::Albums);
                        self.select_in(Pane::Albums, pos);
                    }
                }
                PaletteKind::Playlist => {
                    if let Some(pos) = self.playlists.iter().position(|p| p.id == id) {
                        self.go_to_library(LibraryMode::Playlists);
                        self.select_in(Pane::Playlists, pos);
                    }
                }
                _ => {}
            },
            // Re-entering the action dispatcher is safe: the palette is closed.
            PaletteTarget::Command(action) => self.handle_action(action),
        }
    }

    /// Remember the queue before a destructive change, so it can be undone.
    pub(crate) fn snapshot_queue(&mut self) {
        let queue = self.player.queue.lock().unwrap();
        if queue.is_empty() {
            return;
        }
        let snapshot = (queue.songs().to_vec(), queue.current_index().unwrap_or(0));
        drop(queue);
        self.queue_undo.push(snapshot);
        // A handful of steps is all anyone reaches for, and each holds a queue.
        if self.queue_undo.len() > 10 {
            self.queue_undo.remove(0);
        }
    }

    pub(crate) fn undo_queue(&mut self) {
        let Some((songs, index)) = self.queue_undo.pop() else {
            self.status_message = Some("Nothing to undo".to_string());
            return;
        };
        let count = songs.len();
        self.player.queue.lock().unwrap().restore(songs, index);
        self.status_message = Some(format!("Restored queue of {count} track(s)"));
        self.save_queue_state();
    }

    // ---- ratings, queue order, mixes ------------------------------------
}
