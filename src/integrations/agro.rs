//! Agro Background Synchronization Client.
//!
//! Synchronizes Wander's playback state, active track, and timestamp with the
//! Agro background daemon. Enables automatic, seamless handoff to Wanda (Android).
//!
//! Designed to be zero-cost:
//! - Optional: If disabled or unconfigured, no background task is spawned.
//! - Event-driven / throttled: Only sends updates on track changes or state transitions.
//! - Network-safe: Disconnections or slow daemon responses never block audio playback.

use anyhow::Result;
use reqwest::Client;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::sleep;

use crate::config::AgroConfig;
use crate::integrations::share_link::ShareDomain;
use crate::player::PlayerHandle;

const ADJECTIVES: &[&str] = &[
    "Cosmic", "Groovy", "Hyper", "Electric", "Snarky", "Velvet",
    "Neon", "Chill", "Breezy", "Sonic", "Turbo", "Funky",
    "Glitchy", "Slick", "Wobbly", "Mellow", "Quantum", "Zippy",
];

const ANIMALS: &[&str] = &[
    "Capybara", "Otter", "Badger", "Possum", "Wombat", "Gecko",
    "Pangolin", "Falcon", "Koala", "Lemur", "Quokka", "Jaguar",
    "Meerkat", "Axolotl", "Chinchilla", "Ferret", "Platypus", "Panda",
];

pub fn generate_petname(seed: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    seed.hash(&mut hasher);
    let hash = hasher.finish();

    let adj = ADJECTIVES[(hash as usize) % ADJECTIVES.len()];
    let animal = ANIMALS[((hash >> 16) as usize) % ANIMALS.len()];
    format!("{} {}", adj, animal)
}

/// One queue entry as Agro carries it.
///
/// Ids go over the wire with Wanda's `navidrome:` namespace, because the receiving client
/// identifies a backend by that prefix — a bare Subsonic id is indistinguishable from any other
/// backend's and would be resolved by searching instead of fetched exactly.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffQueueTrack {
    pub track_uri: String,
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub artwork_url: Option<String>,
}

/// The shared namespace for tracks served by the Navidrome both clients point at.
pub fn namespaced_id(song_id: &str) -> String {
    format!("navidrome:{}", song_id)
}

#[derive(Debug, Clone)]
pub struct RemoteHandoff {
    pub track_uri: String,
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub position_ms: i64,
    pub is_playing: bool,
    pub device_id: String,
    pub petname: String,
}

#[derive(Debug, Clone)]
pub struct SyncedSettings {
    pub server_url: Option<String>,
    pub server_username: Option<String>,
    pub lrclib_url: Option<String>,
    pub lyrics_fetch_online: bool,
    pub stream_format: String,
}

pub static ACTIVE_REMOTE_HANDOFF: std::sync::OnceLock<Arc<RwLock<Option<RemoteHandoff>>>> =
    std::sync::OnceLock::new();

pub fn get_remote_handoff() -> Arc<RwLock<Option<RemoteHandoff>>> {
    ACTIVE_REMOTE_HANDOFF
        .get_or_init(|| Arc::new(RwLock::new(None)))
        .clone()
}

pub struct AgroClient {
    client: Client,
    server: String,
    username: String,
    passphrase: String,
    device_id: String,
}

