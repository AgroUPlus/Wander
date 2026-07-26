use anyhow::Result;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::config::Config;
use crate::keymap::{Action, Keymap};
use crate::library::Library;
use crate::player::{PlayerCommand, PlayerHandle};
use crate::subsonic::cache::CoverCache;
use crate::subsonic::models::{Album, Artist, Playlist, Song};
use crate::theme::Theme;
use crate::ui::overlay::{Overlay, PlaylistState, ShareState};
use crate::ui::{Hits, Region};

/// Square pixel size requested for cover art.
///
/// Sized for focus mode on a large terminal: graphics protocols cannot upscale
/// beyond the source, so a small cover leaves the artwork smaller than the pane
/// it was given. Covers are cached on disk, so the extra bytes are paid once
/// per album.
pub const COVER_SIZE: u32 = 1600;
const VOLUME_STEP: f32 = 0.05;
/// Two clicks within this window on the same row count as a double-click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Minimum gap between session-state writes.
const STATE_SAVE_INTERVAL: Duration = Duration::from_secs(1);
/// Fraction of the remaining distance a pane size covers each frame. Matches
/// the easing used for lyric scrolling, so the whole UI moves the same way.
const PANE_EASE: f32 = 0.25;
/// Below this, a tween has arrived and snaps to its target.
const PANE_EPSILON: f32 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Home,
    Queue,
    /// Artists, albums, tracks and playlists are all views of one library, not
    /// separate destinations — an album is always reachable through its artist.
    Library,
    Settings,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Home, Tab::Queue, Tab::Library, Tab::Settings];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Home => "Home",
            Tab::Queue => "Queue",
            Tab::Library => "Library",
            Tab::Settings => "Settings",
        }
    }
}

/// Which slice of the library the Library tab is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LibraryMode {
    Artists,
    Albums,
    Tracks,
    Playlists,
    Favorites,
}

impl LibraryMode {
    pub const ALL: [LibraryMode; 5] = [
        LibraryMode::Artists,
        LibraryMode::Albums,
        LibraryMode::Tracks,
        LibraryMode::Playlists,
        LibraryMode::Favorites,
    ];

    pub fn title(self) -> &'static str {
        match self {
            LibraryMode::Artists => "Artists",
            LibraryMode::Albums => "Albums",
            LibraryMode::Tracks => "Tracks",
            LibraryMode::Playlists => "Playlists",
            LibraryMode::Favorites => "Favorites",
        }
    }

    fn panes(self) -> &'static [Pane] {
        match self {
            LibraryMode::Artists => &[Pane::Artists, Pane::ArtistAlbums, Pane::ArtistSongs],
            LibraryMode::Albums => &[Pane::Albums, Pane::AlbumSongs],
            LibraryMode::Tracks => &[Pane::Tracks],
            LibraryMode::Playlists => &[Pane::Playlists, Pane::PlaylistSongs],
            LibraryMode::Favorites => &[Pane::Favorites],
        }
    }
}

/// An individually focusable and clickable list on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Home,
    Queue,
    Artists,
    ArtistAlbums,
    ArtistSongs,
    Albums,
    AlbumSongs,
    Playlists,
    PlaylistSongs,
    /// Flat track list in the Library tab.
    Tracks,
    /// Starred tracks in the Library tab.
    Favorites,
    Lyrics,
    Settings,
}

/// Results of async work, applied to the state on the UI thread.
pub enum LoadEvent {
    Artists(Vec<Artist>),
    ArtistAlbums {
        artist_id: String,
        albums: Vec<Album>,
    },
    AlbumSongs {
        album_id: String,
        songs: Vec<Song>,
    },
    Albums(Vec<Album>),
    Tracks(Vec<Song>),
    Favorites(Vec<Song>),
    Playlists(Vec<Playlist>),
    PlaylistSongs {
        playlist_id: String,
        songs: Vec<Song>,
    },
    Cover {
        cover_id: String,
        bytes: Vec<u8>,
    },
    Lyrics {
        song_id: String,
        lyrics: Box<crate::subsonic::lyrics::Lyrics>,
    },
    /// A share link, or the reason the server refused to make one.
    ShareCreated(Result<String, String>),
    /// Seed tracks for a Home mix.
    Mix {
        name: String,
        songs: Vec<Song>,
    },
    /// Library genres, biggest first, used for mixes before any history exists.
    Genres(Vec<String>),
    /// Server-side track results for the open palette.
    PaletteSongs {
        generation: u64,
        songs: Vec<Song>,
    },
    /// Cover art finished resizing on the encoder thread.
    ///
    /// Boxed because the encoded image dwarfs every other variant, and an enum
    /// is as large as its biggest member.
    CoverResized(Box<ratatui_image::thread::ResizeResponse>),
    /// Outcome of the settings panel's "Test connection".
    ConnectionTested(String),
    /// A local library scan finished, with what it found.
    LocalScanned {
        songs: usize,
        albums: usize,
    },
    Error(String),
}

/// A list selection that stays in range as the underlying list changes.
#[derive(Debug, Default, Clone, Copy)]
pub struct Selection {
    pub index: usize,
    pub offset: usize,
}

impl Selection {
    pub fn clamp(&mut self, len: usize) {
        if len == 0 {
            self.index = 0;
        } else if self.index >= len {
            self.index = len - 1;
        }
    }

    pub fn move_by(&mut self, delta: isize, len: usize) {
        if len == 0 {
            self.index = 0;
            return;
        }
        let next = self.index as isize + delta;
        self.index = next.clamp(0, len as isize - 1) as usize;
    }

    pub fn set(&mut self, index: usize, len: usize) {
        self.index = index;
        self.clamp(len);
    }

    pub fn reset(&mut self) {
        self.index = 0;
        self.offset = 0;
    }
}

pub struct App {
    pub config: Config,
    pub library: Arc<dyn Library>,
    /// The same library, concretely typed, so the settings panel can swap a
    /// backend in or out. Every other holder keeps its `Arc<dyn Library>` and
    /// never notices, which is what makes reconfiguring work without a restart.
    pub library_root: Option<Arc<crate::library::MergedLibrary>>,
    pub player: PlayerHandle,
    pub keymap: Keymap,
    pub covers: Arc<CoverCache>,
    pub loads: mpsc::UnboundedSender<LoadEvent>,

    pub tab: Tab,
    pub focus: Pane,
    /// Previous tabs, so Backspace can walk back.
    tab_history: Vec<Tab>,
    pub should_quit: bool,
    pub status_message: Option<String>,

    pub show_help: bool,
    pub show_queue_pane: bool,
    pub show_cover_pane: bool,
    pub show_lyrics_pane: bool,
    /// Lyrics in focus mode, tracked separately from the side pane.
    ///
    /// Focus mode is a reading view, so lyrics belong there by default — but
    /// the side pane is off by default, and sharing one flag meant turning one
    /// on forced the other.
    pub show_focus_lyrics: bool,
    pub show_visualiser: bool,

    pub artists: Vec<Artist>,
    pub artist_sel: Selection,
    pub artist_albums: Vec<Album>,
    pub artist_album_sel: Selection,
    pub artist_songs: Vec<Song>,
    pub artist_song_sel: Selection,

    pub library_mode: LibraryMode,
    /// Flat track list, and the starred list, both shown in the Library tab.
    pub tracks: Vec<Song>,
    pub track_sel: Selection,
    pub tracks_loaded: bool,
    pub favorites: Vec<Song>,
    pub favorite_sel: Selection,
    pub favorites_loaded: bool,

    pub albums: Vec<Album>,
    pub album_sel: Selection,
    pub album_songs: Vec<Song>,
    pub album_song_sel: Selection,

    pub playlists: Vec<Playlist>,
    pub playlist_sel: Selection,
    pub playlist_songs: Vec<Song>,
    pub playlist_song_sel: Selection,

    /// Ratings set this session, overlaying what the server last told us.
    ///
    /// The server is the source of truth (`Song::user_rating`), but a rating
    /// only reappears there after the track is refetched, so this keeps the
    /// stars correct in the meantime.
    pub ratings: std::collections::HashMap<String, u8>,

    pub home_sel: Selection,
    pub queue_sel: Selection,
    /// Cursor for unsynced lyrics, which have no timing to follow.
    lyrics_sel: Selection,
    pub settings_sel: Selection,
    /// The text field open on the selected settings row, if any. While this is
    /// `Some`, the settings pane swallows keystrokes so typing a URL cannot
    /// trigger playback shortcuts.
    pub settings_edit: Option<crate::ui::widgets::TextInput>,
    /// Result of the last "Test connection", shown inline on that row.
    pub connection_status: Option<String>,
    /// Progress or result of the last local library scan.
    pub scan_status: Option<String>,
    /// Whether a password is available for the configured user, so the panel
    /// can say "stored" without ever reading the secret itself.
    pub has_stored_password: bool,

