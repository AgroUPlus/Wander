//! One library over both backends.
//!
//! The point of this type is that its identity never changes. `App`, the player
//! task and the Discord integration each hold the same `Arc<MergedLibrary>` for
//! the whole process lifetime; reconfiguring the server from the settings panel
//! swaps the *inner* backend, so there is nothing to hot-swap downstream and no
//! restart to ask the user for.

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use super::{Capabilities, Library, LocalLibrary, SongSource, Source, SubsonicLibrary, is_local_id};
use crate::subsonic::lyrics::LyricSet;
use crate::subsonic::models::{
    Album, AlbumInfo, Artist, Genre, Playlist, SearchResult3, Share, Song,
};

#[derive(Default)]
pub struct MergedLibrary {
    remote: RwLock<Option<Arc<SubsonicLibrary>>>,
    local: RwLock<Option<Arc<LocalLibrary>>>,
}

impl MergedLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_remote(&self, remote: Option<Arc<SubsonicLibrary>>) {
        *self.remote.write().unwrap() = remote;
    }

    pub fn set_local(&self, local: Option<Arc<LocalLibrary>>) {
        *self.local.write().unwrap() = local;
    }

    pub fn remote(&self) -> Option<Arc<SubsonicLibrary>> {
        self.remote.read().unwrap().clone()
    }

    pub fn local(&self) -> Option<Arc<LocalLibrary>> {
        self.local.read().unwrap().clone()
    }

    /// The backend that owns `id`, chosen by its namespace prefix.
    fn owner(&self, id: &str) -> Option<Arc<dyn Library>> {
        if is_local_id(id) {
            self.local().map(|l| l as Arc<dyn Library>)
        } else {
            self.remote().map(|r| r as Arc<dyn Library>)
        }
    }

    /// Both configured backends, remote first so server results lead in merged
    /// lists (they are the larger collection in a mixed setup).
    fn all(&self) -> Vec<Arc<dyn Library>> {
        let mut out: Vec<Arc<dyn Library>> = Vec::new();
        if let Some(remote) = self.remote() {
            out.push(remote);
        }
        if let Some(local) = self.local() {
            out.push(local);
        }
        out
    }
}

/// Run `call` on every backend and concatenate the results.
///
/// A failing backend contributes nothing rather than failing the whole call:
/// an unreachable server must not blank out the local library too.
macro_rules! merge {
    ($self:expr, |$backend:ident| $call:expr) => {{
        let mut out = Vec::new();
        for $backend in $self.all() {
            if let Ok(items) = $call.await {
                out.extend(items);
            }
        }
        Ok(out)
    }};
}

#[async_trait]
impl Library for MergedLibrary {
    fn capabilities(&self) -> Capabilities {
        // The union: an action is offered when *some* backend can do it, and
        // routing by id decides whether this particular track can.
        let mut caps = Capabilities::NONE;
        for backend in self.all() {
            let b = backend.capabilities();
            caps.scrobble |= b.scrobble;
            caps.share |= b.share;
            caps.similarity |= b.similarity;
            caps.playlist_write |= b.playlist_write;
            caps.rating |= b.rating;
        }
        caps
    }

    async fn open(&self, song: &Song, offset: Duration, force_transcode: bool) -> Result<Source> {
        if song.id.starts_with("http://") || song.id.starts_with("https://") {
            let http = reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default();
            return Ok(Source::Http {
                url: song.id.clone(),
                http,
                // Nothing to ask an arbitrary host for: it serves the file from
                // the beginning whatever offset was requested.
                starts_at: Duration::ZERO,
            });
        }
        let raw_path = song
            .id
            .strip_prefix(super::LOCAL_PREFIX)
            .or_else(|| song.id.strip_prefix(super::ONLINE_PREFIX))
            .unwrap_or(&song.id);
        // Only an absolute path: a server id like "100" must never be able to
        // pick up a file of that name in the process's working directory.
        let path = std::path::PathBuf::from(raw_path);
        if path.is_absolute() && path.is_file() {
            return Ok(Source::File(path));
        }

        match self.owner(&song.id) {
            Some(backend) => backend.open(song, offset, force_transcode).await,
            None if is_local_id(&song.id) => {
                bail!("no local library is configured for this track")
            }
            None => bail!("no server is configured for this track"),
        }
    }

    async fn artists(&self) -> Result<Vec<Artist>> {
        merge!(self, |b| b.artists())
    }

    async fn artist_albums(&self, artist_id: &str) -> Result<Vec<Album>> {
        match self.owner(artist_id) {
            Some(b) => b.artist_albums(artist_id).await,
            None => Ok(Vec::new()),
        }
    }

    async fn album_songs(&self, album_id: &str) -> Result<Vec<Song>> {
        match self.owner(album_id) {
            Some(b) => b.album_songs(album_id).await,
            None => Ok(Vec::new()),
        }
    }

    async fn album_list(&self, kind: &str, size: u32, offset: u32) -> Result<Vec<Album>> {
        merge!(self, |b| b.album_list(kind, size, offset))
    }

    async fn album_info(&self, album_id: &str) -> Result<AlbumInfo> {
        match self.owner(album_id) {
            Some(b) => b.album_info(album_id).await,
            None => Ok(AlbumInfo::default()),
        }
    }

    async fn all_songs(&self, count: u32, offset: u32) -> Result<Vec<Song>> {
        merge!(self, |b| b.all_songs(count, offset))
    }

    async fn random_songs(&self, count: u32, genre: Option<&str>) -> Result<Vec<Song>> {
        merge!(self, |b| b.random_songs(count, genre))
    }

    async fn starred_songs(&self) -> Result<Vec<Song>> {
        merge!(self, |b| b.starred_songs())
    }

