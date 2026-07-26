pub mod cache;
pub mod lyrics;
pub mod models;

use anyhow::{Context, Result, anyhow, bail};
use md5::{Digest, Md5};
use serde::de::DeserializeOwned;
use std::time::Duration;

use models::*;

const API_VERSION: &str = "1.16.1";
const CLIENT_NAME: &str = "wander";
const SALT_LEN: usize = 12;

/// Format requested when we cannot decode the original ourselves.
const TRANSCODE_FALLBACK: &str = "mp3";

/// File extensions we can decode locally, and therefore stream untouched.
///
/// `opus` is included because we bundle a libopus-backed decoder (see
/// `player::opus`); symphonia alone does not support it. Keep this in sync
/// with `player::opus::registry`.
const DECODABLE: &[&str] = &[
    "opus", "mp3", "flac", "m4a", "m4b", "mp4", "aac", "alac", "ogg", "oga", "wav", "wave", "mka",
    "webm",
];

fn is_decodable(suffix: &str) -> bool {
    let suffix = suffix.trim_start_matches('.').to_ascii_lowercase();
    DECODABLE.contains(&suffix.as_str())
}

/// A Subsonic/OpenSubsonic API client pointed at a Navidrome server.
///
/// Auth uses the salted-token scheme: each request carries a fresh random salt
/// and `md5(password + salt)`, so the password never crosses the wire.
#[derive(Clone)]
pub struct SubsonicClient {
    http: reqwest::Client,
    base: String,
    username: String,
    password: String,
    /// Transcode format to request from the server; `raw` means no transcode.
    format: String,
}

impl SubsonicClient {
    pub fn new(
        base_url: &str,
        username: &str,
        password: &str,
        format: Option<&str>,
    ) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("wander/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .context("building HTTP client")?;

        Ok(Self {
            http,
            base: base_url.trim_end_matches('/').to_string(),
            username: username.to_string(),
            password: password.to_string(),
            format: format.unwrap_or("raw").to_string(),
        })
    }

    /// Build a fully-qualified URL for an endpoint with auth params applied.
    pub fn url(&self, endpoint: &str, params: &[(&str, &str)]) -> String {
        let (salt, token) = auth_token(&self.password);
        let mut url = format!(
            "{}/rest/{}?u={}&t={}&s={}&v={}&c={}&f=json",
            self.base,
            endpoint,
            urlencoding::encode(&self.username),
            token,
            salt,
            API_VERSION,
            CLIENT_NAME,
        );
        for (key, value) in params {
            url.push('&');
            url.push_str(key);
            url.push('=');
            url.push_str(&urlencoding::encode(value));
        }
        url
    }

    /// Issue a request and extract the typed payload from the envelope.
    async fn get<T: DeserializeOwned>(&self, endpoint: &str, params: &[(&str, &str)]) -> Result<T> {
        let url = self.url(endpoint, params);
        let response = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("requesting {endpoint}"))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .with_context(|| format!("reading {endpoint} response body"))?;

        if !status.is_success() {
            bail!("{endpoint} returned HTTP {status}");
        }

        let value: serde_json::Value = serde_json::from_str(&body)
            .with_context(|| format!("parsing {endpoint} response as JSON"))?;
        let inner = value
            .get("subsonic-response")
            .ok_or_else(|| anyhow!("{endpoint}: response missing subsonic-response envelope"))?;

        let meta: ResponseMeta = serde_json::from_value(inner.clone())
            .with_context(|| format!("parsing {endpoint} envelope"))?;
        if meta.status != "ok" {
            let err = meta
                .error
                .map(|e| format!("{} (code {})", e.message, e.code))
                .unwrap_or_else(|| "unknown error".to_string());
            bail!("{endpoint} failed: {err}");
        }

