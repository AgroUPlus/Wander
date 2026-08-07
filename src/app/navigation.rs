use super::types::*;
use super::*;
use crate::subsonic::models::Song;

/// Free-standing and pure so the focus order can be tested without an `App`,
/// which needs an audio device to exist.
pub fn focus_order(tab: Tab, library_mode: LibraryMode, side_queue: bool) -> Vec<Pane> {
    let mut panes: Vec<Pane> = match tab {
        Tab::Home => vec![Pane::Home],
        Tab::Queue => vec![Pane::Queue],
        Tab::Library => library_mode.panes().to_vec(),
        Tab::Online => vec![Pane::Online],
        Tab::Operations => vec![Pane::Operations],
        Tab::Settings => vec![Pane::Settings],
    };
    // The Up Next pane is drawn to the right of everything else, so it is the
    // last stop when cycling right — reachable from the keyboard rather than
    // only by clicking it.
    if side_queue {
        panes.push(Pane::Queue);
    }
    panes
}

/// Expand a leading `~` to the user's home directory.

impl App {
    pub(crate) fn go_to_tab(&mut self, tab: Tab) {
        if tab != self.tab {
            self.tab_history.push(self.tab);
            // Keep the history shallow; it is a convenience, not a full stack.
            if self.tab_history.len() > 16 {
                self.tab_history.remove(0);
            }
            self.tab = tab;
            self.focus = self.panes()[0];
        }
    }

    /// Panes of the current tab, left to right. Focus moves within this list.
    ///
    /// Lives on `App` rather than `Tab` because the Library tab's panes depend
    /// on the mode it is showing.
    /// Whether the Up Next pane is drawn beside the content.
    ///
    /// The Queue tab already shows the queue full-size, so the side pane would
    /// be redundant there. Both the layout and the focus order read this, so
    /// they cannot disagree about whether the pane is there to focus.
    pub fn side_queue_visible(&self) -> bool {
        self.show_queue_pane && self.tab != Tab::Queue
    }

    /// The focusable panes, left to right, in the order Left/Right cycles them.
    pub fn panes(&self) -> Vec<Pane> {
        focus_order(self.tab, self.library_mode, self.side_queue_visible())
    }

    /// Open the Library tab on a given mode, loading it if needed.
    pub fn go_to_library(&mut self, mode: LibraryMode) {
        self.go_to_tab(Tab::Library);
        self.set_library_mode(mode);
    }

    pub fn set_library_mode(&mut self, mode: LibraryMode) {
        self.library_mode = mode;
        self.focus = self.panes()[0];
        match mode {
            // These two are fetched lazily: the flat lists can be large, and a
            // user who never opens them should never pay for them.
            LibraryMode::Tracks if !self.tracks_loaded => self.load_tracks(),
            LibraryMode::Favorites if !self.favorites_loaded => self.load_favorites(),
            _ => {}
        }
        self.save_queue_state();
    }

    /// Move between Library modes with the same keys used for panes.
    pub fn cycle_library_mode(&mut self, delta: isize) {
        let modes = LibraryMode::ALL;
        let current = modes
            .iter()
            .position(|m| *m == self.library_mode)
            .unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(modes.len() as isize) as usize;
        self.set_library_mode(modes[next]);
    }

    pub(crate) fn cycle_tab(&mut self, delta: isize) {
        let available = self.available_tabs();
        let current = available.iter().position(|t| *t == self.tab).unwrap_or(0) as isize;
        let count = available.len() as isize;
        let next = (current + delta).rem_euclid(count) as usize;
        self.go_to_tab(available[next]);
    }

    /// Move focus between the panes of the current tab.
    /// Move focus back onto a pane that is actually on screen.
    ///
    /// Hiding the Up Next pane while it has focus would otherwise leave the
    /// cursor on a pane the user can no longer see, so their next keypress
    /// would go somewhere invisible.
    pub fn ensure_focus_visible(&mut self) {
        let panes = self.panes();
        if !panes.contains(&self.focus) {
            self.focus = panes[0];
        }
    }