    pub cover_id: Option<String>,
    pub cover_bytes: Option<Vec<u8>>,
    pub cover_dirty: bool,
    /// A finished encoding waiting to be handed to `CoverRenderer`, which the
    /// app does not own.
    pub cover_resized: Option<Box<ratatui_image::thread::ResizeResponse>>,

    pub lyrics: crate::subsonic::lyrics::Lyrics,
    pub lyrics_pending: bool,
    /// Eased scroll position, in lyric-line units. Retained between frames so
    /// the view glides rather than snapping.
    pub lyrics_scroll: f32,
    lyrics_cache: Arc<crate::subsonic::lyrics::LyricsCache>,
    /// Track the loaded lyrics belong to, so stale replies are discarded.
    lyrics_song: Option<String>,

    /// The modal popup that currently owns the keyboard, if any.
    pub overlay: Option<crate::ui::overlay::Overlay>,
    /// Full-screen now-playing view: cover, lyrics and visualiser only.
    pub focus_mode: bool,
    /// Snapshots taken before destructive queue edits, newest last.
    queue_undo: Vec<(Vec<Song>, usize)>,
    /// The play log, read once at startup and appended to in memory. Re-reading
    /// the file on every track change was a visible hitch.
    history: Vec<crate::history::PlayRecord>,
    /// Bytes of the log already folded into `history`.
    history_bytes: u64,
    /// Aggregated listening history, recomputed when a track finishes.
    pub stats: crate::history::Stats,
    /// The library's biggest genres, used for Home mixes until enough has been
    /// played for the history to speak for itself.
    pub library_genres: Vec<String>,
    /// What the Discord integration last did about cover art, surfaced in
    /// Settings so a blank image is explainable rather than mysterious.
    pub discord_diagnostic: Option<Arc<std::sync::Mutex<String>>>,

    pub active_drag: Option<Region>,
    pub drag_ratio: f64,
    /// Target pane sizes. The values actually drawn ease towards these, so a
    /// resize glides instead of jumping — see `animated_*` below.
    pub cover_percent: u16,
    pub queue_percent: u16,
    eased_cover: f32,
    eased_queue: f32,

    /// Session state is written at most once a second: serialising a long queue
    /// on every keypress is what made held-down resize keys stutter.
    state_dirty: bool,
    last_state_save: Instant,

    last_click: Option<(Region, Instant)>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct SavedAppState {
    songs: Vec<Song>,
    index: usize,
    volume: f32,
    cover_percent: u16,
    queue_percent: u16,
    show_queue_pane: bool,
    show_cover_pane: bool,
    show_lyrics_pane: bool,
    #[serde(default = "yes")]
    show_focus_lyrics: bool,
    show_visualiser: bool,
    #[serde(default)]
    radio: bool,
    /// Position within the current track, so a restart resumes there.
    #[serde(default)]
    elapsed_secs: f64,
    #[serde(default = "default_library_mode")]
    library_mode: LibraryMode,
}

/// Saved states written before focus mode existed should still show lyrics.
fn yes() -> bool {
    true
}

fn default_library_mode() -> LibraryMode {
    LibraryMode::Artists
}

fn queue_cache_path() -> Option<std::path::PathBuf> {
    Some(crate::paths::cache_dir()?.join("saved_state.json"))
}

impl App {
    pub fn new(
        config: Config,
        library: Arc<dyn Library>,
        player: PlayerHandle,
        loads: mpsc::UnboundedSender<LoadEvent>,
    ) -> Result<Self> {
        // Built before `config` moves into the struct. Problems are collected
        // and reported once the status line exists, so a typo in `[keys]` is
        // visible rather than silent.
        let mut keymap = Keymap::default();
        let keybinding_problems = keymap.apply_overrides(&config.keys);

        let mut app = Self {
            config,
            library,
            library_root: None,
            player,
            keymap,
            covers: Arc::new(CoverCache::new()?),
            loads,
            tab: Tab::Home,
            focus: Pane::Home,
            tab_history: Vec::new(),
            should_quit: false,
            status_message: None,
            show_help: false,
            show_queue_pane: true,
            show_cover_pane: true,
            show_lyrics_pane: false,
            show_focus_lyrics: true,
            show_visualiser: true,
            artists: Vec::new(),
            artist_sel: Selection::default(),
            artist_albums: Vec::new(),
            artist_album_sel: Selection::default(),
            artist_songs: Vec::new(),
            artist_song_sel: Selection::default(),
            library_mode: LibraryMode::Artists,
            tracks: Vec::new(),
            track_sel: Selection::default(),
            tracks_loaded: false,
            favorites: Vec::new(),
            favorite_sel: Selection::default(),
            favorites_loaded: false,
            albums: Vec::new(),
            album_sel: Selection::default(),
            album_songs: Vec::new(),
            album_song_sel: Selection::default(),
            playlists: Vec::new(),
            playlist_sel: Selection::default(),
            playlist_songs: Vec::new(),
            playlist_song_sel: Selection::default(),
            ratings: std::collections::HashMap::new(),
            home_sel: Selection::default(),
            queue_sel: Selection::default(),
            lyrics_sel: Selection::default(),
            settings_sel: Selection::default(),
            settings_edit: None,
            connection_status: None,
            scan_status: None,
            has_stored_password: false,
            cover_id: None,
            cover_bytes: None,
            cover_dirty: false,
            cover_resized: None,
            lyrics: Default::default(),
            lyrics_pending: false,
            lyrics_scroll: 0.0,
            lyrics_cache: Arc::new(crate::subsonic::lyrics::LyricsCache::new()?),
            lyrics_song: None,
            overlay: None,
            focus_mode: false,
            queue_undo: Vec::new(),
            history: Vec::new(),
            history_bytes: 0,
            stats: crate::history::Stats::default(),
            library_genres: Vec::new(),
            discord_diagnostic: None,
            active_drag: None,
            drag_ratio: 0.0,
            cover_percent: 25,
            queue_percent: 18,
            eased_cover: 25.0,
            eased_queue: 18.0,
            state_dirty: false,
            last_state_save: Instant::now(),
            last_click: None,
        };
        if !keybinding_problems.is_empty() {
            app.status_message = Some(format!("config [keys]: {}", keybinding_problems.join("; ")));
        }
        app.load_queue_state();
        app.history = crate::history::load();
        app.history_bytes = crate::history::size();
        app.stats = crate::history::stats(&app.history, 5);
        Ok(app)
    }

    /// Note that the session state changed. The write itself is deferred to
    /// [`Self::flush_state`], so a held-down key does not hit the disk on every
    /// repeat.
    pub fn save_queue_state(&mut self) {
        self.state_dirty = true;
    }

    /// Write the session state if it is dirty and the cooldown has elapsed.
    /// `force` skips the cooldown, for quitting.
    pub fn flush_state(&mut self, force: bool) {
        if !self.state_dirty {
            return;
        }
        if !force && self.last_state_save.elapsed() < STATE_SAVE_INTERVAL {
            return;
        }
        self.state_dirty = false;
        self.last_state_save = Instant::now();
        self.write_queue_state();
    }

    fn write_queue_state(&self) {
        let Some(path) = queue_cache_path() else {
            return;
        };
        let (songs, index, radio) = {
            let queue = self.player.queue.lock().unwrap();
            (
                queue.songs().to_vec(),
                queue.current_index().unwrap_or(0),
                queue.radio,
            )
        };
        let state = SavedAppState {
            songs,
            index,
            volume: self.player.shared.volume(),
            cover_percent: self.cover_percent,
            queue_percent: self.queue_percent,
            show_queue_pane: self.show_queue_pane,
            show_cover_pane: self.show_cover_pane,
            show_lyrics_pane: self.show_lyrics_pane,
            show_focus_lyrics: self.show_focus_lyrics,
            show_visualiser: self.show_visualiser,
            radio,
            elapsed_secs: self.player.elapsed().as_secs_f64(),
            library_mode: self.library_mode,
        };
        if let Ok(raw) = serde_json::to_string(&state) {
            let _ = std::fs::write(path, raw);
        }
    }

