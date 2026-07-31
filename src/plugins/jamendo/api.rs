use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

const USER_AGENT: &str = "wander-tui/0.1 (music player)";

/// One track from Jamendo's catalogue.
///
/// Unlike Archive (whole concerts) and nyaa (whole releases), Jamendo is
/// track-shaped: a search returns individual songs with real artist and album
/// names, which is what makes it the closest thing to a general music search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JamendoTrack {
    pub id: String,
    pub name: String,
    pub artist_name: String,
    pub album_name: String,
    /// Seconds, as reported by the API.
    pub duration: u32,
    /// Direct stream URL, already in the requested format.
    pub audio: String,
    /// Download URL, when the artist allows downloads.
    pub audiodownload: Option<String>,
    pub image: Option<String>,
    pub release_year: Option<u32>,
}

/// Audio format asked of Jamendo. FLAC where the artist provided one, which is
/// not everywhere, so the fallbacks matter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JamendoFormat {
    Flac,
    Mp3,
    Ogg,
}

impl JamendoFormat {
    pub const ALL: [JamendoFormat; 3] = [
        JamendoFormat::Flac,
        JamendoFormat::Mp3,
        JamendoFormat::Ogg,
    ];

    /// Value stored in the config file.
    pub fn code(self) -> &'static str {
        match self {
            JamendoFormat::Flac => "flac",
            JamendoFormat::Mp3 => "mp32",
            JamendoFormat::Ogg => "ogg",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            JamendoFormat::Flac => "FLAC (lossless)",
            JamendoFormat::Mp3 => "MP3 (high)",
            JamendoFormat::Ogg => "Ogg Vorbis",
        }
    }

    /// Extension the bytes actually arrive as, for the player's format hint.
    pub fn suffix(self) -> &'static str {
        match self {
            JamendoFormat::Flac => "flac",
            JamendoFormat::Mp3 => "mp3",
            JamendoFormat::Ogg => "ogg",
        }
    }

    pub fn from_code(code: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|format| format.code() == code)
            .unwrap_or(JamendoFormat::Flac)
    }
}

pub async fn search_jamendo(
    client: &Client,
    client_id: &str,
    query: &str,
    format: JamendoFormat,
) -> Result<Vec<JamendoTrack>> {
    if client_id.trim().is_empty() {
        anyhow::bail!(
            "Jamendo needs a free client ID — create one at https://devportal.jamendo.com and enter it in Settings ▸ Plugins"
        );
    }

    let url = format!(
        "https://api.jamendo.com/v3.0/tracks/?client_id={}&format=json&limit=100&search={}&audioformat={}&include=musicinfo&groupby=artist_id",
        urlencoding::encode(client_id.trim()),
        urlencoding::encode(query.trim()),
        format.code()
    );

    let response = client
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .context("Failed to reach the Jamendo API")?;

    if !response.status().is_success() {
        anyhow::bail!("Jamendo returned status code {}", response.status());
    }

    let body: serde_json::Value = response
        .json()
        .await
        .context("Failed to parse the Jamendo response")?;

    parse_search_response(&body)
}

pub fn parse_search_response(body: &serde_json::Value) -> Result<Vec<JamendoTrack>> {
    // Jamendo reports its own errors inside a 200 response, so the status code
    // alone never tells you a bad key was rejected.
    if let Some(headers) = body.get("headers") {
        let status = headers.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if status != "success" {
            let message = headers
                .get("error_message")
                .and_then(|v| v.as_str())
                .filter(|m| !m.is_empty())
                .unwrap_or("Jamendo rejected the request");
            anyhow::bail!("{message}");
        }
    }

    let results = body
        .get("results")
        .and_then(|r| r.as_array())
        .context("Jamendo response contained no results")?;

    let mut tracks = Vec::with_capacity(results.len());
    for entry in results {
        let (Some(id), Some(audio)) = (
            entry.get("id").and_then(json_string),
            entry.get("audio").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        // Only https streams are ever handed to the player.
        if !audio.starts_with("https://") {
            continue;
        }

        tracks.push(JamendoTrack {
            id,
            name: entry
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown")
                .to_string(),
            artist_name: entry
                .get("artist_name")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown artist")
                .to_string(),
            album_name: entry
                .get("album_name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            duration: entry
                .get("duration")
                .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
                .unwrap_or(0) as u32,
            audio: audio.to_string(),
            audiodownload: entry
                .get("audiodownload")
                .and_then(|v| v.as_str())
                .filter(|url| url.starts_with("https://"))
                .map(str::to_string),
            image: entry
                .get("album_image")
                .or_else(|| entry.get("image"))
                .and_then(|v| v.as_str())
                .filter(|url| url.starts_with("https://"))
                .map(str::to_string),
            release_year: entry
                .get("releasedate")
                .and_then(|v| v.as_str())
                .and_then(|date| date.get(..4)?.parse().ok()),
        });
    }

    Ok(tracks)
}

/// Jamendo returns ids as either a string or a number depending on endpoint.
fn json_string(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tracks() {
        let body: serde_json::Value = serde_json::from_str(
            r#"{"headers":{"status":"success"},"results":[
                {"id":1532771,"name":"Sunrise","artist_name":"Someone","album_name":"Dawn",
                 "duration":"183","audio":"https://prod.jamendo.com/?trackid=1&format=flac",
                 "audiodownload":"https://prod.jamendo.com/download/track/1/flac",
                 "album_image":"https://usercontent.jamendo.com/cover.jpg","releasedate":"2015-04-02"},
                {"id":2,"name":"No audio"}
            ]}"#,
        )
        .unwrap();

        let tracks = parse_search_response(&body).unwrap();
        assert_eq!(tracks.len(), 1, "a track with no stream URL is unusable");
        assert_eq!(tracks[0].id, "1532771");
        assert_eq!(tracks[0].duration, 183);
        assert_eq!(tracks[0].release_year, Some(2015));
    }

    /// Jamendo reports a rejected key inside a 200 response, so this is the
    /// only place a bad client ID can be noticed.
    #[test]
    fn surfaces_api_level_errors() {
        let body: serde_json::Value = serde_json::from_str(
            r#"{"headers":{"status":"failed","error_message":"Invalid Client Id"},"results":[]}"#,
        )
        .unwrap();

        let err = parse_search_response(&body).unwrap_err();
        assert!(err.to_string().contains("Invalid Client Id"));
    }

    #[test]
    fn insecure_streams_are_rejected() {
        let body: serde_json::Value = serde_json::from_str(
            r#"{"headers":{"status":"success"},"results":[
                {"id":1,"name":"x","audio":"http://insecure.example/track.mp3"}
            ]}"#,
        )
        .unwrap();
        assert!(parse_search_response(&body).unwrap().is_empty());
    }
}
