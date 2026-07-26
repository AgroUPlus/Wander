//! The music-source abstraction.
//!
//! Everything above this layer — the UI, the player task, the integrations —
//! talks to a [`Library`] rather than to a Subsonic server. Two backends
//! implement it (Navidrome over Subsonic, and a local on-disk collection), and
//! [`MergedLibrary`] presents both as one library so a single queue can mix
//! streamed and local tracks freely.
//!
//! Entities keep the Subsonic model types; local ones are distinguished by an
//! id prefix rather than by a wider model, so everything that already persists
//! an id as an opaque string (`saved_state.json`, `history.jsonl`, the command
//! palette) keeps working untouched.

pub mod local;
pub mod merged;
pub mod subsonic;

use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use std::time::Duration;

use crate::subsonic::lyrics::Lyrics;
use crate::subsonic::models::{
    Album, AlbumInfo, Artist, Genre, Playlist, SearchResult3, Share, Song,
};

pub use local::LocalLibrary;
pub use merged::MergedLibrary;
pub use subsonic::SubsonicLibrary;

/// Prefix marking an id as belonging to the local backend.
pub const LOCAL_PREFIX: &str = "local:";
pub const LOCAL_ALBUM_PREFIX: &str = "local-album:";
pub const LOCAL_ARTIST_PREFIX: &str = "local-artist:";
pub const LOCAL_PLAYLIST_PREFIX: &str = "local-playlist:";

/// Whether an id was minted by the local backend.
pub fn is_local_id(id: &str) -> bool {
    id.starts_with(LOCAL_PREFIX)
        || id.starts_with(LOCAL_ALBUM_PREFIX)
        || id.starts_with(LOCAL_ARTIST_PREFIX)
        || id.starts_with(LOCAL_PLAYLIST_PREFIX)
}

/// Where the bytes of a track come from.
///
/// Both variants end up as a `Box<dyn MediaSource>` for the decoder; the split
/// exists because an HTTP body has to be pumped through a channel while a file
/// can be handed over directly.
pub enum Source {
    /// Stream over HTTP. The client comes with the URL because the auth token
    /// is baked into the URL and only the owning backend can build both.
    Http {
        url: String,
        http: reqwest::Client,
    },
    File(PathBuf),
}

impl std::fmt::Debug for Source {
    /// The URL carries an auth token, so it is never printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http { .. } => f.write_str("Source::Http(<url redacted>)"),
            Self::File(path) => write!(f, "Source::File({})", path.display()),
        }
    }
}

/// What a backend can do beyond browsing and playback.
///
/// The UI reads these to grey out actions rather than offering something that
/// will only fail: a local-only setup has no server to share from or scrobble
/// to, and no similarity service to seed radio mode with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub scrobble: bool,
    pub share: bool,
    /// Server-side "similar songs"/"similar artists"/"top songs".
    pub similarity: bool,
    /// Playlists can be created and edited.
    pub playlist_write: bool,
    pub rating: bool,
}

impl Capabilities {
    pub const NONE: Self = Self {
        scrobble: false,
        share: false,
        similarity: false,
        playlist_write: false,
        rating: false,
    };
}

/// A source of music.
///
/// Every method has a default returning an empty result, so a backend only
/// implements what it actually supports and callers never have to distinguish
/// "unsupported" from "nothing found" — the UI uses [`Capabilities`] for that.
#[async_trait]
pub trait Library: Send + Sync {
    fn capabilities(&self) -> Capabilities;

    /// Open a track for playback.
    async fn open(&self, song: &Song, offset: Duration, force_transcode: bool) -> Result<Source>;

    async fn artists(&self) -> Result<Vec<Artist>> {
        Ok(Vec::new())
    }
    async fn artist_albums(&self, _artist_id: &str) -> Result<Vec<Album>> {
        Ok(Vec::new())
    }
    async fn album_songs(&self, _album_id: &str) -> Result<Vec<Song>> {
        Ok(Vec::new())
    }
    async fn album_list(&self, _kind: &str, _size: u32, _offset: u32) -> Result<Vec<Album>> {
        Ok(Vec::new())
    }
    async fn album_info(&self, _album_id: &str) -> Result<AlbumInfo> {
        Ok(AlbumInfo::default())
    }
    async fn all_songs(&self, _count: u32, _offset: u32) -> Result<Vec<Song>> {
        Ok(Vec::new())
    }
    async fn random_songs(&self, _count: u32, _genre: Option<&str>) -> Result<Vec<Song>> {
        Ok(Vec::new())
    }
    async fn starred_songs(&self) -> Result<Vec<Song>> {
        Ok(Vec::new())
    }
    async fn songs_by_genre(&self, _genre: &str, _count: u32, _offset: u32) -> Result<Vec<Song>> {
        Ok(Vec::new())
    }
    async fn genres(&self) -> Result<Vec<Genre>> {
        Ok(Vec::new())
    }
    async fn similar_songs(&self, _song_id: &str, _count: u32) -> Result<Vec<Song>> {
        Ok(Vec::new())
    }
    async fn similar_artists(&self, _artist_id: &str, _count: u32) -> Result<Vec<Artist>> {
        Ok(Vec::new())
    }
    async fn top_songs(&self, _artist_name: &str, _count: u32) -> Result<Vec<Song>> {
        Ok(Vec::new())
    }
    async fn search(&self, _query: &str, _count: u32) -> Result<SearchResult3> {
        Ok(SearchResult3::default())
    }
    async fn lyrics(&self, _song_id: &str) -> Result<Lyrics> {
        Ok(Lyrics::default())
    }
    async fn cover_art(&self, _cover_id: &str, _size: u32) -> Result<Vec<u8>> {
        anyhow::bail!("no cover art for this source")
    }

    async fn playlists(&self) -> Result<Vec<Playlist>> {
        Ok(Vec::new())
    }
    async fn playlist_songs(&self, _playlist_id: &str) -> Result<Vec<Song>> {
        Ok(Vec::new())
    }
    async fn create_playlist(&self, _name: &str, _song_ids: &[String]) -> Result<()> {
        anyhow::bail!("this source cannot create playlists")
    }
    async fn add_to_playlist(&self, _playlist_id: &str, _song_ids: &[String]) -> Result<()> {
        anyhow::bail!("this source cannot edit playlists")
    }
    async fn remove_from_playlist(&self, _playlist_id: &str, _indices: &[usize]) -> Result<()> {
        anyhow::bail!("this source cannot edit playlists")
    }

    async fn scrobble(&self, _song_id: &str, _submission: bool) -> Result<()> {
        Ok(())
    }
    async fn set_starred(&self, _song_id: &str, _starred: bool) -> Result<()> {
        Ok(())
    }
    async fn set_rating(&self, _song_id: &str, _rating: u8) -> Result<()> {
        Ok(())
    }
    async fn create_share(
        &self,
        _ids: &[String],
        _description: &str,
        _expires_ms: Option<i64>,
        _downloadable: bool,
    ) -> Result<Share> {
        anyhow::bail!("this source cannot create share links")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_ids_are_distinguishable_from_server_ids() {
        assert!(is_local_id("local:abc123"));
        assert!(is_local_id("local-album:abc123"));
        assert!(is_local_id("local-artist:abc123"));
        assert!(is_local_id("local-playlist:abc123"));
        // Navidrome ids are bare hex/uuid strings and must never be mistaken
        // for local ones, including ones that merely start with "local".
        assert!(!is_local_id("8f3c9e2a1b"));
        assert!(!is_local_id("localhost"));
        assert!(!is_local_id(""));
    }
}