    pub fn load_queue_state(&mut self) {
        let Some(path) = queue_cache_path() else {
            return;
        };
        let Ok(raw) = std::fs::read_to_string(path) else {
            return;
        };
        let Ok(state): Result<SavedAppState, _> = serde_json::from_str(&raw) else {
            return;
        };
        self.cover_percent = state.cover_percent.clamp(10, 80);
        self.queue_percent = state.queue_percent.clamp(10, 80);
        self.show_queue_pane = state.show_queue_pane;
        self.show_cover_pane = state.show_cover_pane;
        self.show_lyrics_pane = state.show_lyrics_pane;
        self.show_focus_lyrics = state.show_focus_lyrics;
        self.show_visualiser = state.show_visualiser;
        self.library_mode = state.library_mode;
        self.player.send(PlayerCommand::SetVolume(state.volume));
        let restored = {
            let mut queue = self.player.queue.lock().unwrap();
            let restored = !state.songs.is_empty();
            if restored {
                queue.restore(state.songs, state.index);
            }
            queue.radio = state.radio;
            restored
        };
        // Arm the track where it stopped, paused, so play resumes rather than
        // requiring the user to find and select it again.
        if restored {
            self.player.send(PlayerCommand::Resume {
                offset: Duration::from_secs_f64(state.elapsed_secs.max(0.0)),
            });
        }
    }

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

    fn spawn_load<F>(&self, future: F)
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
            if let Some(bytes) = covers.get(&cover_id, COVER_SIZE) {
                return Ok(LoadEvent::Cover { cover_id, bytes });
            }
            let bytes = library.cover_art(&cover_id, COVER_SIZE).await?;
            covers.put(&cover_id, COVER_SIZE, &bytes);
            Ok(LoadEvent::Cover { cover_id, bytes })
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
            LoadEvent::Cover { cover_id, bytes } => {
                if self.cover_id.as_deref() == Some(cover_id.as_str()) {
                    self.cover_bytes = Some(bytes);
                    self.cover_dirty = true;
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
            LoadEvent::Error(message) => {
                self.status_message = Some(message);
            }
        }
    }

    /// Load lyrics for the playing track, preferring the on-disk cache.
    ///
    /// Negative results are cached too, so a track without lyrics is not
    /// re-requested every time it plays.
    fn sync_lyrics(&mut self, song_id: Option<String>) {
        if self.lyrics_song == song_id {
            return;
        }
        self.lyrics_song = song_id.clone();
        self.lyrics = Default::default();
        self.lyrics_scroll = 0.0;

        let Some(song_id) = song_id else {
            self.lyrics_pending = false;
            return;
        };

        self.lyrics_pending = true;
        let library = Arc::clone(&self.library);
        let cache = Arc::clone(&self.lyrics_cache);
        self.spawn_load(async move {
            if let Some(cached) = cache.get(&song_id) {
                return Ok(LoadEvent::Lyrics {
                    song_id,
                    lyrics: Box::new(cached),
                });
            }
            let lyrics = library.lyrics(&song_id).await.unwrap_or_default();
            Ok(LoadEvent::Lyrics {
                song_id,
                lyrics: Box::new(lyrics),
            })
        });
    }