impl AgroClient {
    pub fn new(server: String, username: String, passphrase: String, device_id: String) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            server,
            username,
            passphrase,
            device_id,
        }
    }

    pub async fn register_node(&self, device_name: Option<&str>, current_track: Option<&str>) -> Result<Option<String>> {
        let mutation = r#"
            mutation RegisterNode($userId: String!, $deviceId: String!, $clientType: String!, $deviceName: String, $currentTrack: String) {
                registerNode(userId: $userId, deviceId: $deviceId, clientType: $clientType, deviceName: $deviceName, currentTrack: $currentTrack) {
                    petname
                }
            }
        "#;

        let body = json!({
            "query": mutation,
            "variables": {
                "userId": self.username,
                "deviceId": self.device_id,
                "clientType": "wander",
                "deviceName": device_name,
                "currentTrack": current_track,
            }
        });

        let url = format!("{}/graphql", self.server.trim_end_matches('/'));
        let res = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.passphrase))
            .json(&body)
            .send()
            .await?;

        if res.status().is_success() {
            let json_data: serde_json::Value = res.json().await?;
            let petname = json_data
                .get("data")
                .and_then(|d| d.get("registerNode"))
                .and_then(|r| r.get("petname"))
                .and_then(|p| p.as_str())
                .map(String::from);
            return Ok(petname);
        }

        Ok(None)
    }

    pub async fn update_playback_state(
        &self,
        track_uri: &str,
        title: &str,
        artist: &str,
        album: Option<&str>,
        position_ms: i64,
        is_playing: bool,
        queue: Option<Vec<HandoffQueueTrack>>,
        queue_index: Option<i32>,
    ) -> Result<()> {
        let mutation = r#"
            mutation UpdateHandoff($input: HandoffInput!) {
                updateHandoff(input: $input)
            }
        "#;

        let variables = json!({
            "input": {
                "userId": self.username,
                "trackUri": track_uri,
                "trackTitle": title,
                "artistName": artist,
                "albumName": album,
                "positionMs": position_ms,
                "isPlaying": is_playing,
                "deviceId": self.device_id,
                // Omitted on a plain heartbeat: the server keeps the queue it already has rather
                // than us re-sending an unchanged list every ten seconds.
                "queue": queue,
                "queueIndex": queue_index
            }
        });

        let body = json!({
            "query": mutation,
            "variables": variables
        });

        let url = format!("{}/graphql", self.server.trim_end_matches('/'));
        let _ = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.passphrase))
            .json(&body)
            .send()
            .await;

        Ok(())
    }

    pub async fn fetch_latest_handoff(&self) -> Result<Option<RemoteHandoff>> {
        let query = r#"
            query GetHandoff($userId: String!) {
                playbackHandoff(userId: $userId) {
                    trackUri
                    trackTitle
                    artistName
                    albumName
                    positionMs
                    isPlaying
                    deviceId
                }
            }
        "#;

        let body = json!({
            "query": query,
            "variables": {
                "userId": self.username
            }
        });

        let url = format!("{}/graphql", self.server.trim_end_matches('/'));
        let res = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.passphrase))
            .json(&body)
            .send()
            .await?;

        if !res.status().is_success() {
            return Ok(None);
        }

        let json_data: serde_json::Value = res.json().await?;
        let handoff_opt = json_data.get("data").and_then(|d| d.get("playbackHandoff"));

        if let Some(h) = handoff_opt {
            if h.is_null() {
                return Ok(None);
            }
            let track_uri = h.get("trackUri").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let track_title = h.get("trackTitle").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let artist_name = h.get("artistName").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let album_name = h.get("albumName").and_then(|v| v.as_str()).map(String::from);
            let position_ms = h.get("positionMs").and_then(|v| v.as_i64()).unwrap_or_default();
            let is_playing = h.get("isPlaying").and_then(|v| v.as_bool()).unwrap_or_default();
            let device_id = h.get("deviceId").and_then(|v| v.as_str()).unwrap_or_default().to_string();

            if !track_title.is_empty() {
                let petname = format!("Node {}", &device_id);
                return Ok(Some(RemoteHandoff {
                    track_uri,
                    track_title,
                    artist_name,
                    album_name,
                    position_ms,
                    is_playing,
                    device_id,
                    petname,
                }));
            }
        }

        Ok(None)
    }

    /// The share-link domain this server publishes, if it has one switched on.
    ///
    /// Configured once on Agro and read by every player, so the fleet agrees without the domain
    /// being typed into each one. `None` for a server that has the feature off, does not know the
    /// fields, or cannot be reached — all of which leave the local `[share]` config in charge.
    pub async fn fetch_share_domain(&self) -> Result<Option<ShareDomain>> {
        let query = r#"
            query ShareSettings($userId: String!) {
                syncedSettings(userId: $userId) {
                    shareDomain
                    shareHosts
                    shareEnabled
                }
            }
        "#;

        let body = json!({
            "query": query,
            "variables": { "userId": self.username }
        });

        let url = format!("{}/graphql", self.server.trim_end_matches('/'));
        let res = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.passphrase))
            .json(&body)
            .send()
            .await?;

        if !res.status().is_success() {
            return Ok(None);
        }

        let json_data: serde_json::Value = res.json().await?;
        let settings = json_data
            .get("data")
            .and_then(|d| d.get("syncedSettings"))
            .filter(|s| !s.is_null());
        let Some(settings) = settings else {
            return Ok(None);
        };

        if !settings
            .get("shareEnabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Ok(None);
        }
        let domain = settings
            .get("shareDomain")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if domain.is_empty() {
            return Ok(None);
        }
        let hosts = settings
            .get("shareHosts")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .split(',')
            .map(|host| host.trim().to_string())
            .filter(|host| !host.is_empty())
            .collect();

        Ok(Some(ShareDomain { domain, hosts }))
    }
}

