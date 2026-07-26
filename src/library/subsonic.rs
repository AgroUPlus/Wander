//! [`Library`] over a Navidrome/Subsonic server.
//!
//! Pure delegation to [`SubsonicClient`]; the wire protocol stays in
//! `crate::subsonic`.

use anyhow::Result;
use async_trait::async_trait;
use std::time::Duration;

use super::{Capabilities, Library, Source};
use crate::subsonic::SubsonicClient;
use crate::subsonic::lyrics::LyricSet;
use crate::subsonic::models::{
    Album, AlbumInfo, Artist, Genre, Playlist, SearchResult3, Share, Song,
};

pub struct SubsonicLibrary {
    client: SubsonicClient,
}

impl SubsonicLibrary {
    pub fn new(client: SubsonicClient) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &SubsonicClient {
        &self.client
    }
}

#[async_trait]
impl Library for SubsonicLibrary {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            scrobble: true,
            share: true,
            similarity: true,
            playlist_write: true,
            rating: true,
        }
    }

    async fn open(&self, song: &Song, offset: Duration, force_transcode: bool) -> Result<Source> {
        Ok(Source::Http {
            url: self
                .client
                .stream_url(&song.id, song.suffix.as_deref(), offset, force_transcode),
            http: self.client.http().clone(),
        })
    }

    async fn artists(&self) -> Result<Vec<Artist>> {
        self.client.artists().await
    }
    async fn artist_albums(&self, artist_id: &str) -> Result<Vec<Album>> {
        self.client.artist_albums(artist_id).await
    }
    async fn album_songs(&self, album_id: &str) -> Result<Vec<Song>> {
        self.client.album_songs(album_id).await
    }
    async fn album_list(&self, kind: &str, size: u32, offset: u32) -> Result<Vec<Album>> {
        self.client.album_list(kind, size, offset).await
    }
    async fn album_info(&self, album_id: &str) -> Result<AlbumInfo> {
        self.client.album_info(album_id).await
    }
    async fn all_songs(&self, count: u32, offset: u32) -> Result<Vec<Song>> {
        self.client.all_songs(count, offset).await
    }
    async fn random_songs(&self, count: u32, genre: Option<&str>) -> Result<Vec<Song>> {
        self.client.random_songs(count, genre).await
    }
    async fn starred_songs(&self) -> Result<Vec<Song>> {
        self.client.starred_songs().await
    }
    async fn songs_by_genre(&self, genre: &str, count: u32, offset: u32) -> Result<Vec<Song>> {
        self.client.songs_by_genre(genre, count, offset).await
    }
    async fn genres(&self) -> Result<Vec<Genre>> {
        self.client.genres().await
    }
    async fn similar_songs(&self, song_id: &str, count: u32) -> Result<Vec<Song>> {
        self.client.similar_songs(song_id, count).await
    }
    async fn similar_artists(&self, artist_id: &str, count: u32) -> Result<Vec<Artist>> {
        self.client.similar_artists(artist_id, count).await
    }
    async fn top_songs(&self, artist_name: &str, count: u32) -> Result<Vec<Song>> {
        self.client.top_songs(artist_name, count).await
    }
    async fn search(&self, query: &str, count: u32) -> Result<SearchResult3> {
        self.client.search(query, count).await
    }
    async fn lyrics(&self, song_id: &str) -> Result<LyricSet> {
        self.client.lyrics(song_id).await
    }
    async fn cover_art(&self, cover_id: &str, size: u32) -> Result<Vec<u8>> {
        self.client.cover_art(cover_id, size).await
    }

    async fn playlists(&self) -> Result<Vec<Playlist>> {
        self.client.playlists().await
    }
    async fn playlist_songs(&self, playlist_id: &str) -> Result<Vec<Song>> {
        self.client.playlist_songs(playlist_id).await
    }
    async fn create_playlist(&self, name: &str, song_ids: &[String]) -> Result<()> {
        self.client.create_playlist(name, song_ids).await
    }
    async fn add_to_playlist(&self, playlist_id: &str, song_ids: &[String]) -> Result<()> {
        self.client.add_to_playlist(playlist_id, song_ids).await
    }
    async fn remove_from_playlist(&self, playlist_id: &str, indices: &[usize]) -> Result<()> {
        self.client.remove_from_playlist(playlist_id, indices).await
    }

    async fn scrobble(&self, song_id: &str, submission: bool) -> Result<()> {
        self.client.scrobble(song_id, submission).await
    }
    async fn set_starred(&self, song_id: &str, starred: bool) -> Result<()> {
        self.client.set_starred(song_id, starred).await
    }
    async fn set_rating(&self, song_id: &str, rating: u8) -> Result<()> {
        self.client.set_rating(song_id, rating).await
    }
    async fn create_share(
        &self,
        ids: &[String],
        description: &str,
        expires_ms: Option<i64>,
        downloadable: bool,
    ) -> Result<Share> {
        self.client
            .create_share(ids, description, expires_ms, downloadable)
            .await
    }
}