    /// Keep the cover and lyrics in sync with the playing track.
    pub fn sync_cover(&mut self) {
        let current = self.player.status().current;
        self.sync_lyrics(current.as_ref().map(|song| song.id.clone()));

        let wanted = current.and_then(|song| song.cover_art.clone());
        if wanted != self.cover_id {
            self.cover_id = wanted.clone();
            self.cover_bytes = None;
            self.cover_dirty = true;
            if let Some(cover_id) = wanted {
                self.load_cover(cover_id);
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

    /// Fetch the next queued track's cover ahead of time so track changes are
    /// visually instant rather than showing a placeholder.
    fn prefetch_next_cover(&self) {
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

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        if self.show_help {
            // Any key dismisses help, so it never traps the user.
            self.show_help = false;
            return;
        }

        // A popup owns the keyboard while it is open, so its text fields are
        // not also firing global single-letter bindings.
        if self.handle_overlay_key(key) {
            return;
        }

        // An open settings field owns the keyboard for the same reason: typing
        // a server URL must not trigger the single-letter playback bindings.
        if self.settings_edit.is_some() && self.handle_settings_edit_key(key) {
            return;
        }

        // The search box swallows keys so the user can type freely.

        let Some(action) = self.keymap.resolve(key) else {
            return;
        };
        self.handle_action(action);
    }

    /// Route a keystroke into the open settings text field.
    ///
    /// Returns whether the key was consumed; everything reaches the field
    /// except the keys that close it, so no global binding can fire mid-edit.
    fn handle_settings_edit_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Enter => {
                self.commit_setting_edit();
                return true;
            }
            KeyCode::Esc => {
                self.settings_edit = None;
                return true;
            }
            _ => {}
        }

        let Some(input) = self.settings_edit.as_mut() else {
            return true;
        };
        match key.code {
            KeyCode::Char('w') if ctrl => input.delete_word(),
            KeyCode::Char('u') if ctrl => input.clear(),
            KeyCode::Char(ch) => input.insert(ch),
            KeyCode::Backspace => input.backspace(),
            KeyCode::Delete => input.delete(),
            KeyCode::Left => input.left(),
            KeyCode::Right => input.right(),
            KeyCode::Home => input.home(),
            KeyCode::End => input.end(),
            _ => {}
        }
        true
    }

    pub fn handle_action(&mut self, action: Action) {
        match action {
            Action::Quit => {
                self.save_queue_state();
                self.player.send(PlayerCommand::Stop);
                self.should_quit = true;
            }
            Action::NextTab => self.cycle_tab(1),
            Action::PrevTab => self.cycle_tab(-1),
            Action::TabBack => {
                if let Some(previous) = self.tab_history.pop() {
                    self.tab = previous;
                    self.focus = self.panes()[0];
                }
            }
            Action::Tab(index) => {
                if let Some(tab) = Tab::ALL.get(index) {
                    self.go_to_tab(*tab);
                }
            }

            // Home is a horizontal row; up/down would have nowhere to go.
            Action::Up if self.focus == Pane::Home => {}
            Action::Down if self.focus == Pane::Home => {}
            Action::Up => self.move_selection(-1),
            Action::Down => self.move_selection(1),
            Action::PageUp => self.move_selection(-10),
            Action::PageDown => self.move_selection(10),
            Action::Top => self.move_selection(isize::MIN / 2),
            Action::Bottom => self.move_selection(isize::MAX / 2),
            // Home's mixes are drawn as a row, so they are navigated as one.
            // Home's mixes are drawn as a horizontal row, so Left/Right walk
            // them rather than cycling panes — until the cursor reaches the
            // end, where the next step continues into whatever is drawn there.
            Action::Left => match self.focus {
                Pane::Settings => self.adjust_setting(-1),
                Pane::Home if self.home_sel.index > 0 => self.move_selection(-1),
                _ => self.focus_by(-1),
            },
            Action::Right => match self.focus {
                Pane::Settings => self.adjust_setting(1),
                Pane::Home if self.home_sel.index + 1 < crate::ui::home::mix_count(self) => {
                    self.move_selection(1)
                }
                _ => self.focus_by(1),
            },
            // Unconditional pane cycling. Left/Right are spoken for on Home
            // (the mix row) and Settings (value editing), so these are the only
            // way to reach the side panes from the keyboard on those tabs.
            Action::FocusNext => self.cycle_focus(1),
            Action::FocusPrev => self.cycle_focus(-1),
            Action::Confirm => self.activate(),
            Action::Cancel => {
                self.show_help = false;
                if self.focus_mode {
                    // Goes through the setter so focus is handed back to the
                    // tab underneath rather than left on the lyrics.
                    self.set_focus_mode(false);
                }
                self.status_message = None;
            }

            Action::JumpToArtist => self.jump_to_current_artist(),
            Action::JumpToAlbum => self.jump_to_current_album(),

            Action::TogglePause => self.player.send(PlayerCommand::TogglePause),
            Action::Stop => self.player.send(PlayerCommand::Stop),
            Action::NextTrack => self.player.send(PlayerCommand::Next),
            Action::PrevTrack => self.player.send(PlayerCommand::Prev),
            Action::SeekForward => self.player.send(PlayerCommand::SeekForward),
            Action::SeekBackward => self.player.send(PlayerCommand::SeekBackward),
            Action::VolumeUp => self.player.send(PlayerCommand::AdjustVolume(VOLUME_STEP)),
            Action::VolumeDown => self.player.send(PlayerCommand::AdjustVolume(-VOLUME_STEP)),

            Action::AddToQueue => {
                let songs = self.selected_songs();
                if !songs.is_empty() {
                    self.status_message = Some(format!("Queued {} track(s)", songs.len()));
                    self.player.send(PlayerCommand::Enqueue(songs));
                    // Advance so repeated presses queue consecutive tracks.
                    self.move_selection(1);
                }
            }
            Action::RemoveFromQueue => {
                if self.focus == Pane::Settings {
                    // The same "remove the selected thing" gesture, applied to
                    // a music folder or a queue column.
                    self.delete_setting();
                } else if self.focus == Pane::PlaylistSongs {
                    self.remove_from_playlist();
                } else if self.tab == Tab::Queue || self.focus == Pane::Queue {
                    self.snapshot_queue();
                    self.player
                        .send(PlayerCommand::Remove(self.queue_sel.index));
                }
            }
            Action::ClearQueue => {
                self.snapshot_queue();
                // One command rather than clearing the queue here and stopping
                // separately: the player owns both halves of that transition.
                self.player.send(PlayerCommand::Clear);
                self.save_queue_state();
                self.status_message = Some("Queue cleared — C-z to undo".to_string());
            }
            Action::ToggleRepeat => self.player.send(PlayerCommand::ToggleRepeat),
            Action::ToggleShuffle => self.player.send(PlayerCommand::ToggleShuffle),

            Action::Refresh => {
                self.status_message = Some("Refreshing library…".to_string());
                self.bootstrap();
            }
            Action::ToggleHelp => self.show_help = !self.show_help,
            Action::ToggleQueuePane => {
                self.show_queue_pane = !self.show_queue_pane;
                self.ensure_focus_visible();
            }
            Action::ToggleCoverPane => self.show_cover_pane = !self.show_cover_pane,
            Action::ToggleLyricsPane => {
                // Whichever lyrics the user is actually looking at.
                if self.focus_mode {
                    self.show_focus_lyrics = !self.show_focus_lyrics;
                } else {
                    self.show_lyrics_pane = !self.show_lyrics_pane;
                }
                self.save_queue_state();
            }
            Action::ToggleVisualiser => self.show_visualiser = !self.show_visualiser,
            Action::ToggleRadio => {
                let enabled = !self.player.queue.lock().unwrap().radio;
                self.status_message = Some(if enabled {
                    "Radio mode on — queue extends automatically".to_string()
                } else {
                    "Radio mode off".to_string()
                });
                self.player.send(PlayerCommand::ToggleRadio);
            }
            Action::ToggleStar => {
                if let Some(song) = self.player.status().current {
                    let starred = !song.is_starred();
                    self.status_message = Some(if starred {
                        "Starred".into()
                    } else {
                        "Unstarred".to_string()
                    });
                    self.player.send(PlayerCommand::SetStarred {
                        song_id: song.id,
                        starred,
                    });
                }
            }

            // The side panes sit on the right, so the arrow moves the *divider*
            // rather than the panes: left widens them, right narrows them.
            // Doing it the other way round made the boundary travel opposite to
            // the key that was pressed.
            Action::ResizePaneLeft => self.nudge_side_panes(1),
            Action::ResizePaneRight => self.nudge_side_panes(-1),

            Action::LibraryModeNext => {
                if self.tab == Tab::Library {
                    self.cycle_library_mode(1);
                } else {
                    self.go_to_tab(Tab::Library);
                }
            }
            Action::LibraryModePrev => {
                if self.tab == Tab::Library {
                    self.cycle_library_mode(-1);
                } else {
                    self.go_to_tab(Tab::Library);
                }
            }
            Action::PlayNext => {
                let songs = self.selected_songs();
                if !songs.is_empty() {
                    let count = songs.len();
                    self.player.queue.lock().unwrap().insert_next(songs);
                    self.status_message = Some(format!("Playing {count} track(s) next"));
                    self.save_queue_state();
                }
            }
            Action::MoveTrackUp => self.move_queued_track(-1),
            Action::MoveTrackDown => self.move_queued_track(1),
            Action::AddToPlaylist => self.open_playlist_picker(),
            Action::Share => self.open_share(),
            Action::RatingUp => self.adjust_rating(1),
            Action::RatingDown => self.adjust_rating(-1),
            Action::OpenPalette => self.open_palette(),
            Action::UndoQueue => self.undo_queue(),
            Action::ToggleFocusMode => self.set_focus_mode(!self.focus_mode),
        }

        // Pane sizes are part of the saved layout; persist them as they change
        // rather than relying on a later save happening to pick them up.
        if matches!(action, Action::ResizePaneLeft | Action::ResizePaneRight) {
            self.save_queue_state();
        }
    }

    /// Route a mouse event using the hit map built during the last draw.
    pub fn handle_mouse(&mut self, event: crossterm::event::MouseEvent, hits: &Hits) {
        use crossterm::event::{MouseButton, MouseEventKind};

        match event.kind {
            MouseEventKind::ScrollDown => self.move_selection(3),
            MouseEventKind::ScrollUp => self.move_selection(-3),

            MouseEventKind::Down(MouseButton::Left) => {
                let Some(region) = hits.at(event.column, event.row) else {
                    return;
                };
                self.status_message = None;

                match region {
                    Region::Tab(index) => {
                        if let Some(tab) = Tab::ALL.get(index) {
                            self.go_to_tab(*tab);
                        }
                    }
                    Region::LibraryMode(index) => {
                        if let Some(mode) = LibraryMode::ALL.get(index) {
                            self.set_library_mode(*mode);
                        }
                    }
                    Region::PlayPause => self.player.send(PlayerCommand::TogglePause),
                    Region::Repeat => self.player.send(PlayerCommand::ToggleRepeat),
                    Region::Shuffle => self.player.send(PlayerCommand::ToggleShuffle),
                    Region::CurrentArtist => self.jump_to_current_artist(),
                    Region::CurrentAlbum => self.jump_to_current_album(),
                    Region::Seek => {
                        self.active_drag = Some(Region::Seek);
                        self.update_drag_ratio(event.column, hits, Region::Seek);
                    }
                    Region::Volume => {
                        self.active_drag = Some(Region::Volume);
                        self.update_drag_ratio(event.column, hits, Region::Volume);
                    }
                    Region::Row { pane, index } => {
                        self.focus = pane;
                        // Only synced lyrics carry timestamps; clicking an
                        // unsynced line would otherwise seek to zero.
                        if pane == Pane::Lyrics {
                            if self.lyrics.synced {
                                if let Some(line) = self.lyrics.lines.get(index) {
                                    self.player.send(PlayerCommand::SeekTo(line.at));
                                }
                            } else {
                                self.select_in(Pane::Lyrics, index);
                            }
                        } else {
                            self.select_in(pane, index);

                            let now = Instant::now();
                            let is_double = matches!(
                                self.last_click,
                                Some((last, at)) if last == region && now.duration_since(at) < DOUBLE_CLICK
                            );
                            self.last_click = Some((region, now));
                            if is_double {
                                self.activate();
                            }
                        }
                    }
                }
            }

            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(drag_region) = self.active_drag {
                    self.update_drag_ratio(event.column, hits, drag_region);
                }
            }

            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(drag_region) = self.active_drag.take() {
                    match drag_region {
                        Region::Seek => {
                            let Some(song) = self.player.status().current else {
                                return;
                            };
                            let target =
                                Duration::from_secs_f64(song.duration as f64 * self.drag_ratio);
                            self.player.send(PlayerCommand::SeekTo(target));
                        }
                        Region::Volume => {
                            self.player
                                .send(PlayerCommand::SetVolume(self.drag_ratio as f32));
                        }
                        _ => {}
                    }
                }
            }

            _ => {}
        }
    }

    fn update_drag_ratio(&mut self, column: u16, hits: &Hits, region: Region) {
        let Some(rect) = hits.rect_of(region) else {
            return;
        };
        // Both sliders fill their registered rect exactly, so the click maps
        // straight through with no padding to compensate for.
        if rect.width == 0 {
            return;
        }
        let offset = column.saturating_sub(rect.x);
        self.drag_ratio = crate::ui::widgets::Slider::ratio_at(offset, rect.width);
    }

    /// Whether the app has nothing to play from yet.
    pub fn is_unconfigured(&self) -> bool {
        let no_server = !self.config.server.enabled
            || self.config.server.url.trim().is_empty()
            || self.config.server.username.trim().is_empty();
        no_server && self.config.local.paths.is_empty()
    }

    /// Open the first-run chooser, if there is nothing set up yet.
    pub fn maybe_start_setup(&mut self) {
        if self.is_unconfigured() && self.overlay.is_none() {
            self.overlay = Some(Overlay::Setup(Default::default()));
        }
    }

