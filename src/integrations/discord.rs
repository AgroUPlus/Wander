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
            let state = format!("{} · {}", song.artist_or_unknown(), song.album_or_unknown());

            // `large_url` makes the album art itself clickable.
            let mut assets = Assets::new()
                .large_text(song.album_or_unknown())
                .large_url(REPO_URL);
            assets = match image.as_deref() {
                Some(url) => assets.large_image(url),
                None => assets.large_image(FALLBACK_IMAGE),
            };

            let mut activity = Activity::new()
                .activity_type(ActivityType::Listening)
                .details(song.title.as_str())
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

    /// Public cover art URL for a song's album, if one can be derived safely.
    ///
    /// Two sources, in order of reliability:
    ///
    /// 1. `getAlbumInfo2`, which on a server with Last.fm configured returns
    ///    ready-made public image URLs;
    /// 2. the Cover Art Archive, keyed by the album's MusicBrainz release ID.
    ///
    /// Many libraries — game soundtracks especially — have neither, in which
    /// case there is genuinely no image that can be shared without leaking a
    /// credential-bearing Navidrome URL. `self.diagnostic` records which case
    /// applied so Settings can explain the blank frame.
    async fn art_url(&mut self, song: &crate::subsonic::models::Song) -> Option<String> {
        if !self.config.cover_art {
            self.report("disabled in config");
            return None;
        }
        let Some(album_id) = song.album_id.as_deref() else {
            self.report("track has no album id");
            return None;
        };

        if let Some(cached) = self.art.get(album_id) {
            return cached.clone();
        }

        let info = match self.library.album_info(album_id).await {
            Ok(info) => info,
            // A network blip must not poison the cache for the whole session,
            // so return without recording a negative result.
            Err(err) => {
                self.report(&format!("lookup failed: {err:#}"));
                return None;
            }
        };

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
            // Belt and braces: never hand out a URL carrying credentials, even
            // if the construction above is changed later.
            .filter(|url| is_safe_to_share(url));

        match url.as_deref() {
            Some(url) => self.report(url),
            None => self.report("no public cover URL for this album"),
        }
        self.art.insert(album_id.to_string(), url.clone());
        url
    }

    fn report(&self, message: &str) {
        if let Ok(mut diagnostic) = self.diagnostic.lock() {
            *diagnostic = message.to_string();
        }
    }
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
}
