//! Online lyrics fetching via LRCLIB (https://lrclib.net).
//!
//! When a track has no lyrics in the server or local tags, Wander can query
//! LRCLIB's free public REST API for synced (`.lrc`) or plain text lyrics.

use serde::Deserialize;

use super::lyrics::{LyricSet, Lyrics};
use super::models::Song;
use crate::config::LyricsConfig;

#[derive(Debug, Clone, Deserialize)]
pub struct LrclibResponse {
    #[serde(rename = "syncedLyrics")]
    pub synced_lyrics: Option<String>,
    #[serde(rename = "plainLyrics")]
    pub plain_lyrics: Option<String>,
    #[serde(rename = "trackName")]
    #[allow(dead_code)]
    pub track_name: Option<String>,
    #[serde(rename = "artistName")]
    #[allow(dead_code)]
    pub artist_name: Option<String>,
    #[serde(rename = "albumName")]
    #[allow(dead_code)]
    pub album_name: Option<String>,
    #[allow(dead_code)]
    pub duration: Option<f64>,
}

impl LrclibResponse {
    pub fn to_lyric_set(&self) -> Option<LyricSet> {
        if let Some(synced) = &self.synced_lyrics
            && !synced.trim().is_empty()
        {
            let mut lyrics = Lyrics::parse_lrc(synced);
            if !lyrics.is_empty() {
                lyrics.lang = Some("lrclib (synced)".to_string());
                return Some(LyricSet::from(lyrics));
            }
        }

        if let Some(plain) = &self.plain_lyrics
            && !plain.trim().is_empty()
        {
            let mut lyrics = Lyrics::parse_lrc(plain);
            if !lyrics.is_empty() {
                lyrics.lang = Some("lrclib".to_string());
                return Some(LyricSet::from(lyrics));
            }
        }

        None
    }
}

/// Remove feature attributes and remaster suffixes to improve search matching.
pub fn clean_title(title: &str) -> String {
    let mut cleaned = title.trim();
    for separator in [
        " (feat.", " (Feat.", " [feat.", " [Feat.", " (with ", " [with ", " (ft.", " [ft.",
    ] {
        if let Some(pos) = cleaned.find(separator) {
            cleaned = &cleaned[..pos];
        }
    }
    for suffix in [" (Remaster", " [Remaster", " (20", " (19"] {
        if let Some(pos) = cleaned.find(suffix) {
            cleaned = &cleaned[..pos];
        }
    }
    cleaned.trim().to_string()
}

fn build_url(endpoint: &str, params: &[(&str, &str)]) -> String {
    let mut url = endpoint.to_string();
    let mut first = true;
    for (k, v) in params {
        if first {
            url.push('?');
            first = false;
        } else {
            url.push('&');
        }
        url.push_str(k);
        url.push('=');
        url.push_str(&urlencoding::encode(v));
    }
    url
}

/// Fetch lyrics for a song from LRCLIB.
pub async fn fetch_online_lyrics(
    http: &reqwest::Client,
    config: &LyricsConfig,
    song: &Song,
) -> Option<LyricSet> {
    if !config.fetch_online {
        return None;
    }

    let base_url = if config.lrclib_url.trim().is_empty() {
        "https://lrclib.net"
    } else {
        config.lrclib_url.trim().trim_end_matches('/')
    };

    let title = song.title.trim();
    let artist = song.artist.as_deref().unwrap_or("").trim();
    let album = song.album.as_deref().unwrap_or("").trim();
    let duration = song.duration;

    // 1. Try GET /api/get for an exact metadata hit
    let mut query = vec![("track_name", title)];
    if !artist.is_empty() {
        query.push(("artist_name", artist));
    }
    if !album.is_empty() {
        query.push(("album_name", album));
    }
    let duration_str = duration.to_string();
    if duration > 0 {
        query.push(("duration", &duration_str));
    }

    let get_url = build_url(&format!("{base_url}/api/get"), &query);
    if let Ok(resp) = http.get(&get_url).send().await
        && resp.status().is_success()
        && let Ok(data) = resp.json::<LrclibResponse>().await
        && let Some(set) = data.to_lyric_set()
    {
        return Some(set);
    }

    // 2. Try GET /api/search with original title and artist
    let search_endpoint = format!("{base_url}/api/search");
    let mut search_query = vec![("track_name", title)];
    if !artist.is_empty() {
        search_query.push(("artist_name", artist));
    }
    let search_url = build_url(&search_endpoint, &search_query);

    if let Ok(resp) = http.get(&search_url).send().await
        && resp.status().is_success()
        && let Ok(results) = resp.json::<Vec<LrclibResponse>>().await
    {
        for res in results {
            if let Some(set) = res.to_lyric_set() {
                return Some(set);
            }
        }
    }

    // 3. Try GET /api/search with cleaned title
    let cleaned = clean_title(title);
    if cleaned != title && !cleaned.is_empty() {
        let mut clean_query = vec![("track_name", cleaned.as_str())];
        if !artist.is_empty() {
            clean_query.push(("artist_name", artist));
        }
        let clean_url = build_url(&search_endpoint, &clean_query);

        if let Ok(resp) = http.get(&clean_url).send().await
            && resp.status().is_success()
            && let Ok(results) = resp.json::<Vec<LrclibResponse>>().await
        {
            for res in results {
                if let Some(set) = res.to_lyric_set() {
                    return Some(set);
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_title_strips_features_and_remaster_tags() {
        assert_eq!(clean_title("Starboy (feat. Daft Punk)"), "Starboy");
        assert_eq!(
            clean_title("Bohemian Rhapsody (2011 Remaster)"),
            "Bohemian Rhapsody"
        );
        assert_eq!(clean_title("Plain Song Title"), "Plain Song Title");
    }

    #[test]
    fn lrclib_response_converts_synced_and_plain_lyrics() {
        let synced = LrclibResponse {
            synced_lyrics: Some("[00:10.00]Hello world".into()),
            plain_lyrics: Some("Hello world".into()),
            track_name: Some("Test".into()),
            artist_name: Some("Artist".into()),
            album_name: None,
            duration: Some(120.0),
        };
        let set = synced.to_lyric_set().expect("should parse synced");
        assert!(set.active().synced);
        assert_eq!(set.active().lines[0].text, "Hello world");
        assert_eq!(set.active().lang.as_deref(), Some("lrclib (synced)"));

        let plain = LrclibResponse {
            synced_lyrics: None,
            plain_lyrics: Some("Plain lyrics line".into()),
            track_name: Some("Test".into()),
            artist_name: Some("Artist".into()),
            album_name: None,
            duration: Some(120.0),
        };
        let set = plain.to_lyric_set().expect("should parse plain");
        assert!(!set.active().synced);
        assert_eq!(set.active().lines[0].text, "Plain lyrics line");
        assert_eq!(set.active().lang.as_deref(), Some("lrclib"));
    }
}