    async fn songs_by_genre(&self, genre: &str, count: u32, offset: u32) -> Result<Vec<Song>> {
        merge!(self, |b| b.songs_by_genre(genre, count, offset))
    }

    async fn genres(&self) -> Result<Vec<Genre>> {
        merge!(self, |b| b.genres())
    }

    async fn similar_songs(&self, song_id: &str, count: u32) -> Result<Vec<Song>> {
        match self.owner(song_id) {
            Some(b) => b.similar_songs(song_id, count).await,
            None => Ok(Vec::new()),
        }
    }

    async fn similar_artists(&self, artist_id: &str, count: u32) -> Result<Vec<Artist>> {
        match self.owner(artist_id) {
            Some(b) => b.similar_artists(artist_id, count).await,
            None => Ok(Vec::new()),
        }
    }

    async fn top_songs(&self, artist_name: &str, count: u32) -> Result<Vec<Song>> {
        // Keyed by name, not id, so there is nothing to route on.
        merge!(self, |b| b.top_songs(artist_name, count))
    }

    async fn search(&self, query: &str, count: u32) -> Result<SearchResult3> {
        let mut merged = SearchResult3::default();
        for backend in self.all() {
            if let Ok(result) = backend.search(query, count).await {
                merged.artist.extend(result.artist);
                merged.album.extend(result.album);
                merged.song.extend(result.song);
            }
        }
        Ok(merged)
    }

    async fn lyrics(&self, song_id: &str) -> Result<LyricSet> {
        match self.owner(song_id) {
            Some(b) => b.lyrics(song_id).await,
            None => Ok(LyricSet::default()),
        }
    }

    async fn cover_art(&self, cover_id: &str, size: u32) -> Result<Vec<u8>> {
        // Online plugins have no backend to route to: their artwork is either
        // a plain URL or embedded in a file they cached, so it is fetched here
        // rather than being handed to a server that has never heard of it.
        if cover_id.starts_with("http://") || cover_id.starts_with("https://") {
            let http = reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default();
            let response = http
                .get(cover_id)
                .header("User-Agent", "wander-tui/0.1 (https://archive.org; music player)")
                .send()
                .await
                .context("fetching cover art")?;
            if !response.status().is_success() {
                bail!("cover art request returned HTTP {}", response.status());
            }
            return Ok(response.bytes().await.context("reading cover art")?.to_vec());
        }

        if let Some(path) = cover_id.strip_prefix(super::ONLINE_PREFIX) {
            let path = std::path::PathBuf::from(path);
            return tokio::task::spawn_blocking(move || super::local::read_cover(&path))
                .await
                .unwrap_or_else(|err| bail!("cover art read panicked: {err}"));
        }

        match self.owner(cover_id) {
            Some(b) => b.cover_art(cover_id, size).await,
            None => bail!("no backend owns cover art {cover_id}"),
        }
    }

    async fn playlists(&self) -> Result<Vec<Playlist>> {
        merge!(self, |b| b.playlists())
    }

    async fn playlist_songs(&self, playlist_id: &str) -> Result<Vec<Song>> {
        match self.owner(playlist_id) {
            Some(b) => b.playlist_songs(playlist_id).await,
            None => Ok(Vec::new()),
        }
    }

    async fn create_playlist(&self, name: &str, song_ids: &[String]) -> Result<()> {
        // A new playlist has no id to route on. Anything touching a local file
        // has to be stored locally, because the server cannot reference a path
        // on this machine; everything else prefers the server.
        if song_ids
            .iter()
            .any(|id| SongSource::of(id) != SongSource::Server)
        {
            return match self.local() {
                Some(local) => local.create_playlist(name, song_ids).await,
                None => bail!("no local library is configured to store this playlist"),
            };
        }
        match self
            .remote()
            .map(|r| r as Arc<dyn Library>)
            .or_else(|| self.local().map(|l| l as Arc<dyn Library>))
        {
            Some(backend) => backend.create_playlist(name, song_ids).await,
            None => bail!("no library is configured to store playlists"),
        }
    }

    async fn add_to_playlist(&self, playlist_id: &str, song_ids: &[String]) -> Result<()> {
        match self.owner(playlist_id) {
            Some(b) => b.add_to_playlist(playlist_id, song_ids).await,
            None => bail!("no backend owns playlist {playlist_id}"),
        }
    }

    async fn remove_from_playlist(&self, playlist_id: &str, indices: &[usize]) -> Result<()> {
        match self.owner(playlist_id) {
            Some(b) => b.remove_from_playlist(playlist_id, indices).await,
            None => bail!("no backend owns playlist {playlist_id}"),
        }
    }

    async fn scrobble(&self, song_id: &str, submission: bool) -> Result<()> {
        match self.owner(song_id) {
            Some(b) => b.scrobble(song_id, submission).await,
            None => Ok(()),
        }
    }

    async fn set_starred(&self, song_id: &str, starred: bool) -> Result<()> {
        match self.owner(song_id) {
            Some(b) => b.set_starred(song_id, starred).await,
            None => Ok(()),
        }
    }

    async fn set_rating(&self, song_id: &str, rating: u8) -> Result<()> {
        match self.owner(song_id) {
            Some(b) => b.set_rating(song_id, rating).await,
            None => Ok(()),
        }
    }

    async fn create_share(
        &self,
        ids: &[String],
        description: &str,
        expires_ms: Option<i64>,
        downloadable: bool,
    ) -> Result<Share> {
        if ids
            .iter()
            .any(|id| SongSource::of(id) != SongSource::Server)
        {
            bail!("only server tracks can be shared: there is no server to serve the others");
        }
        match self.remote() {
            Some(remote) => {
                remote
                    .create_share(ids, description, expires_ms, downloadable)
                    .await
            }
            None => bail!("sharing needs a configured server"),
        }
    }
}
