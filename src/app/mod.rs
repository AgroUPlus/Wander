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

pub(crate) mod features;
pub(crate) mod input;
pub(crate) mod layout;
pub(crate) mod loading;
pub(crate) mod navigation;
pub(crate) mod overlays;
pub(crate) mod settings;
pub(crate) mod storage;
#[cfg(test)]
mod tests;
pub mod types;

pub use layout::format_duration;
pub use navigation::focus_order;
pub use types::*;

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

pub struct App {
    pub config: Config,
    /// The theme actually drawn with: the configured preset, tinted by the
    /// current cover art. Everything in `ui` reads this rather than
    /// `config.theme`, so generated colours can never leak back into the file
    /// on disk.
    pub theme: Theme,
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
    /// How the spectrum is drawn; cycled with `V`.
    pub viz_mode: crate::ui::visualiser::VizMode,

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
    /// Accents pulled from the current artwork. `config.theme` stays the
    /// pristine preset — it is what gets written back to `config.toml` — and
    /// this is folded into it to produce [`App::theme`].
    pub cover_palette: Option<crate::theme::Palette>,
    /// Bumped whenever the artwork itself changes, so the frame loop can tell a
    /// new cover from a redraw of the same one. See [`App::frame_shape`].
    cover_generation: u64,
    /// A finished encoding waiting to be handed to `CoverRenderer`, which the
    /// app does not own.
    pub cover_resized: Option<Box<ratatui_image::thread::ResizeResponse>>,

    pub lyrics: crate::subsonic::lyrics::LyricSet,
    /// Shared HTTP client for the few requests that do not go through a library
    /// backend, i.e. lyric translation.
    http: reqwest::Client,
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
    pub visualiser_height: u16,
    eased_cover: f32,
    eased_queue: f32,
    eased_visualiser_height: f32,
    drag_start_viz_height: Option<u16>,

    /// Session state is written at most once a second: serialising a long queue
    /// on every keypress is what made held-down resize keys stutter.
    state_dirty: bool,
    last_state_save: Instant,

    last_click: Option<(Region, Instant)>,
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
            theme: config.theme.clone(),
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
            viz_mode: crate::ui::visualiser::VizMode::default(),
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
            cover_palette: None,
            cover_generation: 0,
            cover_resized: None,
            lyrics: Default::default(),
            lyrics_pending: false,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
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
            visualiser_height: 8,
            eased_cover: 25.0,
            eased_queue: 18.0,
            eased_visualiser_height: 8.0,
            drag_start_viz_height: None,
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
}