    /// Move focus one pane, wrapping around the ends.
    ///
    /// Wrapping is right for a dedicated cycle key — holding it should visit
    /// every pane — but wrong for Left/Right, where running off the left edge
    /// should stop rather than teleport to the far right.
    pub(crate) fn cycle_focus(&mut self, delta: isize) {
        let panes = self.panes();
        if panes.is_empty() {
            return;
        }
        let current = panes.iter().position(|p| *p == self.focus).unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(panes.len() as isize) as usize;
        self.focus = panes[next];
    }

    pub(crate) fn focus_by(&mut self, delta: isize) {
        let panes = self.panes();
        let current = panes.iter().position(|p| *p == self.focus).unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, panes.len() as isize - 1) as usize;
        self.focus = panes[next];
    }

    /// Length of the list backing a pane.
    pub(crate) fn pane_len(&self, pane: Pane) -> usize {
        match pane {
            Pane::Queue => self.player.queue.lock().unwrap().len(),
            Pane::Artists => self.artists.len(),
            Pane::ArtistAlbums => self.artist_albums.len(),
            Pane::ArtistSongs => self.artist_songs.len(),
            Pane::Albums => self.albums.len(),
            Pane::AlbumSongs => self.album_songs.len(),
            Pane::Playlists => self.playlists.len(),
            Pane::PlaylistSongs => self.playlist_songs.len(),
            Pane::Tracks => self.tracks.len(),
            Pane::Favorites => self.favorites.len(),
            Pane::Online => match self.online_source {
                #[cfg(feature = "nyaa")]
                OnlineSource::Nyaa => self.nyaa_plugin.results.len(),
                OnlineSource::Archive => self.archive_plugin.results.len(),
                OnlineSource::Jamendo => self.jamendo_plugin.results.len(),
            },
            Pane::Settings => crate::ui::settings::rows(&self.config).len(),
            Pane::Operations => self.operations.len(),
            Pane::Home => crate::ui::home::mix_count(self),
            // Synced lyrics scroll with playback and have no cursor; unsynced
            // ones have nothing to follow, so they are scrolled by hand.
            Pane::Lyrics => {
                if self.lyrics.synced {
                    0
                } else {
                    self.lyrics.lines.len()
                }
            }
        }
    }

    pub(crate) fn selection_mut(&mut self, pane: Pane) -> &mut Selection {
        match pane {
            Pane::Queue => &mut self.queue_sel,
            Pane::Artists => &mut self.artist_sel,
            Pane::ArtistAlbums => &mut self.artist_album_sel,
            Pane::ArtistSongs => &mut self.artist_song_sel,
            Pane::Albums => &mut self.album_sel,
            Pane::AlbumSongs => &mut self.album_song_sel,
            Pane::Playlists => &mut self.playlist_sel,
            Pane::PlaylistSongs => &mut self.playlist_song_sel,
            Pane::Tracks => &mut self.track_sel,
            Pane::Favorites => &mut self.favorite_sel,
            Pane::Lyrics => &mut self.lyrics_sel,
            Pane::Online => match self.online_source {
                #[cfg(feature = "nyaa")]
                OnlineSource::Nyaa => &mut self.nyaa_plugin.selection,
                OnlineSource::Archive => &mut self.archive_plugin.selection,
                OnlineSource::Jamendo => &mut self.jamendo_plugin.selection,
            },
            Pane::Settings => &mut self.settings_sel,
            Pane::Operations => &mut self.operations_sel,
            Pane::Home => &mut self.home_sel,
        }
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        let pane = self.focus;
        let len = self.pane_len(pane);
        let before = self.selection_mut(pane).index;
        self.selection_mut(pane).move_by(delta, len);
        let after = self.selection_mut(pane).index;
        if before != after {
            self.on_selection_changed(pane);
        }
    }

    pub(crate) fn select_in(&mut self, pane: Pane, index: usize) {
        let len = self.pane_len(pane);
        let before = self.selection_mut(pane).index;
        self.selection_mut(pane).set(index, len);
        if before != self.selection_mut(pane).index {
            self.on_selection_changed(pane);
        }
    }

    /// Load the dependent pane when a parent selection changes.
    pub(crate) fn on_selection_changed(&mut self, pane: Pane) {
        match pane {
            Pane::Artists => {
                if let Some(artist) = self.artists.get(self.artist_sel.index) {
                    self.load_artist_albums(artist.id.clone());
                }
            }
            Pane::ArtistAlbums => {
                if let Some(album) = self.artist_albums.get(self.artist_album_sel.index) {
                    self.load_album_songs(album.id.clone());
                }
            }
            Pane::Albums => {
                if let Some(album) = self.albums.get(self.album_sel.index) {
                    self.load_album_songs(album.id.clone());
                }
            }
            Pane::Playlists => {
                if let Some(playlist) = self.playlists.get(self.playlist_sel.index) {
                    self.load_playlist_songs(playlist.id.clone());
                }
            }
            _ => {}
        }
    }

    /// The song list the focused pane represents, and the selected index in it.
    pub(crate) fn focused_songs(&self) -> (Vec<Song>, usize) {
        match self.focus {
            Pane::Queue => (Vec::new(), self.queue_sel.index),
            Pane::Artists | Pane::ArtistAlbums => (self.artist_songs.clone(), 0),
            Pane::ArtistSongs => (self.artist_songs.clone(), self.artist_song_sel.index),
            Pane::Albums => (self.album_songs.clone(), 0),
            Pane::AlbumSongs => (self.album_songs.clone(), self.album_song_sel.index),
            Pane::Playlists => (self.playlist_songs.clone(), 0),
            Pane::PlaylistSongs => (self.playlist_songs.clone(), self.playlist_song_sel.index),
            Pane::Tracks => (self.tracks.clone(), self.track_sel.index),
            Pane::Favorites => (self.favorites.clone(), self.favorite_sel.index),
            // Nothing selectable: lyrics, settings, home, online & operations track their own state.
            Pane::Lyrics | Pane::Settings | Pane::Home | Pane::Online | Pane::Operations => (Vec::new(), 0),
        }
    }

    /// Songs the current selection refers to, for enqueueing.
    pub(crate) fn selected_songs(&self) -> Vec<Song> {
        match self.focus {
            // A container pane queues everything it contains.
            Pane::Artists | Pane::ArtistAlbums | Pane::Albums | Pane::Playlists => {
                self.focused_songs().0
            }
            Pane::Queue => Vec::new(),
            _ => {
                let (songs, index) = self.focused_songs();
                songs.get(index).cloned().into_iter().collect()
            }
        }
    }

    pub(crate) fn activate(&mut self) {
        if self.focus == Pane::Online {
            let (action, stream): (_, fn(&mut Self)) = match self.online_source {
                #[cfg(feature = "nyaa")]
                OnlineSource::Nyaa => (
                    self.config.plugins.nyaa.primary_action,
                    Self::stream_selected_nyaa_item as fn(&mut Self),
                ),
                OnlineSource::Archive => (
                    self.config.plugins.archive.primary_action,
                    Self::stream_selected_archive_item as fn(&mut Self),
                ),
                OnlineSource::Jamendo => (
                    self.config.plugins.jamendo.primary_action,
                    Self::stream_selected_jamendo_track as fn(&mut Self),
                ),
            };
            let download: fn(&mut Self) = match self.online_source {
                #[cfg(feature = "nyaa")]
                OnlineSource::Nyaa => Self::download_selected_nyaa_item,
                OnlineSource::Archive => Self::download_selected_archive_item,
                OnlineSource::Jamendo => Self::download_selected_jamendo_track,
            };
            if action == crate::config::OnlinePrimaryAction::Stream {
                stream(self);
            } else {
                download(self);
            }
            return;
        }
        if self.focus == Pane::Settings {
            self.activate_setting();
            return;
        }
        if self.focus == Pane::Home {
            self.start_mix(self.home_sel.index);
            return;
        }
        if self.focus == Pane::Queue {
            self.player
                .send(PlayerCommand::PlayQueueIndex(self.queue_sel.index));
            return;
        }
        let (songs, index) = self.focused_songs();
        if !songs.is_empty() {
            // Replacing the queue is destructive; make it undoable.
            self.snapshot_queue();
            self.player.send(PlayerCommand::PlayNow { songs, index });
        }
    }

    // ---- overlays ------------------------------------------------------

    /// The track an action like star, rate or share should apply to: the
    /// selection when one is focused, otherwise whatever is playing.
    pub(crate) fn target_songs(&self) -> Vec<Song> {
        let selected = self.selected_songs();
        if !selected.is_empty() {
            return selected;
        }
        self.player.status().current.into_iter().collect()
    }
}