/// Kept so shutdown can tell the server this device has stopped. Without it the last thing Agro
/// heard was "playing", and it kept saying so on every other device until something overwrote it.
pub static ACTIVE_CLIENT: std::sync::OnceLock<Arc<AgroClient>> = std::sync::OnceLock::new();

/// Announces that this device is no longer playing. Called on exit, and deliberately blocking with
/// a short timeout: the process is on its way out, so this is the last chance to be heard.
pub async fn announce_stopped(player: &PlayerHandle) {
    let Some(client) = ACTIVE_CLIENT.get() else {
        return;
    };
    let Some(song) = player.status().current.clone() else {
        return;
    };
    let position_ms = (player.elapsed().as_secs_f64() * 1000.0) as i64;
    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        client.update_playback_state(
            &namespaced_id(&song.id),
            &song.title,
            song.artist.as_deref().unwrap_or("Unknown Artist"),
            song.album.as_deref(),
            position_ms,
            false,
            None,
            None,
        ),
    )
    .await;
}

/// Spawns the lightweight Agro sync background task if enabled in configuration.
pub fn spawn(player: PlayerHandle, config: AgroConfig) -> Result<()> {
    if !config.enabled || config.passphrase.trim().is_empty() {
        return Ok(());
    }

    let client = Arc::new(AgroClient::new(
        config.server,
        config.username,
        config.passphrase,
        config.device_id,
    ));
    let _ = ACTIVE_CLIENT.set(Arc::clone(&client));

    let remote_handoff_store = get_remote_handoff();

    let initial_name = config.device_name.clone();
    tokio::spawn(async move {
        // Initial node registration
        let _ = client.register_node(initial_name.as_deref(), None).await;

        let mut last_song_id = String::new();
        let mut last_paused = true;
        let mut last_update_time = std::time::Instant::now();
        let mut check_remote_tick = 0u64;

        loop {
            let status = player.status();
            if let Some(song) = status.current.clone() {
                let elapsed = player.elapsed();
                let paused = player.is_paused();
                let is_playing = !paused;

                let track_changed = song.id != last_song_id;
                let pause_changed = paused != last_paused;
                let periodic_sync = is_playing && last_update_time.elapsed() >= Duration::from_secs(10);

                if track_changed || pause_changed || periodic_sync {
                    last_song_id = song.id.clone();
                    last_paused = paused;
                    last_update_time = std::time::Instant::now();

                    let position_ms = (elapsed.as_secs_f64() * 1000.0) as i64;
                    let client_ref = Arc::clone(&client);

                    // The queue only travels when it can have changed. A periodic position
                    // heartbeat leaves it alone, and the server keeps the stored one.
                    let (queue, queue_index) = if track_changed {
                        let (songs, position) = player.queue.lock().unwrap().in_play_order();
                        let tracks: Vec<HandoffQueueTrack> = songs
                            .iter()
                            .map(|s| HandoffQueueTrack {
                                track_uri: namespaced_id(&s.id),
                                track_title: s.title.clone(),
                                artist_name: s
                                    .artist
                                    .clone()
                                    .unwrap_or_else(|| "Unknown Artist".to_string()),
                                album_name: s.album.clone(),
                                artwork_url: None,
                            })
                            .collect();
                        (Some(tracks), position.map(|p| p as i32))
                    } else {
                        (None, None)
                    };

                    let uri = namespaced_id(&song.id);
                    let title = song.title.clone();
                    let artist = song.artist.clone().unwrap_or_else(|| "Unknown Artist".to_string());
                    let album = song.album.clone();

                    tokio::spawn(async move {
                        let _ = client_ref
                            .update_playback_state(
                                &uri,
                                &title,
                                &artist,
                                album.as_deref(),
                                position_ms,
                                is_playing,
                                queue,
                                queue_index,
                            )
                            .await;
                    });
                }
            } else {
                // If local player is idle, check remote handoff every 6 seconds
                check_remote_tick += 1;
                if check_remote_tick % 3 == 0 {
                    if let Ok(Some(remote)) = client.fetch_latest_handoff().await {
                        if remote.device_id != client.device_id && remote.is_playing {
                            let mut store = remote_handoff_store.write().await;
                            *store = Some(remote);
                        } else {
                            let mut store = remote_handoff_store.write().await;
                            *store = None;
                        }
                    }
                }
            }

            sleep(Duration::from_secs(2)).await;
        }
    });

    Ok(())
}
