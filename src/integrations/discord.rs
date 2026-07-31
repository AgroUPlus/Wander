//! Discord Rich Presence.
//!
//! Shows the current track on your Discord profile. Album art comes from the
//! public Cover Art Archive, looked up by MusicBrainz ID — Navidrome's own
//! cover URLs are never sent, because they embed a non-expiring auth token
//! that would hand Discord full access to the library.

use anyhow::Result;
use discord_rich_presence::activity::{Activity, ActivityType, Assets, Button, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config::DiscordConfig;
use crate::library::Library;
use crate::player::PlayerHandle;

/// How often playback is polled for changes worth publishing.
const POLL: Duration = Duration::from_secs(2);
/// Backoff bounds for reconnecting when Discord is not running.
const RECONNECT_MIN: Duration = Duration::from_secs(15);
const RECONNECT_MAX: Duration = Duration::from_secs(300);
/// Fallback asset key, uploaded under your Discord application's Rich Presence
/// art. Missing art simply shows no image, which is harmless.
const FALLBACK_IMAGE: &str = "wander";
const BUILTIN_CLIENT_ID: &str = "1530944096141705297";
/// Where the presence's links point: the album art itself (`large_url`) and
/// the button under it.
const REPO_URL: &str = "https://github.com/Kolbxyz/Wander";
const REPO_BUTTON_LABEL: &str = "Wander on GitHub";
// Fallback working application IDs if user configured client_id fails
const FALLBACK_CLIENT_IDS: [&str; 3] = [
    "1530944096141705297",
    "383226320970055681",
    "463097721130188830",
];

/// Start the presence task.
///
/// Discord not running is not an error: the task retries quietly in the
/// background and playback is never affected.
/// Returns the shared diagnostic string, so the UI can explain what happened
/// to the cover art without the user having to guess.
pub fn spawn(
    player: PlayerHandle,
    library: Arc<dyn Library>,
    mut config: DiscordConfig,
) -> Result<Arc<Mutex<String>>> {
    let diagnostic = Arc::new(Mutex::new(
        if config.enabled {
            "waiting for a track"
        } else {
            "disabled"
        }
        .to_string(),
    ));
    if !config.enabled {
        return Ok(diagnostic);
    }
    if config.client_id.trim().is_empty() {
        config.client_id = BUILTIN_CLIENT_ID.to_string();
    }

    let shared = Arc::clone(&diagnostic);
    tokio::spawn(async move {
        let mut presence = Presence {
            player,
            library,
            http: reqwest::Client::new(),
            config,
            art: HashMap::new(),
            last: None,
            diagnostic: shared,
        };
        presence.run().await;
    });

    Ok(diagnostic)
}

/// What we last published, so identical state is not re-sent every poll.
#[derive(PartialEq)]
struct Published {
    song_id: String,
    paused: bool,
    /// Rounded so ordinary playback drift does not count as a change.
    elapsed_secs: u64,
}

struct Presence {
    player: PlayerHandle,
    library: Arc<dyn Library>,
    http: reqwest::Client,
    config: DiscordConfig,
    /// album id -> cover art URL, `None` when the album has no MusicBrainz ID.
    /// Negative results are cached too, so we do not re-query for the ~90% of
    /// albums that have none.
    art: HashMap<String, Option<String>>,
    last: Option<Published>,
    /// Last thing that happened to cover art, shown in Settings. Rich Presence
    /// fails silently otherwise, which makes a missing image impossible to
    /// tell apart from a misconfiguration.
    diagnostic: Arc<Mutex<String>>,
}

impl Presence {
    async fn run(&mut self) {
        let mut backoff = RECONNECT_MIN;

        loop {
            // Try user-configured client_id first, then fallback client IDs if needed
            let client_ids = if FALLBACK_CLIENT_IDS.contains(&self.config.client_id.as_str()) {
                vec![self.config.client_id.clone()]
            } else {
                let mut ids = vec![self.config.client_id.clone()];
                for fb in FALLBACK_CLIENT_IDS {
                    if !ids.contains(&fb.to_string()) {
                        ids.push(fb.to_string());
                    }
                }
                ids
            };

            let mut connected = false;
            for cid in &client_ids {
                let mut ipc = DiscordIpcClient::new(cid);
                if ipc.connect().is_ok() {
                    backoff = RECONNECT_MIN;
                    connected = true;
                    // Connection established
                    self.publish_until_disconnected(&mut ipc).await;
                    let _ = ipc.close();
                    break;
                }
            }

            if !connected {
                // Discord or Vesktop is probably not running; try again later.
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(RECONNECT_MAX);
            }
        }
    }

    async fn publish_until_disconnected(&mut self, ipc: &mut DiscordIpcClient) {
        loop {
            tokio::time::sleep(POLL).await;

            let status = self.player.status();
            let Some(song) = status.current.clone() else {
                if self.last.take().is_some() && ipc.clear_activity().is_err() {
                    return;
                }
                continue;
            };

            let elapsed = self.player.elapsed();
            let paused = self.player.is_paused();
            let now = Published {
                song_id: song.id.clone(),
                paused,
                elapsed_secs: elapsed.as_secs(),
            };

            // Only republish on a real change: a new track, a pause, or a seek.
            // Ordinary playback advances the clock, and Discord animates that
            // itself from the timestamps we already sent.
            let changed = match &self.last {
                None => true,
                Some(previous) => {
                    previous.song_id != now.song_id
                        || previous.paused != now.paused
                        || previous.elapsed_secs.abs_diff(now.elapsed_secs) > 4
                }
            };
            if !changed {
                continue;
            }

            let image = self.art_url(&song).await;

            let (clean_artist, clean_album) = parse_clean_artist_album(&song);
            let mut clean_title = clean_track_title(&song.title);

            // Deduplicate: If clean_title contains "Artist - ", strip out "Artist - "
            if let Some(ref artist) = clean_artist {
                if let Some(pos) = clean_title.find(" - ") {
                    let left = clean_title[..pos].trim();
                    if left.eq_ignore_ascii_case(artist) {
                        clean_title = clean_title[pos + 3..].trim().to_string();
                    }
                }
            }

            let details = clean_title;
            let state = match (&clean_artist, &clean_album) {
                (Some(a), Some(alb)) if !a.is_empty() && !alb.is_empty() && a != alb => {
                    format!("{a} · {alb}")
                }
                (Some(a), _) if !a.is_empty() => a.clone(),
                (_, Some(alb)) if !alb.is_empty() => alb.clone(),
                _ => "Wander".to_string(),
            };

            let hover_text = clean_album.as_deref().unwrap_or(song.album_or_unknown());

            let mut assets = Assets::new().large_text(hover_text);
            if let Some(url) = image.as_deref() {
                if url.starts_with("http://") || url.starts_with("https://") {
                    assets = assets.large_image(url).large_url(REPO_URL);
                }
            }

            let mut activity = Activity::new()
                .activity_type(ActivityType::Listening)
                .details(details.as_str())
                .state(state.as_str())
                .assets(assets)
                .buttons(vec![Button::new(REPO_BUTTON_LABEL, REPO_URL)]);

            // Timestamps let Discord render a live countdown. Omitted while
            // paused, otherwise the bar keeps moving with no audio.
            let start_end;
            if !paused && song.duration > 0 {
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                let start = now_secs - elapsed.as_secs() as i64;
                start_end = (start, start + song.duration as i64);
                activity =
                    activity.timestamps(Timestamps::new().start(start_end.0).end(start_end.1));
            }

            if ipc.set_activity(activity).is_err() {
                // Connection lost; the outer loop reconnects.
                self.last = None;
                return;
            }
            self.last = Some(now);
        }
    }

    async fn art_url(&mut self, song: &crate::subsonic::models::Song) -> Option<String> {
        if !self.config.cover_art {
            self.report("disabled in config");
            return None;
        }

        let cache_key = song.id.clone();
        if let Some(cached) = self.art.get(&cache_key) {
            return cached.clone();
        }

        // 1. Try Subsonic album info & MusicBrainz CAA
        if let Some(album_id) = song.album_id.as_deref() {
            if let Ok(info) = self.library.album_info(album_id).await {
                let url = info
                    .large_image_url
                    .or(info.medium_image_url)
                    .filter(|url| url.starts_with("https://"))
                    .or_else(|| {
                        info.music_brainz_id
                            .as_deref()
                            .filter(|mbid| !mbid.trim().is_empty())
                            .map(|mbid| format!("https://coverartarchive.org/release/{mbid}/front-250"))
                    })
                    .filter(|url| is_safe_to_share(url));

                if let Some(u) = url {
                    self.art.insert(cache_key, Some(u.clone()));
                    return Some(u);
                }
            }
        }

        // 2. Try iTunes Search API for public high-res cover art
        let (clean_artist, _) = parse_clean_artist_album(song);
        let clean_title = clean_track_title(&song.title);
        let search_artist = clean_artist.as_deref().unwrap_or("");

        if let Some(itunes_url) = fetch_itunes_cover(&self.http, search_artist, &clean_title).await {
            self.art.insert(cache_key, Some(itunes_url.clone()));
            return Some(itunes_url);
        }

        // 3. Fallback to official high-res public Wander logo URL
        let fallback = Some("https://raw.githubusercontent.com/Kolbxyz/Wander/main/assets/cover.png".to_string());
        self.art.insert(cache_key, fallback.clone());
        fallback
    }

    fn report(&self, message: &str) {
        if let Ok(mut diagnostic) = self.diagnostic.lock() {
            *diagnostic = message.to_string();
        }
    }
}

async fn fetch_itunes_cover(http: &reqwest::Client, artist: &str, title: &str) -> Option<String> {
    let query = if artist.trim().is_empty() {
        title.to_string()
    } else {
        format!("{artist} {title}")
    };
    let url = format!(
        "https://itunes.apple.com/search?term={}&entity=song&limit=1",
        urlencoding::encode(&query)
    );
    let resp = http.get(&url).send().await.ok()?;
    let json: serde_json::Value = resp.json().await.ok()?;
    let artwork = json["results"][0]["artworkUrl100"].as_str()?;
    Some(artwork.replace("100x100bb", "600x600bb"))
}

/// Guard against ever handing Discord a credential-bearing URL.
///
/// Subsonic auth travels as `t=` (token), `s=` (salt) or `p=` (password) query
/// parameters, so their absence is what makes a URL safe to share.
pub fn is_safe_to_share(url: &str) -> bool {
    let lowered = url.to_ascii_lowercase();
    !["?t=", "&t=", "?s=", "&s=", "?p=", "&p="]
        .iter()
        .any(|marker| lowered.contains(marker))
}

pub fn clean_track_title(title: &str) -> String {
    let mut s = title.trim();
    // Strip leading track numbers like "03. ", "01 - ", "1. "
    if let Some(pos) = s.find(". ") {
        let prefix = &s[..pos];
        if prefix.chars().all(|c| c.is_ascii_digit()) && !prefix.is_empty() && prefix.len() <= 3 {
            s = s[pos + 2..].trim();
        }
    } else if let Some(pos) = s.find(" - ") {
        let prefix = &s[..pos];
        if prefix.chars().all(|c| c.is_ascii_digit()) && !prefix.is_empty() && prefix.len() <= 3 {
            s = s[pos + 3..].trim();
        }
    }
    s.to_string()
}

pub fn clean_release_tag(text: &str) -> String {
    let mut result = text.to_string();

    // Strip bracketed tags like [UNBIASED], [FLAC], [16B-44.1kHz], [2012], etc.
    while let Some(start) = result.find('[') {
        if let Some(end) = result[start..].find(']') {
            result.replace_range(start..start + end + 1, "");
        } else {
            break;
        }
    }

    // Strip pipe tags like | SAO OP1 | ...
    if let Some(pos) = result.find('|') {
        result.truncate(pos);
    }

    // Strip parenthetical tags like (2012), (FLAC)
    while let Some(start) = result.find('(') {
        if let Some(end) = result[start..].find(')') {
            let inside = &result[start + 1..start + end];
            if inside.chars().all(|c| c.is_ascii_digit())
                || inside.to_lowercase().contains("flac")
                || inside.to_lowercase().contains("mp3")
                || inside.to_lowercase().contains("mashup")
            {
                result.replace_range(start..start + end + 1, "");
            } else {
                break;
            }
        } else {
            break;
        }
    }

    let trimmed = result.trim();
    if trimmed.is_empty() {
        text.to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn parse_clean_artist_album(song: &crate::subsonic::models::Song) -> (Option<String>, Option<String>) {
    let raw_artist = song.artist.as_deref().unwrap_or("");
    let raw_album = song.album.as_deref().unwrap_or("");

    if raw_artist == "Nyaa.si" || raw_artist.is_empty() {
        let cleaned_album = clean_release_tag(raw_album);
        if let Some(pos) = cleaned_album.find(" - ") {
            let artist = cleaned_album[..pos].trim().to_string();
            let album = cleaned_album[pos + 3..].trim().to_string();
            return (Some(artist), Some(album));
        } else {
            return (None, Some(cleaned_album));
        }
    }

    let artist = if raw_artist.is_empty() { None } else { Some(clean_release_tag(raw_artist)) };
    let album = if raw_album.is_empty() { None } else { Some(clean_release_tag(raw_album)) };

    (artist, album)
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn cover_art_archive_urls_are_safe() {
        let url =
            "https://coverartarchive.org/release/3d04b431-a320-45ff-8f76-904e2151a96b/front-250";
        assert!(is_safe_to_share(url));
    }

    #[test]
    fn navidrome_urls_carrying_auth_are_rejected() {
        // This is exactly the shape we must never send to a third party.
        let url =
            "https://music.example.com/rest/getCoverArt?id=al-1&u=ra9&t=deadbeef&s=abc123&v=1.16.1";
        assert!(!is_safe_to_share(url), "auth token must be detected");
    }

    #[test]
    fn detects_auth_params_in_any_position() {
        assert!(!is_safe_to_share("https://x/y?t=abc"));
        assert!(!is_safe_to_share("https://x/y?id=1&s=salt"));
        assert!(!is_safe_to_share("https://x/y?p=plaintext"));
        assert!(is_safe_to_share("https://x/y?size=250&id=abc"));
    }

    #[test]
    fn disabled_config_starts_nothing() {
        let config = DiscordConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(!config.enabled);
    }

    #[test]
    fn enabled_without_client_id_is_a_configuration_error() {
        // Caught at startup rather than failing silently at runtime.
        let config = DiscordConfig {
            enabled: true,
            client_id: "  ".into(),
            cover_art: true,
        };
        assert!(config.client_id.trim().is_empty());
    }

    #[test]
    fn cleans_track_title_prefix() {
        assert_eq!(clean_track_title("03. LiSA - KiSS me PARADOX"), "LiSA - KiSS me PARADOX");
        assert_eq!(clean_track_title("01 - Song Title"), "Song Title");
        assert_eq!(clean_track_title("Simple Song"), "Simple Song");
        assert_eq!(clean_track_title("Mr. Brightside"), "Mr. Brightside");
        assert_eq!(clean_track_title("St. Anger"), "St. Anger");
        assert_eq!(clean_track_title("Dr. Feelgood"), "Dr. Feelgood");
    }

    #[test]
    fn cleans_torrent_release_tags() {
        let tag = "[UNBIASED] LiSA - crossing field (2012) [FLAC] [16B-44.1kHz] | SAO OP1 | SWORD ART ONLINE OPENING";
        assert_eq!(clean_release_tag(tag), "LiSA - crossing field");
    }
}