        serde_json::from_value(inner.clone()).with_context(|| format!("parsing {endpoint} payload"))
    }

    /// Verify connectivity and credentials, returning the server version.
    pub async fn ping(&self) -> Result<String> {
        let url = self.url("ping", &[]);
        let body = self.http.get(&url).send().await?.text().await?;
        let value: serde_json::Value = serde_json::from_str(&body)?;
        let inner = value
            .get("subsonic-response")
            .ok_or_else(|| anyhow!("ping: malformed response"))?;
        let meta: ResponseMeta = serde_json::from_value(inner.clone())?;
        if meta.status != "ok" {
            let err = meta
                .error
                .map(|e| format!("{} (code {})", e.message, e.code))
                .unwrap_or_else(|| "unknown error".to_string());
            bail!("authentication failed: {err}");
        }
        Ok(meta.server_version.unwrap_or(meta.version))
    }

    /// All artists, flattened out of Navidrome's per-letter index buckets.
    pub async fn artists(&self) -> Result<Vec<Artist>> {
        let response: ArtistsResponse = self.get("getArtists", &[]).await?;
        Ok(response
            .artists
            .index
            .into_iter()
            .flat_map(|index| index.artist)
            .collect())
    }

    pub async fn artist_albums(&self, artist_id: &str) -> Result<Vec<Album>> {
        let response: ArtistResponse = self.get("getArtist", &[("id", artist_id)]).await?;
        Ok(response.artist.album)
    }

    pub async fn album_songs(&self, album_id: &str) -> Result<Vec<Song>> {
        let response: AlbumResponse = self.get("getAlbum", &[("id", album_id)]).await?;
        Ok(response.album.song)
    }

    /// `kind` is a Subsonic album list type: `alphabeticalByName`, `newest`,
    /// `frequent`, `recent`, `random`, `starred`, …
    pub async fn album_list(&self, kind: &str, size: u32, offset: u32) -> Result<Vec<Album>> {
        let response: AlbumListResponse = self
            .get(
                "getAlbumList2",
                &[
                    ("type", kind),
                    ("size", &size.to_string()),
                    ("offset", &offset.to_string()),
                ],
            )
            .await?;
        Ok(response.album_list.album)
    }

    /// Extra album metadata: MusicBrainz ID and, on a server with Last.fm
    /// configured, public cover URLs that are safe to hand to third parties.
    pub async fn album_info(&self, album_id: &str) -> Result<AlbumInfo> {
        let response: AlbumInfoResponse = self.get("getAlbumInfo2", &[("id", album_id)]).await?;
        Ok(response.album_info)
    }

    /// Timed lyrics for a song, via the OpenSubsonic `songLyrics` extension.
    ///
    /// A server without the extension, or a track with none, yields empty
    /// lyrics rather than an error — the UI treats both the same way.
    pub async fn lyrics(&self, song_id: &str) -> Result<lyrics::Lyrics> {
        let response: LyricsResponse = self.get("getLyricsBySongId", &[("id", song_id)]).await?;
        Ok(lyrics::Lyrics::from_structured(
            response.lyrics_list.structured,
        ))
    }

    /// Report a play to the server.
    ///
    /// `submission = false` marks the track as "now playing"; `true` records a
    /// completed play, which is what feeds play counts and recently-played.
    pub async fn scrobble(&self, song_id: &str, submission: bool) -> Result<()> {
        let _: serde_json::Value = self
            .get(
                "scrobble",
                &[
                    ("id", song_id),
                    ("submission", if submission { "true" } else { "false" }),
                ],
            )
            .await?;
        Ok(())
    }

    pub async fn set_starred(&self, song_id: &str, starred: bool) -> Result<()> {
        let endpoint = if starred { "star" } else { "unstar" };
        let _: serde_json::Value = self.get(endpoint, &[("id", song_id)]).await?;
        Ok(())
    }

    /// Random songs, optionally restricted to a genre. The main source of
    /// candidates for radio mode.
    pub async fn random_songs(&self, count: u32, genre: Option<&str>) -> Result<Vec<Song>> {
        let count = count.to_string();
        let mut params: Vec<(&str, &str)> = vec![("size", count.as_str())];
        if let Some(genre) = genre {
            params.push(("genre", genre));
        }
        let response: RandomSongsResponse = self.get("getRandomSongs", &params).await?;
        Ok(response.random_songs.song)
    }

    /// Songs the user has starred.
    pub async fn starred_songs(&self) -> Result<Vec<Song>> {
        let response: StarredResponse = self.get("getStarred2", &[]).await?;
        Ok(response.starred.song)
    }

    pub async fn playlists(&self) -> Result<Vec<Playlist>> {
        let response: PlaylistsResponse = self.get("getPlaylists", &[]).await?;
        Ok(response.playlists.playlist)
    }

    pub async fn playlist_songs(&self, playlist_id: &str) -> Result<Vec<Song>> {
        let response: PlaylistResponse = self.get("getPlaylist", &[("id", playlist_id)]).await?;
        Ok(response.playlist.entry)
    }

    /// A page of the whole song library.
    ///
    /// Subsonic has no "list all songs" call; an empty `search3` query is the
    /// conventional way to ask for everything, and it paginates.
    pub async fn all_songs(&self, count: u32, offset: u32) -> Result<Vec<Song>> {
        let response: Search3Response = self
            .get(
                "search3",
                &[
                    ("query", ""),
                    ("artistCount", "0"),
                    ("albumCount", "0"),
                    ("songCount", &count.to_string()),
                    ("songOffset", &offset.to_string()),
                ],
            )
            .await?;
        Ok(response.result.song)
    }

    /// Server-side similar songs. Requires Last.fm on Navidrome, so callers
    /// must tolerate an empty list rather than treating it as a failure.
    pub async fn similar_songs(&self, id: &str, count: u32) -> Result<Vec<Song>> {
        let response: SimilarSongsResponse = self
            .get(
                "getSimilarSongs2",
                &[("id", id), ("count", &count.to_string())],
            )
            .await?;
        Ok(response.similar_songs.song)
    }

    /// Artists the server considers close to this one. Same caveat as
    /// [`Self::similar_songs`].
    pub async fn similar_artists(&self, artist_id: &str, count: u32) -> Result<Vec<Artist>> {
        let response: ArtistInfoResponse = self
            .get(
                "getArtistInfo2",
                &[("id", artist_id), ("count", &count.to_string())],
            )
            .await?;
        Ok(response.artist_info.similar_artist)
    }

    /// An artist's most-played tracks, keyed by name rather than id.
    pub async fn top_songs(&self, artist_name: &str, count: u32) -> Result<Vec<Song>> {
        let response: TopSongsResponse = self
            .get(
                "getTopSongs",
                &[("artist", artist_name), ("count", &count.to_string())],
            )
            .await?;
        Ok(response.top_songs.song)
    }

    pub async fn songs_by_genre(&self, genre: &str, count: u32, offset: u32) -> Result<Vec<Song>> {
        let response: SongsByGenreResponse = self
            .get(
                "getSongsByGenre",
                &[
                    ("genre", genre),
                    ("count", &count.to_string()),
                    ("offset", &offset.to_string()),
                ],
            )
            .await?;
        Ok(response.songs_by_genre.song)
    }

    pub async fn genres(&self) -> Result<Vec<Genre>> {
        let response: GenresResponse = self.get("getGenres", &[]).await?;
        Ok(response.genres.genre)
    }

    /// A 1–5 star rating, or 0 to clear it.
    pub async fn set_rating(&self, song_id: &str, rating: u8) -> Result<()> {
        let _: serde_json::Value = self
            .get(
                "setRating",
                &[("id", song_id), ("rating", &rating.min(5).to_string())],
            )
            .await?;
        Ok(())
    }

    /// Create a public share link.
    ///
    /// `expires` is milliseconds since the epoch, as the Subsonic API defines
    /// it. Fails when sharing is disabled server-side.
    pub async fn create_share(
        &self,
        ids: &[String],
        description: &str,
        expires_ms: Option<i64>,
        downloadable: bool,
    ) -> Result<Share> {
        let expires = expires_ms.map(|ms| ms.to_string());
        let mut params: Vec<(&str, &str)> = Vec::new();
        for id in ids {
            params.push(("id", id.as_str()));
        }
        if !description.is_empty() {
            params.push(("description", description));
        }
        if let Some(expires) = expires.as_deref() {
            params.push(("expires", expires));
        }
        params.push(("downloadable", if downloadable { "true" } else { "false" }));

        let response: SharesResponse = self.get("createShare", &params).await?;
        response
            .shares
            .share
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("server accepted the share but returned no link"))
    }

    pub async fn create_playlist(&self, name: &str, song_ids: &[String]) -> Result<()> {
        let mut params: Vec<(&str, &str)> = vec![("name", name)];
        for id in song_ids {
            params.push(("songId", id.as_str()));
        }
        let _: serde_json::Value = self.get("createPlaylist", &params).await?;
        Ok(())
    }

    pub async fn add_to_playlist(&self, playlist_id: &str, song_ids: &[String]) -> Result<()> {
        let mut params: Vec<(&str, &str)> = vec![("playlistId", playlist_id)];
        for id in song_ids {
            params.push(("songIdToAdd", id.as_str()));
        }
        let _: serde_json::Value = self.get("updatePlaylist", &params).await?;
        Ok(())
    }

    pub async fn remove_from_playlist(&self, playlist_id: &str, indices: &[usize]) -> Result<()> {
        let indices: Vec<String> = indices.iter().map(|i| i.to_string()).collect();
        let mut params: Vec<(&str, &str)> = vec![("playlistId", playlist_id)];
        for index in &indices {
            params.push(("songIndexToRemove", index.as_str()));
        }
        let _: serde_json::Value = self.get("updatePlaylist", &params).await?;
        Ok(())
    }

    pub async fn search(&self, query: &str, count: u32) -> Result<SearchResult3> {
        let count = count.to_string();
        let response: Search3Response = self
            .get(
                "search3",
                &[
                    ("query", query),
                    ("artistCount", &count),
                    ("albumCount", &count),
                    ("songCount", &count),
                ],
            )
            .await?;
        Ok(response.result)
    }

    /// Fetch cover art bytes at the given square size.
    pub async fn cover_art(&self, cover_id: &str, size: u32) -> Result<Vec<u8>> {
        let url = self.url(
            "getCoverArt",
            &[("id", cover_id), ("size", &size.to_string())],
        );
        let response = self
            .http
            .get(&url)
            .send()
            .await
            .context("requesting cover art")?;
        if !response.status().is_success() {
            bail!("cover art request returned HTTP {}", response.status());
        }
        Ok(response
            .bytes()
            .await
            .context("reading cover art")?
            .to_vec())
    }

    /// URL for streaming a song, optionally starting partway in.
    ///
    /// Navidrome honours `timeOffset` when it transcodes; for a raw stream it
    /// is ignored and playback starts from the beginning.
    ///
    /// `force_transcode` asks the server to re-encode even for a format we
    /// would normally decode ourselves. The player uses it to recover from a
    /// file our decoders reject, so an odd file degrades instead of failing.
    pub fn stream_url(
        &self,
        song_id: &str,
        suffix: Option<&str>,
        offset: std::time::Duration,
        force_transcode: bool,
    ) -> String {
        let offset_secs = offset.as_secs().to_string();
        let base_format = self.format_for(suffix);
        // Subsonic servers only honor timeOffset when transcoding, so if offset is non-zero
        // and format is raw, force transcoding to the fallback format (e.g. mp3/opus).
        let format = if force_transcode || (!offset.is_zero() && base_format == "raw") {
            TRANSCODE_FALLBACK
        } else {
            base_format
        };
        let mut params: Vec<(&str, &str)> = vec![("id", song_id), ("format", format)];
        if !offset.is_zero() {
            params.push(("timeOffset", offset_secs.as_str()));
        }
        self.url("stream", &params)
    }

    /// Choose the stream format for a song.
    ///
    /// An explicit `format` in the config always wins. Otherwise we stream the
    /// original bytes, except for codecs the built-in decoder cannot handle,
    /// which the server transcodes for us.
    fn format_for(&self, suffix: Option<&str>) -> &str {
        if self.format != "raw" {
            return &self.format;
        }
        match suffix {
            Some(suffix) if !is_decodable(suffix) => TRANSCODE_FALLBACK,
            _ => "raw",
        }
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }
}