    /// Act on the first-run choice: land on the settings row that starts the
    /// chosen path, so the next keypress is already the useful one.
    fn begin_setup(&mut self, choice: usize) {
        use crate::ui::settings::SettingItem;
        // 1 is "local folder"; 0 and 2 both begin with the server.
        let target = if choice == 1 {
            SettingItem::AddLocalPath
        } else {
            SettingItem::ServerUrl
        };

        self.go_to_tab(Tab::Settings);
        self.focus = Pane::Settings;
        if let Some(index) = crate::ui::settings::rows(&self.config)
            .iter()
            .position(|item| *item == target)
        {
            self.settings_sel.index = index;
        }
        self.status_message = Some(match choice {
            1 => "Press Enter to type the path to your music folder".to_string(),
            _ => "Press Enter to type your server URL, then fill in the rows below".to_string(),
        });
        // Open the field straight away: the user already said what they want.
        self.activate_setting();
    }

    /// The settings row currently selected.
    pub fn selected_setting(&self) -> Option<crate::ui::settings::SettingItem> {
        let rows = crate::ui::settings::rows(&self.config);
        rows.get(self.settings_sel.index.min(rows.len().saturating_sub(1)))
            .copied()
    }

    /// Left/Right on a settings row: cycle or nudge the value in place.
    pub fn adjust_setting(&mut self, delta: isize) {
        use crate::ui::settings::SettingItem;
        let Some(item) = self.selected_setting() else {
            return;
        };

        match item {
            SettingItem::ServerEnabled => {
                self.config.server.enabled = !self.config.server.enabled;
                let _ = self.config.save();
                self.apply_server_config();
            }
            SettingItem::StreamFormat => {
                let formats = [None, Some("mp3"), Some("opus"), Some("flac")];
                let current = formats
                    .iter()
                    .position(|f| f.map(String::from) == self.config.server.format)
                    .unwrap_or(0) as isize;
                let next = (current + delta).rem_euclid(formats.len() as isize) as usize;
                self.config.server.format = formats[next].map(String::from);
                let _ = self.config.save();
                // The format is baked into the stream URL, so the client has to
                // be rebuilt for the change to take effect.
                self.apply_server_config();
                self.status_message = Some(format!(
                    "Stream format: {}",
                    self.config.server.format.as_deref().unwrap_or("raw")
                ));
            }

            SettingItem::ScanOnStart => {
                self.config.local.scan_on_start = !self.config.local.scan_on_start;
                let _ = self.config.save();
            }

            SettingItem::ThemePreset => self.cycle_theme_preset(delta),
            SettingItem::Glyphs => {
                use crate::ui::glyphs::GlyphSet;
                let sets = [GlyphSet::Nerd, GlyphSet::Unicode, GlyphSet::Ascii];
                let current = sets
                    .iter()
                    .position(|s| *s == self.config.glyphs)
                    .unwrap_or(0) as isize;
                let next = (current + delta).rem_euclid(sets.len() as isize) as usize;
                self.config.glyphs = sets[next];
                let _ = self.config.save();
                self.status_message = Some(format!("Icons: {:?}", self.config.glyphs));
            }
            SettingItem::CoverWidth => {
                self.cover_percent = if delta > 0 {
                    (self.cover_percent + 2).min(60)
                } else {
                    self.cover_percent.saturating_sub(2).max(15)
                };
                self.save_queue_state();
            }
            SettingItem::QueueWidth => {
                self.queue_percent = if delta > 0 {
                    (self.queue_percent + 2).min(50)
                } else {
                    self.queue_percent.saturating_sub(2).max(10)
                };
                self.save_queue_state();
            }
            SettingItem::ShowCover => {
                self.show_cover_pane = !self.show_cover_pane;
                self.save_queue_state();
            }
            SettingItem::ShowQueue => {
                self.show_queue_pane = !self.show_queue_pane;
                self.ensure_focus_visible();
                self.save_queue_state();
            }
            SettingItem::ShowLyrics => {
                self.show_lyrics_pane = !self.show_lyrics_pane;
                self.save_queue_state();
            }

            SettingItem::VolumeScale => {
                self.config.volume_log = !self.config.volume_log;
                let _ = self.config.save();
                self.status_message = Some(format!(
                    "Volume scaling: {}",
                    if self.config.volume_log {
                        "Logarithmic (perceptual)"
                    } else {
                        "Linear"
                    }
                ));
            }
            SettingItem::BufferSeconds => {
                // Bounded well away from zero: too small a buffer underruns on
                // any network hiccup, too large makes seeking feel unresponsive.
                let next = self.config.buffer_seconds + if delta > 0 { 0.5 } else { -0.5 };
                self.config.buffer_seconds = next.clamp(1.0, 30.0);
                let _ = self.config.save();
                self.status_message = Some("Audio buffer applies on the next start".to_string());
            }
            SettingItem::AutoMix => {
                // Through the player, so switching it on fills the queue right
                // away rather than waiting for the next track to end.
                let active = !self.player.queue.lock().unwrap().radio;
                self.player.send(PlayerCommand::ToggleRadio);
                self.save_queue_state();
                self.status_message =
                    Some(format!("Auto-Mix: {}", if active { "ON" } else { "OFF" }));
            }

            SettingItem::DiscordEnabled => {
                self.config.discord.enabled = !self.config.discord.enabled;
                let _ = self.config.save();
                self.status_message = Some(format!(
                    "Discord Rich Presence: {} (restart to apply)",
                    if self.config.discord.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    }
                ));
            }
            SettingItem::DiscordCoverArt => {
                self.config.discord.cover_art = !self.config.discord.cover_art;
                let _ = self.config.save();
            }

            SettingItem::QueueColumn(index) => {
                if let Some(column) = self.config.queue_columns.get_mut(index) {
                    let width = column.width as isize + delta * 5;
                    column.width = width.clamp(5, 90) as u16;
                    let _ = self.config.save();
                }
            }

            // Rows whose only interaction is Enter.
            SettingItem::ServerUrl
            | SettingItem::ServerUsername
            | SettingItem::ServerPassword
            | SettingItem::TestConnection
            | SettingItem::LocalPath(_)
            | SettingItem::AddLocalPath
            | SettingItem::LocalPlaylistDir
            | SettingItem::Rescan
            | SettingItem::ClearQueue
            | SettingItem::DiscordClientId
            | SettingItem::AddQueueColumn
            | SettingItem::ShowKeybindings => {}
        }
    }

    /// Enter on a settings row: open a text field, or run the row's action.
    pub fn activate_setting(&mut self) {
        use crate::ui::settings::SettingItem;
        let Some(item) = self.selected_setting() else {
            return;
        };

        if item.is_text() {
            let current = match item {
                // Never pre-fill the password field from the keyring: the panel
                // shows that one is stored without ever handling the secret.
                SettingItem::ServerPassword => String::new(),
                SettingItem::ServerUrl => self.config.server.url.clone(),
                SettingItem::ServerUsername => self.config.server.username.clone(),
                SettingItem::LocalPath(index) => self
                    .config
                    .local
                    .paths
                    .get(index)
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                SettingItem::AddLocalPath => String::new(),
                SettingItem::LocalPlaylistDir => self
                    .config
                    .local
                    .playlist_dir
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                SettingItem::DiscordClientId => self.config.discord.client_id.clone(),
                _ => String::new(),
            };
            self.settings_edit =
                Some(crate::ui::widgets::TextInput::new(current).masked(item.is_secret()));
            return;
        }

        match item {
            SettingItem::TestConnection => self.test_connection(),
            SettingItem::Rescan => self.rescan_local_library(),
            SettingItem::ClearQueue => {
                self.snapshot_queue();
                // One command rather than clearing the queue here and stopping
                // separately: the player owns both halves of that transition.
                self.player.send(PlayerCommand::Clear);
                self.save_queue_state();
                self.status_message = Some("Queue cleared — C-z to undo".to_string());
            }
            SettingItem::ShowKeybindings => self.show_help = true,
            SettingItem::QueueColumn(index) => {
                use crate::config::ColumnKind;
                const KINDS: &[ColumnKind] = &[
                    ColumnKind::Artist,
                    ColumnKind::Title,
                    ColumnKind::Album,
                    ColumnKind::Length,
                    ColumnKind::Track,
                    ColumnKind::Year,
                ];
                if let Some(column) = self.config.queue_columns.get_mut(index) {
                    let current = KINDS.iter().position(|k| *k == column.kind).unwrap_or(0);
                    column.kind = KINDS[(current + 1) % KINDS.len()];
                    let _ = self.config.save();
                }
            }
            SettingItem::AddQueueColumn => {
                use crate::config::{Column, ColumnKind};
                self.config.queue_columns.push(Column {
                    kind: ColumnKind::Year,
                    width: 10,
                });
                let _ = self.config.save();
            }
            // Toggles are reachable with Enter as well as Left/Right, which is
            // what the on-screen hint promises.
            _ => self.adjust_setting(1),
        }
    }

    /// Delete on a settings row: remove the per-item entry it stands for.
    pub fn delete_setting(&mut self) {
        use crate::ui::settings::SettingItem;
        let Some(item) = self.selected_setting() else {
            return;
        };
        match item {
            SettingItem::LocalPath(index) if index < self.config.local.paths.len() => {
                let removed = self.config.local.paths.remove(index);
                let _ = self.config.save();
                self.apply_local_config();
                self.status_message = Some(format!("Removed {}", removed.display()));
            }
            SettingItem::QueueColumn(index) if index < self.config.queue_columns.len() => {
                // Never leave the queue with no columns at all; there would be
                // nothing to click on and no way to add one back.
                if self.config.queue_columns.len() > 1 {
                    self.config.queue_columns.remove(index);
                    let _ = self.config.save();
                } else {
                    self.status_message = Some("The queue needs at least one column".to_string());
                }
            }
            SettingItem::ServerPassword => {
                let _ = crate::paths::delete_keyring_password(&self.config.server.username);
                self.has_stored_password = false;
                self.status_message = Some("Removed the stored password".to_string());
            }
            _ => {}
        }
        let len = crate::ui::settings::rows(&self.config).len();
        self.settings_sel.clamp(len);
    }

    /// Enter in an open settings text field: store what was typed.
    pub fn commit_setting_edit(&mut self) {
        use crate::ui::settings::SettingItem;
        let Some(input) = self.settings_edit.take() else {
            return;
        };
        let Some(item) = self.selected_setting() else {
            return;
        };
        let value = input.value().trim().to_string();

        match item {
            SettingItem::ServerUrl => {
                // Stored without a trailing slash so the client's `{base}/rest/…`
                // never produces a double slash.
                self.config.server.url = value.trim_end_matches('/').to_string();
                let _ = self.config.save();
                self.apply_server_config();
            }
            SettingItem::ServerUsername => {
                self.config.server.username = value;
                let _ = self.config.save();
                self.refresh_password_state();
                self.apply_server_config();
            }
            SettingItem::ServerPassword => {
                if value.is_empty() {
                    self.status_message = Some("Password unchanged".to_string());
                    return;
                }
                if self.config.server.username.trim().is_empty() {
                    self.status_message = Some("Set a username first".to_string());
                    return;
                }
                // Into the OS keyring, never into config.toml.
                match crate::paths::store_keyring_password(&self.config.server.username, &value) {
                    Ok(()) => {
                        // Any plaintext password left in the config is now
                        // redundant, and keeping it would silently win over the
                        // keyring entry the user just set.
                        self.config.server.password.clear();
                        let _ = self.config.save();
                        self.has_stored_password = true;
                        self.status_message = Some("Password stored in the keyring".to_string());
                        self.apply_server_config();
                    }
                    Err(err) => {
                        self.status_message = Some(format!("Could not store password: {err:#}"))
                    }
                }
            }
            SettingItem::LocalPath(index) => {
                if value.is_empty() {
                    return;
                }
                if let Some(slot) = self.config.local.paths.get_mut(index) {
                    *slot = expand_home(&value);
                    let _ = self.config.save();
                    self.apply_local_config();
                }
            }
            SettingItem::AddLocalPath => {
                if value.is_empty() {
                    return;
                }
                let path = expand_home(&value);
                if !path.is_dir() {
                    self.status_message = Some(format!("{} is not a folder", path.display()));
                    return;
                }
                self.config.local.paths.push(path);
                let _ = self.config.save();
                self.apply_local_config();
                self.rescan_local_library();
            }
            SettingItem::LocalPlaylistDir => {
                self.config.local.playlist_dir = (!value.is_empty()).then(|| expand_home(&value));
                let _ = self.config.save();
                self.apply_local_config();
            }
            SettingItem::DiscordClientId => {
                self.config.discord.client_id = value;
                let _ = self.config.save();
                self.status_message =
                    Some("Discord application ID saved (restart to apply)".into());
            }
            _ => {}
        }
    }

    /// Note whether a password is available, without reading the secret.
    pub fn refresh_password_state(&mut self) {
        self.has_stored_password = !self.config.server.password.is_empty()
            || self
                .config
                .password()
                .map(|p| !p.is_empty())
                .unwrap_or(false);
    }

    /// Rebuild the Subsonic backend from the current config and swap it in.
    ///
    /// Nothing downstream is touched: `App`, the player task and Discord all
    /// hold the same `MergedLibrary`, so this takes effect immediately and
    /// without a restart.
    pub fn apply_server_config(&mut self) {
        let Some(root) = self.library_root.clone() else {
            return;
        };
        match crate::build_remote(&self.config) {
            Ok(remote) => {
                root.set_remote(remote);
                self.connection_status = None;
                self.invalidate_library();
            }
            Err(err) => self.status_message = Some(format!("Server settings: {err:#}")),
        }
    }

    /// Rebuild the local backend from the current config and swap it in.
    pub fn apply_local_config(&mut self) {
        let Some(root) = self.library_root.clone() else {
            return;
        };
        if self.config.local.paths.is_empty() {
            root.set_local(None);
        } else if let Some(local) = root.local() {
            local.set_playlist_dir(self.config.local.playlist_dir.clone());
        } else {
            root.set_local(Some(Arc::new(crate::library::LocalLibrary::from_cache(
                self.config.local.playlist_dir.clone(),
            ))));
        }
        self.invalidate_library();
    }

    /// Drop the cached library views so the next visit to a tab refetches.
    ///
    /// Called after a backend changes, because every list on screen came from
    /// the old one and would otherwise keep showing a library that is no longer
    /// configured.
    pub fn invalidate_library(&mut self) {
        // Artists, albums and playlists are reloaded whenever their list is
        // empty, so clearing them is what marks them stale; tracks and
        // favourites carry an explicit flag.
        self.artists.clear();
        self.albums.clear();
        self.playlists.clear();
        self.tracks.clear();
        self.tracks_loaded = false;
        self.favorites.clear();
        self.favorites_loaded = false;
        self.bootstrap();
    }

    fn test_connection(&mut self) {
        self.connection_status = Some("checking…".to_string());
        match crate::build_remote(&self.config) {
            Ok(None) => {
                self.connection_status =
                    Some("no server configured (set a URL and username)".to_string())
            }
            Err(err) => self.connection_status = Some(format!("failed: {err:#}")),
            Ok(Some(remote)) => {
                let loads = self.loads.clone();
                tokio::spawn(async move {
                    let result = match remote.client().ping().await {
                        Ok(version) => format!("OK — server version {version}"),
                        Err(err) => format!("failed: {err:#}"),
                    };
                    let _ = loads.send(LoadEvent::ConnectionTested(result));
                });
            }
        }
    }

    pub fn rescan_local_library(&mut self) {
        if self.config.local.paths.is_empty() {
            self.scan_status = Some("add a music folder first".to_string());
            return;
        }
        let Some(root) = self.library_root.clone() else {
            return;
        };
        // Make sure a backend exists to receive the scan.
        self.apply_local_config();
        let Some(local) = root.local() else { return };

        let roots = self.config.local.paths.clone();
        let loads = self.loads.clone();
        self.scan_status = Some("scanning…".to_string());

        tokio::task::spawn_blocking(move || {
            let previous = local.index();
            let index = crate::library::local::scan::scan(&roots, &previous, |_| {});
            let songs = index.tracks.len();
            let albums = index.albums().len();
            let _ = index.save();
            local.set_index(index);
            let _ = loads.send(LoadEvent::LocalScanned { songs, albums });
        });
    }

    pub fn cycle_theme_preset(&mut self, delta: isize) {
        let names = Theme::PRESET_NAMES;
        let current = self
            .config
            .theme_preset
            .as_deref()
            .and_then(|active| names.iter().position(|&n| n == active))
            .unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(names.len() as isize) as usize;
        let preset_name = names[next];
        self.config.theme = Theme::preset(preset_name);
        self.config.theme_preset = Some(preset_name.to_string());
        let _ = self.config.save();
        self.status_message = Some(format!("Theme set to {preset_name}"));
    }

    pub fn jump_to_current_artist(&mut self) {
        let status = self.player.status();
        let Some(song) = status.current.as_ref() else {
            return;
        };
        let artist_name = song.artist_or_unknown();
        if let Some(pos) = self.artists.iter().position(|a| {
            a.id == song.artist_id.clone().unwrap_or_default()
                || a.name.eq_ignore_ascii_case(artist_name)
        }) {
            self.go_to_library(LibraryMode::Artists);
            self.artist_sel.index = pos;
            let artist_id = self.artists[pos].id.clone();
            self.load_artist_albums(artist_id);
            self.status_message = Some(format!("Jumped to artist: {artist_name}"));
        } else {
            self.status_message = Some(format!("Artist '{artist_name}' not found in library"));
        }
    }

    pub fn jump_to_current_album(&mut self) {
        let status = self.player.status();
        let Some(song) = status.current.as_ref() else {
            return;
        };
        let album_title = song.album_or_unknown();
        if let Some(pos) = self.albums.iter().position(|a| {
            a.id == song.album_id.clone().unwrap_or_default()
                || a.name.eq_ignore_ascii_case(album_title)
        }) {
            self.go_to_library(LibraryMode::Albums);
            self.album_sel.index = pos;
            let album_id = self.albums[pos].id.clone();
            self.load_album_songs(album_id);
            self.status_message = Some(format!("Jumped to album: {album_title}"));
        } else {
            self.status_message = Some(format!("Album '{album_title}' not found in library"));
        }
    }

    fn go_to_tab(&mut self, tab: Tab) {
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

    fn cycle_tab(&mut self, delta: isize) {
        let current = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0) as isize;
        let count = Tab::ALL.len() as isize;
        let next = (current + delta).rem_euclid(count) as usize;
        self.go_to_tab(Tab::ALL[next]);
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
    fn cycle_focus(&mut self, delta: isize) {
        let panes = self.panes();
        if panes.is_empty() {
            return;
        }
        let current = panes.iter().position(|p| *p == self.focus).unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(panes.len() as isize) as usize;
        self.focus = panes[next];
    }

    fn focus_by(&mut self, delta: isize) {
        let panes = self.panes();
        let current = panes.iter().position(|p| *p == self.focus).unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, panes.len() as isize - 1) as usize;
        self.focus = panes[next];
    }

    /// Length of the list backing a pane.
    fn pane_len(&self, pane: Pane) -> usize {
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
            Pane::Settings => crate::ui::settings::rows(&self.config).len(),
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

    fn selection_mut(&mut self, pane: Pane) -> &mut Selection {
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
            Pane::Settings => &mut self.settings_sel,
            Pane::Home => &mut self.home_sel,
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let pane = self.focus;
        let len = self.pane_len(pane);
        let before = self.selection_mut(pane).index;
        self.selection_mut(pane).move_by(delta, len);
        let after = self.selection_mut(pane).index;
        if before != after {
            self.on_selection_changed(pane);
        }
    }

    fn select_in(&mut self, pane: Pane, index: usize) {
        let len = self.pane_len(pane);
        let before = self.selection_mut(pane).index;
        self.selection_mut(pane).set(index, len);
        if before != self.selection_mut(pane).index {
            self.on_selection_changed(pane);
        }
    }

    /// Load the dependent pane when a parent selection changes.
    fn on_selection_changed(&mut self, pane: Pane) {
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
    fn focused_songs(&self) -> (Vec<Song>, usize) {
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
            // Nothing selectable: lyrics, settings & home track their own state.
            Pane::Lyrics | Pane::Settings | Pane::Home => (Vec::new(), 0),
        }
    }

    /// Songs the current selection refers to, for enqueueing.
    fn selected_songs(&self) -> Vec<Song> {
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

    fn activate(&mut self) {
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
    fn target_songs(&self) -> Vec<Song> {
        let selected = self.selected_songs();
        if !selected.is_empty() {
            return selected;
        }
        self.player.status().current.into_iter().collect()
    }

    pub fn open_share(&mut self) {
        let songs = self.target_songs();
        if songs.is_empty() {
            self.status_message = Some("Nothing to share".to_string());
            return;
        }
        // Say why up front rather than opening a dialog that can only fail:
        // a local file has no public URL, and sharing needs a server.
        if songs
            .iter()
            .any(|song| crate::library::is_local_id(&song.id))
        {
            self.status_message =
                Some("Local files cannot be shared — there is no server to serve them".to_string());
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
    fn handle_overlay_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
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

    fn submit_share(&mut self) {
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

    fn submit_playlist_add(&mut self) {
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
    fn remove_from_playlist(&mut self) {
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
    fn apply_palette_songs(&mut self, generation: u64, songs: Vec<Song>) {
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

    fn search_for_palette(&mut self) {
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

    fn run_palette_choice(&mut self) {
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
    fn snapshot_queue(&mut self) {
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

    fn undo_queue(&mut self) {
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
    fn adjust_rating(&mut self, delta: i8) {
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

    fn move_queued_track(&mut self, delta: isize) {
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
    fn start_mix(&mut self, index: usize) {
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

    /// Which lyric line the view should centre on when there is no timing to
    /// follow.
    pub fn lyrics_scroll_target(&self) -> usize {
        self.lyrics_sel
            .index
            .min(self.lyrics.lines.len().saturating_sub(1))
    }

    /// Statistics including the track playing right now, so the Home counters
    /// tick during a track rather than only jumping when it ends.
    pub fn live_stats(&self) -> crate::history::Stats {
        if self.player.status().current.is_none() {
            return self.stats.clone();
        }
        let secs = self.player.elapsed().as_secs();
        // The play started `secs` ago, so that is the hour bucket it belongs to.
        let hour = ((crate::history::now() - secs as i64).rem_euclid(86_400) / 3600) as usize;
        self.stats.with_in_progress(secs, hour)
    }

    /// Enter or leave the full-screen now-playing view.
    ///
    /// Focus moves to the lyrics while it is open. That is the only place they
    /// can hold the cursor now that they have no tab of their own, and it makes
    /// up/down do the obvious thing: scroll the words you are reading.
    pub fn set_focus_mode(&mut self, on: bool) {
        self.focus_mode = on;
        self.focus = if on { Pane::Lyrics } else { self.panes()[0] };
        self.status_message = Some(if on {
            "Focus mode — F or Esc to leave".to_string()
        } else {
            "Focus mode off".to_string()
        });
    }

    /// Widen (`+1`) or narrow (`-1`) the side panes by one step.
    /// Move the dividers one step.
    ///
    /// Both side panes move, so the content area between them changes by twice
    /// `PANE_STEP` per press — which is why that constant is half of what one
    /// press should feel like.
    fn nudge_side_panes(&mut self, direction: i8) {
        self.cover_percent = step_percent(self.cover_percent, direction);
        self.queue_percent = step_percent(self.queue_percent, direction);
    }

    // ---- pane size tweening ---------------------------------------------

    /// Advance the pane-size animations one frame, and report the sizes to draw
    /// with. Called once per frame from the renderer.
    pub fn tween_panes(&mut self) -> (u16, u16) {
        fn ease(current: &mut f32, target: u16) -> u16 {
            let target = target as f32;
            if (target - *current).abs() < PANE_EPSILON {
                *current = target;
            } else {
                *current += (target - *current) * PANE_EASE;
            }
            current.round().max(0.0) as u16
        }
        (
            ease(&mut self.eased_cover, self.cover_percent),
            ease(&mut self.eased_queue, self.queue_percent),
        )
    }

    fn panes_are_tweening(&self) -> bool {
        (self.eased_cover - self.cover_percent as f32).abs() >= PANE_EPSILON
            || (self.eased_queue - self.queue_percent as f32).abs() >= PANE_EPSILON
    }

    /// Whether anything on screen is animating and therefore needs redrawing.
    ///
    /// The main loop only wakes on a timer while this is true, so an animation
    /// that is not listed here simply will not run.
    pub fn is_animating(&self) -> bool {
        // Home's live counters only move while a track is playing, which the
        // first condition already covers.
        (self.player.status().playing && !self.player.is_paused())
            || self.panes_are_tweening()
            || self.lyrics_are_scrolling()
    }

    /// Hand-scrolled lyrics ease toward the cursor, which needs frames to
    /// finish; without this the glide stops wherever the last keypress left it.
    fn lyrics_are_scrolling(&self) -> bool {
        !self.lyrics.synced
            && !self.lyrics.lines.is_empty()
            && (self.lyrics_scroll - self.lyrics_scroll_target() as f32).abs() >= 0.01
    }
}

/// Put text on the system clipboard via OSC 52.
///
/// The terminal itself does the copying, so this works over SSH and needs no
/// clipboard library. Terminals that ignore the sequence simply do nothing —
/// which is why the share popup keeps showing the URL either way.
fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    let encoded = base64(text.as_bytes());
    let mut stdout = std::io::stdout();
    let _ = write!(stdout, "\x1b]52;c;{encoded}\x07");
    let _ = stdout.flush();
}

fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(ALPHABET[(n >> 18 & 63) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// A song's effective rating: what this session set, else what the server
/// reported, else unrated.
///
/// The local value has to win: `setRating` is fire-and-forget, and the server's
/// copy of the song is not refetched until the list it came from reloads, so
/// without this the stars would flick back to the old value.
fn merge_rating(local: Option<u8>, server: Option<u8>) -> u8 {
    local.or(server).unwrap_or(0).min(5)
}

/// Bounds a side pane can be resized between, as a percentage of the width.
const PANE_MIN_PERCENT: u16 = 10;
const PANE_MAX_PERCENT: u16 = 45;
/// Per-pane step. Both side panes move together on a resize, so the content
/// area between them changes by twice this — one press, one visible step.
const PANE_STEP: u16 = 1;

/// One resize step, clamped so a pane can never vanish or swallow the screen.
fn step_percent(current: u16, direction: i8) -> u16 {
    if direction >= 0 {
        (current + PANE_STEP).min(PANE_MAX_PERCENT)
    } else {
        current.saturating_sub(PANE_STEP).max(PANE_MIN_PERCENT)
    }
}

/// The focusable panes of a tab, left to right.
///
/// Free-standing and pure so the focus order can be tested without an `App`,
/// which needs an audio device to exist.
pub fn focus_order(tab: Tab, library_mode: LibraryMode, side_queue: bool) -> Vec<Pane> {
    let mut panes: Vec<Pane> = match tab {
        Tab::Home => vec![Pane::Home],
        Tab::Queue => vec![Pane::Queue],
        Tab::Library => library_mode.panes().to_vec(),
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
///
/// Users type `~/Music`; nothing in `std` expands it, and a literal `~`
/// directory is not what they meant.
fn expand_home(input: &str) -> std::path::PathBuf {
    let trimmed = input.trim();
    if let Some(rest) = trimmed.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return std::path::PathBuf::from(home).join(rest);
    }
    if trimmed == "~"
        && let Some(home) = std::env::var_os("HOME")
    {
        return std::path::PathBuf::from(home);
    }
    std::path::PathBuf::from(trimmed)
}

/// Format a duration as `m:ss`, or `h:mm:ss` past an hour.
pub fn format_duration(duration: Duration) -> String {
    let total = duration.as_secs();
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Up Next pane used to be reachable only by clicking it: it was drawn
    /// beside every tab but was in no tab's focus order.
    #[test]
    fn the_side_queue_is_the_last_stop_when_cycling_right() {
        for tab in [Tab::Home, Tab::Library, Tab::Settings] {
            let panes = focus_order(tab, LibraryMode::Artists, true);
            assert_eq!(
                panes.last(),
                Some(&Pane::Queue),
                "{tab:?} should end at the Up Next pane"
            );
            assert!(panes.len() > 1, "{tab:?} should have somewhere to come back to");
        }
    }

    #[test]
    fn a_hidden_side_queue_is_not_focusable() {
        for tab in [Tab::Home, Tab::Library, Tab::Settings] {
            assert!(
                !focus_order(tab, LibraryMode::Artists, false).contains(&Pane::Queue),
                "{tab:?} must not focus a pane that is not drawn"
            );
        }
    }

    /// The Queue tab draws the queue as its content, so the side pane is
    /// suppressed there and must not appear twice in the focus order.
    #[test]
    fn the_queue_tab_lists_the_queue_once() {
        let panes = focus_order(Tab::Queue, LibraryMode::Artists, false);
        assert_eq!(panes, vec![Pane::Queue]);
    }

    /// Every Library view keeps its own panes and gains the queue at the end.
    #[test]
    fn library_views_keep_their_panes_and_gain_the_queue() {
        let modes = [
            LibraryMode::Artists,
            LibraryMode::Albums,
            LibraryMode::Tracks,
            LibraryMode::Playlists,
            LibraryMode::Favorites,
        ];
        for mode in modes {
            let without = focus_order(Tab::Library, mode, false);
            let with = focus_order(Tab::Library, mode, true);
            assert_eq!(without, mode.panes().to_vec());
            assert_eq!(with[..without.len()], without[..]);
            assert_eq!(with.last(), Some(&Pane::Queue));
        }
    }

    #[test]
    fn formats_short_and_long_durations() {
        assert_eq!(format_duration(Duration::from_secs(0)), "0:00");
        assert_eq!(format_duration(Duration::from_secs(75)), "1:15");
        assert_eq!(format_duration(Duration::from_secs(255)), "4:15");
        assert_eq!(format_duration(Duration::from_secs(3661)), "1:01:01");
    }

    #[test]
    fn selection_stays_within_bounds() {
        let mut sel = Selection::default();
        sel.move_by(5, 3);
        assert_eq!(sel.index, 2, "clamps to the last item");
        sel.move_by(-10, 3);
        assert_eq!(sel.index, 0, "clamps to the first item");
    }

    #[test]
    fn selection_on_an_empty_list_stays_at_zero() {
        let mut sel = Selection::default();
        sel.move_by(3, 0);
        assert_eq!(sel.index, 0);
    }

    #[test]
    fn clamp_pulls_selection_back_when_the_list_shrinks() {
        let mut sel = Selection {
            index: 9,
            offset: 0,
        };
        sel.clamp(4);
        assert_eq!(sel.index, 3);
        sel.clamp(0);
        assert_eq!(sel.index, 0);
    }

    #[test]
    fn every_library_mode_declares_at_least_one_pane() {
        for mode in LibraryMode::ALL {
            assert!(!mode.panes().is_empty(), "{} has no panes", mode.title());
        }
    }

    /// Lyrics lost their own tab, so focus mode is the only place they can hold
    /// the cursor — which is what makes hand-scrolling unsynced lyrics
    /// reachable. If they ever reappear in a browsable pane list, revisit
    /// `set_focus_mode`, which currently assumes it is the sole route.
    #[test]
    fn lyrics_are_not_a_browsable_pane() {
        for mode in LibraryMode::ALL {
            assert!(
                !mode.panes().contains(&Pane::Lyrics),
                "{} lists the lyrics pane",
                mode.title()
            );
        }
    }

    #[test]
    fn a_rating_set_this_session_wins_over_the_servers_copy() {
        assert_eq!(merge_rating(Some(4), Some(2)), 4);
        assert_eq!(
            merge_rating(None, Some(2)),
            2,
            "server value when unset here"
        );
        assert_eq!(merge_rating(Some(0), Some(5)), 0, "clearing must stick");
        assert_eq!(merge_rating(None, None), 0);
    }

    #[test]
    fn a_nonsense_rating_from_the_server_cannot_overflow_the_stars() {
        // Stars are rendered by repeating a glyph `rating` times.
        assert_eq!(merge_rating(None, Some(99)), 5);
    }

    /// The side panes are drawn on the right, so widening them moves the
    /// divider left. Pressing `M-→` must therefore *narrow* them, or the
    /// boundary travels opposite to the arrow.
    #[test]
    fn the_divider_follows_the_arrow_key() {
        let start = 25;
        // M-← widens the side panes, pushing the divider left.
        assert!(step_percent(start, 1) > start);
        // M-→ narrows them, letting the content take the space back.
        assert!(step_percent(start, -1) < start);
    }

    #[test]
    fn resizing_stops_at_the_bounds() {
        let mut narrow = PANE_MIN_PERCENT;
        for _ in 0..20 {
            narrow = step_percent(narrow, -1);
        }
        assert_eq!(narrow, PANE_MIN_PERCENT, "a pane must not vanish");

        let mut wide = PANE_MAX_PERCENT;
        for _ in 0..20 {
            wide = step_percent(wide, 1);
        }
        assert_eq!(wide, PANE_MAX_PERCENT, "a pane must not swallow the screen");
    }

    #[test]
    fn base64_matches_the_reference_encoding() {
        // Padding is the part that is easy to get wrong, so cover all three
        // input lengths mod 3.
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(
            base64(b"https://music.example.com/share/AbC1"),
            "aHR0cHM6Ly9tdXNpYy5leGFtcGxlLmNvbS9zaGFyZS9BYkMx"
        );
    }

    #[test]
    fn hit_map_returns_the_topmost_region() {
        let mut hits = Hits::default();
        let area = ratatui::layout::Rect::new(0, 0, 10, 2);
        hits.push(area, Region::Seek);
        hits.push(area, Region::Volume);
        // Later pushes win, so overlays take precedence over what is beneath.
        assert_eq!(hits.at(5, 1), Some(Region::Volume));
        assert_eq!(hits.at(50, 50), None);
    }

    #[test]
    fn hit_map_finds_a_regions_rect() {
        let mut hits = Hits::default();
        let area = ratatui::layout::Rect::new(3, 4, 10, 1);
        hits.push(area, Region::Seek);
        assert_eq!(hits.rect_of(Region::Seek), Some(area));
        assert_eq!(hits.rect_of(Region::Volume), None);
    }
}
