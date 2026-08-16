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

    pub async fn create_short_link(&self, target_url: &str) -> Result<String> {
        let mutation = r#"
            mutation CreateShortLink($userId: String, $targetUrl: String!, $source: String) {
                createShortLink(userId: $userId, targetUrl: $targetUrl, source: $source)
            }
        "#;
        // Attributed to the account, which is what makes the link appear in Agro's link manager —
        // an unowned link could be minted but never listed, counted or revoked.
        let body = json!({
            "query": mutation,
            "variables": {
                "userId": self.username,
                "targetUrl": target_url,
                "source": "navidrome"
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
            anyhow::bail!("Agro server returned status {}", res.status());
        }
        let json_data: serde_json::Value = res.json().await?;
        let uid = json_data
            .get("data")
            .and_then(|d| d.get("createShortLink"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Failed to extract createShortLink UID from Agro response"))?;
        Ok(uid.to_string())
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
    /// Reports completed plays.
    ///
    /// The server is idempotent on (account, artist, title, time), so re-sending a batch this
    /// client was unsure about is safe — which is what lets the outbox retry rather than having to
    /// know whether a timed-out request actually landed.
    async fn record_scrobbles(&self, device_name: &str, plays: &[PendingScrobble]) -> Result<()> {
        let mutation = r#"
            mutation RecordScrobbles(
                $userId: String!, $deviceName: String!, $clientType: String,
                $entries: [ScrobbleInput!]!
            ) {
                recordScrobbles(
                    userId: $userId, deviceName: $deviceName, clientType: $clientType,
                    entries: $entries
                )
            }
        "#;

        let entries: Vec<serde_json::Value> = plays
            .iter()
            .map(|play| {
                json!({
                    "trackTitle": play.title,
                    "artistName": play.artist,
                    "albumName": play.album,
                    // One genre, because that is what the server stores. The first is the primary
                    // one on every backend that reports more than one.
                    "genre": play.genres.first(),
                    "durationSecs": play.secs as i64,
                    "playedAt": rfc3339(play.at),
                })
            })
            .collect();

        let body = json!({
            "query": mutation,
            "variables": {
                "userId": self.username,
                "deviceName": device_name,
                "clientType": "wander",
                "entries": entries,
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
            anyhow::bail!("agro rejected the scrobble batch: {}", res.status());
        }
        // A GraphQL error arrives with a 200, so the body has to be looked at too — otherwise a
        // rejected batch is silently dropped from the outbox.
        let payload: serde_json::Value = res.json().await?;
        if payload.get("errors").is_some_and(|e| !e.is_null()) {
            anyhow::bail!("agro rejected the scrobble batch");
        }
        Ok(())
    }

    /// The fleet's listening statistics, in the same shape the local ones are computed in.
    ///
    /// Returning [`crate::history::Stats`] rather than a type of its own is the point: every
    /// consumer — the Home tab, the mix seeding, the Discover shelf — keeps reading exactly what it
    /// always read, and only where the numbers come from changes.
    pub async fn fetch_stats(&self, period: &str) -> Result<crate::history::Stats> {
        let query = r#"
            query Stats($userId: String!, $period: String) {
                listeningStats(userId: $userId, period: $period) {
                    secsToday secsWeek secsTotal playsTotal streak
                    topArtists { name value }
                    topAlbums { name value }
                    topTracks { name value }
                    topGenres { name value }
                    byDay
                    heatmap
                    byHour
                }
            }
        "#;

        let body = json!({
            "query": query,
            "variables": { "userId": self.username, "period": period }
        });

        let url = format!("{}/graphql", self.server.trim_end_matches('/'));
        let res = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.passphrase))
            .json(&body)
            .send()
            .await?;

        let payload: serde_json::Value = res.json().await?;
        let stats = payload
            .get("data")
            .and_then(|d| d.get("listeningStats"))
            .filter(|s| !s.is_null())
            .ok_or_else(|| anyhow::anyhow!("agro returned no statistics"))?;

        let mut by_hour = [0u64; 24];
        for (index, value) in numbers(stats.get("byHour")).into_iter().take(24).enumerate() {
            by_hour[index] = value;
        }

        Ok(crate::history::Stats {
            secs_today: number(stats.get("secsToday")),
            secs_week: number(stats.get("secsWeek")),
            secs_total: number(stats.get("secsTotal")),
            plays_total: number(stats.get("playsTotal")) as usize,
            streak: number(stats.get("streak")) as u32,
            top_artists: entries(stats.get("topArtists")),
            top_albums: entries(stats.get("topAlbums")),
            top_tracks: entries(stats.get("topTracks")),
            top_genres: entries(stats.get("topGenres")),
            by_day: numbers(stats.get("byDay")),
            heatmap: numbers(stats.get("heatmap")),
            by_hour,
        })
    }

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

/// A Unix timestamp as RFC3339 UTC, which is what Agro stores plays as.
///
/// Written out rather than pulled from a date crate: this is the only date formatting in the whole
/// binary, and a dependency for twenty lines of arithmetic is a poor trade. The algorithm is
/// Howard Hinnant's civil-from-days, shifting the epoch to March so leap days land at the end of
/// the year and the month arithmetic needs no table.
fn rfc3339(at: i64) -> String {
    let days = at.div_euclid(86_400);
    let secs_of_day = at.rem_euclid(86_400);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);

    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

fn number(value: Option<&serde_json::Value>) -> u64 {
    value.and_then(|v| v.as_i64()).unwrap_or(0).max(0) as u64
}

fn numbers(value: Option<&serde_json::Value>) -> Vec<u64> {
    value
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_i64().unwrap_or(0).max(0) as u64)
                .collect()
        })
        .unwrap_or_default()
}

fn entries(value: Option<&serde_json::Value>) -> Vec<(String, u32)> {
    value
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some((
                        item.get("name")?.as_str()?.to_string(),
                        item.get("value")?.as_i64().unwrap_or(0).max(0) as u32,
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Plays waiting to be reported to Agro.
///
/// An outbox rather than a direct post from the player, for two reasons. The player has no
/// configuration and no business acquiring any — it is handed a queue and a device. And a play that
/// happened while the server was unreachable is still a play: buffering it here means a laptop that
/// listened through an outage reports that listening the next time it connects, instead of losing
/// it.
static SCROBBLE_OUTBOX: std::sync::Mutex<Vec<PendingScrobble>> = std::sync::Mutex::new(Vec::new());

/// Ceiling on the outbox. Reached only when Agro has been unreachable for a very long time, and at
/// that point the oldest plays are the ones worth dropping.
const MAX_OUTBOX: usize = 1_000;

#[derive(Clone)]
struct PendingScrobble {
    title: String,
    artist: String,
    album: String,
    genres: Vec<String>,
    secs: u32,
    at: i64,
}

/// Records a completed play for the background task to report.
///
/// Called unconditionally by the player, including when Agro is not configured — the buffer is
/// bounded and simply never drained in that case, which costs a few kilobytes and keeps the call
/// site free of a configuration check it is not in a position to make.
pub fn note_play(record: &crate::history::PlayRecord) {
    let Ok(mut outbox) = SCROBBLE_OUTBOX.lock() else {
        return;
    };
    if outbox.len() >= MAX_OUTBOX {
        outbox.remove(0);
    }
    outbox.push(PendingScrobble {
        title: record.title.clone(),
        artist: record.artist.clone(),
        album: record.album.clone(),
        genres: record.genres.clone(),
        secs: record.secs,
        at: record.at,
    });
}

/// Sends whatever is waiting, putting it back if the server could not be reached.
async fn drain_scrobbles(client: &AgroClient, device_name: &str) {
    let batch: Vec<PendingScrobble> = {
        let Ok(mut outbox) = SCROBBLE_OUTBOX.lock() else {
            return;
        };
        if outbox.is_empty() {
            return;
        }
        std::mem::take(&mut *outbox)
    };

    if client.record_scrobbles(device_name, &batch).await.is_err() {
        // Back onto the front of the queue, ahead of anything logged while the request was in
        // flight, so the outbox stays in play order.
        if let Ok(mut outbox) = SCROBBLE_OUTBOX.lock() {
            let mut restored = batch;
            restored.append(&mut outbox);
            restored.truncate(MAX_OUTBOX);
            *outbox = restored;
        }
    }
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
    // The name plays are attributed to. Falls back to the device id, which is always set, so a
    // fleet with unnamed devices still gets a per-device breakdown rather than a blank row.
    let device_label = initial_name
        .clone()
        .unwrap_or_else(|| client.device_id.clone());
    tokio::spawn(async move {
        // Initial node registration
        let _ = client.register_node(initial_name.as_deref(), None).await;

        let mut last_song_id = String::new();
        let mut last_paused = true;
        let mut last_update_time = std::time::Instant::now();
        let mut check_remote_tick = 0u64;
        let mut scrobble_tick = 0u64;

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

            // Every fifth pass, so a listening session posts its plays within ten seconds without
            // making a request on every tick of a two-second loop.
            scrobble_tick += 1;
            if scrobble_tick % 5 == 0 {
                drain_scrobbles(&client, &device_label).await;
            }

            sleep(Duration::from_secs(2)).await;
        }

        // Unreachable in practice — the loop above never breaks — but written so the outbox has an
        // owner at every point rather than only inside the tick.
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::rfc3339;

    /// Hand-rolled date arithmetic, so the cases that actually break it are the ones worth pinning:
    /// the epoch, a leap day, the century rule, and a year boundary.
    #[test]
    fn formats_timestamps_as_utc_rfc3339() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1), "1970-01-01T00:00:01Z");
        // 2024-02-29, a leap day in a leap year that is also divisible by four.
        assert_eq!(rfc3339(1_709_164_800), "2024-02-29T00:00:00Z");
        // 2000-02-29: divisible by 100 but also by 400, so it *is* a leap year.
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        // 1900 was not a leap year; 1900-03-01 is the day after 1900-02-28.
        assert_eq!(rfc3339(-2_203_977_600), "1900-02-28T00:00:00Z");
        assert_eq!(rfc3339(-2_203_891_200), "1900-03-01T00:00:00Z");
        // Last second of a year, and the first of the next.
        assert_eq!(rfc3339(1_735_689_599), "2024-12-31T23:59:59Z");
        assert_eq!(rfc3339(1_735_689_600), "2025-01-01T00:00:00Z");
    }
}
