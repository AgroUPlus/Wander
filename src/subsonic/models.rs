// These types mirror the Subsonic API schema. Some fields are not read by the
// current UI but are kept so the models stay faithful to the wire format and
// are ready for later screens.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// The metadata fields present on every `subsonic-response` envelope. The
/// typed payload is extracted separately by the client, because payload-less
/// replies (`ping`) and error replies have no `T` to deserialize.
#[derive(Debug, Deserialize)]
pub struct ResponseMeta {
    pub status: String,
    pub version: String,
    #[serde(rename = "type")]
    pub server_type: Option<String>,
    #[serde(rename = "serverVersion")]
    pub server_version: Option<String>,
    pub error: Option<ApiError>,
}

#[derive(Debug, Deserialize)]
pub struct ApiError {
    pub code: i32,
    pub message: String,
}

/// `getArtists` buckets artists by index letter; callers want them flat.
#[derive(Debug, Deserialize)]
pub struct ArtistsResponse {
    pub artists: ArtistIndexes,
}

#[derive(Debug, Deserialize)]
pub struct ArtistIndexes {
    #[serde(default)]
    pub index: Vec<ArtistIndex>,
}

#[derive(Debug, Deserialize)]
pub struct ArtistIndex {
    #[serde(default)]
    pub artist: Vec<Artist>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Artist {
    pub id: String,
    pub name: String,
    #[serde(rename = "albumCount", default)]
    pub album_count: u32,
    #[serde(rename = "coverArt")]
    pub cover_art: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ArtistResponse {
    pub artist: ArtistWithAlbums,
}

#[derive(Debug, Deserialize)]
pub struct ArtistWithAlbums {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub album: Vec<Album>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Album {
    pub id: String,
    pub name: String,
    pub artist: Option<String>,
    #[serde(rename = "artistId")]
    pub artist_id: Option<String>,
    #[serde(rename = "coverArt")]
    pub cover_art: Option<String>,
    #[serde(rename = "songCount", default)]
    pub song_count: u32,
    #[serde(default)]
    pub duration: u32,
    pub year: Option<u32>,
    pub genre: Option<String>,
    /// MusicBrainz release ID, when the server knows one. Used to look up
    /// public cover art that carries no credentials.
    #[serde(rename = "musicBrainzId")]
    pub music_brainz_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AlbumResponse {
    pub album: AlbumWithSongs,
}

#[derive(Debug, Deserialize)]
pub struct AlbumWithSongs {
    pub id: String,
    pub name: String,
    #[serde(rename = "musicBrainzId")]
    pub music_brainz_id: Option<String>,
    #[serde(default)]
    pub song: Vec<Song>,
}

#[derive(Debug, Deserialize)]
pub struct AlbumListResponse {
    #[serde(rename = "albumList2")]
    pub album_list: AlbumList,
}

#[derive(Debug, Deserialize)]
pub struct AlbumList {
    #[serde(default)]
    pub album: Vec<Album>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Song {
    pub id: String,
    pub title: String,
    pub album: Option<String>,
    #[serde(rename = "albumId")]
    pub album_id: Option<String>,
    pub artist: Option<String>,
    #[serde(rename = "artistId")]
    pub artist_id: Option<String>,
    #[serde(rename = "coverArt")]
    pub cover_art: Option<String>,
    /// Duration in seconds. Absent on some entries, so default to zero.
    #[serde(default)]
    pub duration: u32,
    #[serde(rename = "bitRate", default)]
    pub bit_rate: u32,
    pub track: Option<u32>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub suffix: Option<String>,
    #[serde(rename = "contentType")]
    pub content_type: Option<String>,
    #[serde(default)]
    pub size: u64,
    /// Timestamp when the user starred this song; absent when not starred.
    pub starred: Option<String>,
    /// The user's own 1–5 rating, when they have set one.
    #[serde(rename = "userRating")]
    pub user_rating: Option<u8>,
    #[serde(rename = "playCount", default)]
    pub play_count: u32,
    /// OpenSubsonic multi-genre list; richer than the single `genre` field.
    #[serde(default)]
    pub genres: Vec<ItemGenre>,
    #[serde(default)]
    pub moods: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemGenre {
    pub name: String,
}

impl Song {
    pub fn is_starred(&self) -> bool {
        self.starred.is_some()
    }

    /// All genre names for this song, from either representation.
    pub fn genre_names(&self) -> Vec<String> {
        if !self.genres.is_empty() {
            return self.genres.iter().map(|g| g.name.to_lowercase()).collect();
        }
        self.genre.iter().map(|g| g.to_lowercase()).collect()
    }
}

impl Song {
    pub fn artist_or_unknown(&self) -> &str {
        self.artist.as_deref().unwrap_or("Unknown Artist")
    }

    pub fn album_or_unknown(&self) -> &str {
        self.album.as_deref().unwrap_or("Unknown Album")
    }
}

#[derive(Debug, Deserialize)]
pub struct PlaylistsResponse {
    pub playlists: Playlists,
}

#[derive(Debug, Deserialize)]
pub struct Playlists {
    #[serde(default)]
    pub playlist: Vec<Playlist>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    #[serde(rename = "songCount", default)]
    pub song_count: u32,
    #[serde(default)]
    pub duration: u32,
    #[serde(rename = "coverArt")]
    pub cover_art: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PlaylistResponse {
    pub playlist: PlaylistWithSongs,
}

#[derive(Debug, Deserialize)]
pub struct PlaylistWithSongs {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub entry: Vec<Song>,
}

#[derive(Debug, Deserialize)]
pub struct Search3Response {
    #[serde(rename = "searchResult3")]
    pub result: SearchResult3,
}

#[derive(Debug, Default, Deserialize)]
pub struct SearchResult3 {
    #[serde(default)]
    pub artist: Vec<Artist>,
    #[serde(default)]
    pub album: Vec<Album>,
    #[serde(default)]
    pub song: Vec<Song>,
}

/// OpenSubsonic `getLyricsBySongId` (the `songLyrics` extension).
#[derive(Debug, Deserialize)]
pub struct LyricsResponse {
    #[serde(rename = "lyricsList")]
    pub lyrics_list: LyricsList,
}

#[derive(Debug, Default, Deserialize)]
pub struct LyricsList {
    #[serde(rename = "structuredLyrics", default)]
    pub structured: Vec<StructuredLyrics>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StructuredLyrics {
    /// True when every line carries a timestamp, enabling the scrolling view.
    #[serde(default)]
    pub synced: bool,
    pub lang: Option<String>,
    #[serde(rename = "displayArtist")]
    pub display_artist: Option<String>,
    #[serde(rename = "displayTitle")]
    pub display_title: Option<String>,
    #[serde(default)]
    pub line: Vec<LyricLine>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LyricLine {
    /// Offset from the start of the track, in milliseconds. Absent on
    /// unsynced lyrics.
    pub start: Option<u64>,
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct RandomSongsResponse {
    #[serde(rename = "randomSongs")]
    pub random_songs: SongList,
}

#[derive(Debug, Default, Deserialize)]
pub struct SongList {
    #[serde(default)]
    pub song: Vec<Song>,
}

#[derive(Debug, Deserialize)]
pub struct StarredResponse {
    #[serde(rename = "starred2")]
    pub starred: SongList,
}

/// `getAlbumInfo2`. Carries the MusicBrainz ID and, when the server has Last.fm
/// configured, ready-made public image URLs.
#[derive(Debug, Deserialize)]
pub struct AlbumInfoResponse {
    #[serde(rename = "albumInfo")]
    pub album_info: AlbumInfo,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct AlbumInfo {
    #[serde(rename = "musicBrainzId")]
    pub music_brainz_id: Option<String>,
    #[serde(rename = "largeImageUrl")]
    pub large_image_url: Option<String>,
    #[serde(rename = "mediumImageUrl")]
    pub medium_image_url: Option<String>,
}

/// `getSimilarSongs2`. Empty on a Navidrome without Last.fm configured, which
/// is why radio mode never relies on it alone.
#[derive(Debug, Deserialize)]
pub struct SimilarSongsResponse {
    #[serde(rename = "similarSongs2")]
    pub similar_songs: SongList,
}

#[derive(Debug, Deserialize)]
pub struct TopSongsResponse {
    #[serde(rename = "topSongs")]
    pub top_songs: SongList,
}

#[derive(Debug, Deserialize)]
pub struct SongsByGenreResponse {
    #[serde(rename = "songsByGenre")]
    pub songs_by_genre: SongList,
}

#[derive(Debug, Deserialize)]
pub struct ArtistInfoResponse {
    #[serde(rename = "artistInfo2")]
    pub artist_info: ArtistInfo,
}

#[derive(Debug, Default, Deserialize)]
pub struct ArtistInfo {
    #[serde(rename = "similarArtist", default)]
    pub similar_artist: Vec<Artist>,
}

/// `createShare` / `getShares`. Navidrome returns the public URL directly.
#[derive(Debug, Deserialize)]
pub struct SharesResponse {
    pub shares: Shares,
}

#[derive(Debug, Default, Deserialize)]
pub struct Shares {
    #[serde(default)]
    pub share: Vec<Share>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Share {
    pub id: String,
    pub url: String,
    pub description: Option<String>,
    pub expires: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GenresResponse {
    pub genres: Genres,
}

#[derive(Debug, Deserialize)]
pub struct Genres {
    #[serde(default)]
    pub genre: Vec<Genre>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Genre {
    pub value: String,
    #[serde(rename = "songCount", default)]
    pub song_count: u32,
    #[serde(rename = "albumCount", default)]
    pub album_count: u32,
}
