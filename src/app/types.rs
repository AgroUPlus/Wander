use crate::subsonic::models::{Album, Artist, Playlist, Song};
use anyhow::Result;
use std::sync::Arc;

/// A coarse fingerprint of the current frame's structure, used to decide when
/// the screen must be repainted from scratch. See [`App::frame_shape`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameShape {
    pub(crate) overlay: Option<u8>,
    pub(crate) show_help: bool,
    pub(crate) focus_mode: bool,
    pub(crate) panes: [bool; 5],
    pub(crate) panes_sizes: [u16; 3],
    pub(crate) cover: u64,
    pub(crate) tab: u8,
    pub(crate) viz_mode: crate::ui::visualiser::VizMode,
}

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

    pub(crate) fn panes(self) -> &'static [Pane] {
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
        /// Accents derived from the artwork, or `None` when it has no usable
        /// colour. Extracted on the loading task so decoding never lands on
        /// the frame loop.
        palette: Option<crate::theme::Palette>,
    },
    Lyrics {
        song_id: String,
        lyrics: Box<crate::subsonic::lyrics::LyricSet>,
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