/// Generate a random salt and the matching `md5(password + salt)` token.
fn auth_token(password: &str) -> (String, String) {
    let salt = random_salt(SALT_LEN);
    let token = md5_hex(&format!("{password}{salt}"));
    (salt, token)
}

fn md5_hex(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn random_salt(len: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    (0..len)
        .map(|_| ALPHABET[rand::random_range(0..ALPHABET.len())] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_matches_subsonic_spec_vector() {
        // From the Subsonic API docs: password "sesame", salt "c19b2d",
        // token md5("sesamec19b2d") = 26719a1196d2a940705a59634eb18eab.
        assert_eq!(md5_hex("sesamec19b2d"), "26719a1196d2a940705a59634eb18eab");
    }

    #[test]
    fn salt_is_random_and_correct_length() {
        let a = random_salt(SALT_LEN);
        let b = random_salt(SALT_LEN);
        assert_eq!(a.len(), SALT_LEN);
        assert_ne!(a, b, "two salts should not collide");
    }

    #[test]
    fn url_includes_auth_and_encodes_params() {
        let client = SubsonicClient::new("https://music.test/", "user name", "pw", None).unwrap();
        let url = client.url("search3", &[("query", "a b&c")]);
        assert!(url.starts_with("https://music.test/rest/search3?"));
        assert!(url.contains("u=user%20name"));
        assert!(url.contains("query=a%20b%26c"));
        assert!(url.contains("f=json"));
        assert!(!url.contains("pw"), "password must never appear in the URL");
    }

    #[test]
    fn streams_decodable_formats_untouched() {
        let client = SubsonicClient::new("https://music.test", "u", "p", None).unwrap();
        // Opus included: we decode it natively via libopus, so it must stream
        // raw rather than being re-encoded by the server.
        for suffix in ["opus", "flac", "mp3", "m4a", "FLAC", "OPUS"] {
            assert_eq!(
                client.format_for(Some(suffix)),
                "raw",
                "{suffix} is decodable"
            );
        }
    }

    #[test]
    fn transcodes_formats_the_decoder_cannot_handle() {
        let client = SubsonicClient::new("https://music.test", "u", "p", None).unwrap();
        assert_eq!(client.format_for(Some("wma")), "mp3");
        assert_eq!(client.format_for(Some("ape")), "mp3");
    }

    #[test]
    fn explicit_config_format_overrides_per_song_choice() {
        let client = SubsonicClient::new("https://music.test", "u", "p", Some("ogg")).unwrap();
        assert_eq!(client.format_for(Some("flac")), "ogg");
        assert_eq!(client.format_for(Some("opus")), "ogg");
    }

    #[test]
    fn unknown_suffix_streams_raw() {
        let client = SubsonicClient::new("https://music.test", "u", "p", None).unwrap();
        assert_eq!(client.format_for(None), "raw");
    }

    #[test]
    fn stream_url_carries_offset_only_when_seeking() {
        let client = SubsonicClient::new("https://music.test", "u", "p", None).unwrap();
        let start = client.stream_url("s1", Some("wma"), std::time::Duration::ZERO, false);
        assert!(start.contains("format=mp3"));
        assert!(!start.contains("timeOffset"));

        let seeked =
            client.stream_url("s1", Some("wma"), std::time::Duration::from_secs(42), false);
        assert!(seeked.contains("timeOffset=42"));
    }

    #[test]
    fn forced_transcode_overrides_a_natively_decodable_format() {
        let client = SubsonicClient::new("https://music.test", "u", "p", None).unwrap();
        // Recovery path: a file our decoders reject is re-requested as MP3
        // even though its format is normally streamed raw.
        let url = client.stream_url("s1", Some("flac"), std::time::Duration::ZERO, true);
        assert!(url.contains("format=mp3"));
        let normal = client.stream_url("s1", Some("flac"), std::time::Duration::ZERO, false);
        assert!(normal.contains("format=raw"));
    }

    #[test]
    fn parses_error_envelope() {
        let body = r#"{"subsonic-response":{"status":"failed","version":"1.16.1",
            "error":{"code":40,"message":"Wrong username or password"}}}"#;
        let value: serde_json::Value = serde_json::from_str(body).unwrap();
        let meta: ResponseMeta =
            serde_json::from_value(value["subsonic-response"].clone()).unwrap();
        assert_eq!(meta.status, "failed");
        assert_eq!(meta.error.unwrap().code, 40);
    }

    #[test]
    fn flattens_artist_index_buckets() {
        let body = r#"{"artists":{"index":[
            {"name":"A","artist":[{"id":"1","name":"Aphex","albumCount":3}]},
            {"name":"B","artist":[{"id":"2","name":"Boards","albumCount":5}]}]}}"#;
        let parsed: ArtistsResponse = serde_json::from_str(body).unwrap();
        let flat: Vec<_> = parsed
            .artists
            .index
            .into_iter()
            .flat_map(|i| i.artist)
            .collect();
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[1].name, "Boards");
    }
}
