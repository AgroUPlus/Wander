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

/// The shared namespace for tracks.
pub fn namespaced_id(song_id: &str) -> String {
    if song_id.starts_with("navidrome:")
        || song_id.starts_with("local:")
        || song_id.starts_with("online:")
        || song_id.starts_with("ytm:")
        || song_id.starts_with("ytmusic:")
        || song_id.starts_with("archive:")
        || song_id.starts_with("http://")
        || song_id.starts_with("https://")
    {
        song_id.to_string()
    } else {
        format!("navidrome:{}", song_id)
    }
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

/// Tells a session that is still happening from the last one that did.
///
/// The server keeps whatever a device reported last and goes on keeping it after that device is
/// closed, killed or carried out of range — none of which sends a final "stopped". So `is_playing`
/// alone would leave a phone that died mid-track looking like it is still playing it tomorrow.
///
/// Staleness is judged by the position failing to advance rather than by the server's timestamp,
/// deliberately: a playing track moves its own clock forward, which is true regardless of whether
/// two devices agree about what time it is. Comparing a remote timestamp against this machine's
/// clock would make the feature depend on both being right.
#[derive(Default)]
struct RemoteFreshness {
    seen: Option<(String, i64)>,
    unchanged_since: Option<std::time::Instant>,
}

impl RemoteFreshness {
    /// Records what the server just said and answers whether it still counts as live.
    fn accept(&mut self, handoff: &RemoteHandoff) -> bool {
        let now = (handoff.track_uri.clone(), handoff.position_ms);
        if self.seen.as_ref() != Some(&now) {
            self.seen = Some(now);
            self.unchanged_since = None;
            return true;
        }
        // The window is generous next to the sender's ten-second heartbeat, because the cost of
        // being wrong is asymmetric: a few seconds of staleness is invisible, while dropping a
        // live session over one slow heartbeat makes the display flicker.
        let since = *self.unchanged_since.get_or_insert_with(std::time::Instant::now);
        since.elapsed() < Duration::from_secs(HANDOFF_STALE_SECS)
    }

    fn forget(&mut self) {
        self.seen = None;
        self.unchanged_since = None;
    }
}

/// How long a position may sit still before the sender is presumed gone.
const HANDOFF_STALE_SECS: u64 = 45;

pub static ACTIVE_REMOTE_HANDOFF: std::sync::OnceLock<Arc<RwLock<Option<RemoteHandoff>>>> =
    std::sync::OnceLock::new();

pub fn get_remote_handoff() -> Arc<RwLock<Option<RemoteHandoff>>> {
    ACTIVE_REMOTE_HANDOFF
        .get_or_init(|| Arc::new(RwLock::new(None)))
        .clone()
}

pub struct AgroClient {
    client: Client,
    pub server: String,
    pub username: String,
    passphrase: String,
    pub device_id: String,
    token: Arc<tokio::sync::RwLock<Option<String>>>,
}

/// Exchanges an account passphrase for a device token via `/api/v1/login`.
pub async fn exchange_token(server: &str, username: &str, passphrase: &str, device_id: &str) -> Result<String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
        .unwrap_or_else(|_| Client::new());

    let login_url = format!("{}/api/v1/login", server.trim_end_matches('/'));
    let body = json!({
        "username": username.trim(),
        "passphrase": passphrase.trim(),
        "label": device_id.trim()
    });

    let res = client.post(&login_url).json(&body).send().await?;
    let status = res.status();
    if status.is_success() {
        let json_data: serde_json::Value = res.json().await?;
        if let Some(token) = json_data.get("token").and_then(|t| t.as_str()) {
            // Every caller's token is remembered here rather than at each call site. There are
            // three of them — the GraphQL client, the sync client and the reconnecting WebSocket —
            // and one that forgot would quietly go back to minting a credential per attempt.
            remember_token(token);
            return Ok(token.to_string());
        }
    }
    anyhow::bail!("Login exchange failed with status {}", status)
}

/// The device token, shared by every [`AgroClient`] in the process and kept across runs.
///
/// A client is built per operation, so a token cached on the client itself never survives long
/// enough to be reused — each one bought its own from `/api/v1/login` and left another row in the
/// server's app-password list. The process-wide cache fixes the repetition within a run; writing
/// it to the config file fixes it across runs.
static DEVICE_TOKEN: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

/// The token to try first: whatever this process last obtained, else what the config file kept.
pub(crate) fn cached_token() -> Option<String> {
    if let Ok(guard) = DEVICE_TOKEN.read() {
        if let Some(token) = guard.as_ref() {
            return Some(token.clone());
        }
    }
    let stored = crate::config::Config::load()
        .ok()
        .map(|config| config.agro.device_token.trim().to_string())
        .filter(|token| !token.is_empty())?;
    if let Ok(mut guard) = DEVICE_TOKEN.write() {
        *guard = Some(stored.clone());
    }
    Some(stored)
}

/// Remembers a freshly minted token, in this process and on disk.
///
/// The config is re-read before it is written so this cannot clobber an edit made while the
/// program was running — only the one field is carried over. A failure to save is not worth
/// reporting: the token still works for this run, and the next run buys another.
fn remember_token(token: &str) {
    if let Ok(mut guard) = DEVICE_TOKEN.write() {
        *guard = Some(token.to_string());
    }
    if let Ok(mut config) = crate::config::Config::load() {
        if config.agro.device_token != token {
            config.agro.device_token = token.to_string();
            let _ = config.save();
        }
    }
}

impl AgroClient {
    pub fn new(server: String, username: String, passphrase: String, device_id: String) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            server,
            username,
            passphrase,
            device_id,
            token: Arc::new(tokio::sync::RwLock::new(cached_token())),
        }
    }

    /// The credential to present, best first.
    ///
    /// The passphrase is the last resort rather than the default: it stopped being a bearer token
    /// when device tokens arrived, so leading with it means a guaranteed 401 before every first
    /// request. It is still tried, because a config written by the dashboard's pairing snippet
    /// puts a device token in that same field.
    async fn auth_header(&self) -> String {
        let read = self.token.read().await;
        if let Some(tok) = read.as_ref() {
            return format!("Bearer {tok}");
        }
        drop(read);
        format!("Bearer {}", self.passphrase.trim())
    }

    async fn try_exchange(&self) -> bool {
        // Nothing to exchange with. Whatever is in `passphrase` is already being sent as a bearer
        // token, and asking `/api/v1/login` to accept a device token as a passphrase just burns a
        // request against the rate limiter.
        if self.passphrase.trim().is_empty() {
            return false;
        }
        let label = self
            .device_name()
            .unwrap_or_else(|| self.device_id.trim().to_string());
        if let Ok(new_tok) = exchange_token(&self.server, &self.username, &self.passphrase, &label).await {
            remember_token(&new_tok);
            *self.token.write().await = Some(new_tok);
            true
        } else {
            false
        }
    }

    /// Asks the server who this device is signed in as.
    ///
    /// The only way to find out whether the configured credential actually works. Everything else
    /// fails quietly — a refused handoff looks exactly like a device that is not playing — so the
    /// settings screen reported "Synced" purely on the basis that *some* credential was written in
    /// the config, which is the one thing that was never in doubt.
    pub async fn verify(&self) -> Result<String> {
        let answer = self
            .graphql(&json!({ "query": "{ me { username } }" }))
            .await?;
        answer["data"]["me"]["username"]
            .as_str()
            .map(|name| name.to_string())
            .ok_or_else(|| {
                let refusal = answer["errors"][0]["message"]
                    .as_str()
                    .unwrap_or("the server did not say who this device is");
                anyhow::anyhow!("{refusal}")
            })
    }

    /// What this device calls itself in the server's app-password list.
    ///
    /// A stable name, so a token that is replaced is recognisable as a replacement rather than as
    /// another unrelated device.
    fn device_name(&self) -> Option<String> {
        crate::config::Config::load()
            .ok()
            .and_then(|config| config.agro.device_name)
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
    }

    pub async fn graphql(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
        let url = format!("{}/graphql", self.server.trim_end_matches('/'));
        let mut auth = self.auth_header().await;

        let mut res = self
            .client
            .post(&url)
            .header("Authorization", &auth)
            .json(body)
            .send()
            .await?;

        if res.status() == reqwest::StatusCode::UNAUTHORIZED && self.try_exchange().await {
            auth = self.auth_header().await;
            res = self
                .client
                .post(&url)
                .header("Authorization", &auth)
                .json(body)
                .send()
                .await?;
        }

        if !res.status().is_success() {
            anyhow::bail!("Agro server returned status {}", res.status());
        }

        let json_data: serde_json::Value = res.json().await?;
        if let Some(errors) = json_data.get("errors").and_then(|e| e.as_array())
            && !errors.is_empty()
        {
            let message = errors
                .first()
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unspecified error");
            anyhow::bail!("Agro error: {message}");
        }

        Ok(json_data)
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

        if let Ok(json_data) = self.graphql(&body).await {
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

        let _ = self.graphql(&body).await;
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
        let json_data = self.graphql(&body).await?;
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

        let Ok(json_data) = self.graphql(&body).await else {
            return Ok(None);
        };
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

        self.graphql(&body).await?;
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

        let payload = self.graphql(&body).await?;
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

        let Ok(json_data) = self.graphql(&body).await else {
            return Ok(None);
        };
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

/// Sends the play log this machine already had, once per account.
///
/// [`note_play`] only ever sees plays that happen while it is running, so switching Agro on left
/// the fleet's statistics starting from zero for this device — months of local history sitting in
/// `history.jsonl` and counting for nothing. This offers the lot.
///
/// Marked done by a file naming the account, so it runs again if the machine is later pointed at a
/// different one. Re-running is harmless in any case: the server is idempotent on
/// (account, artist, title, time), so a play it already holds is ignored rather than doubled.
///
/// Sent in chunks because a long history is well past what one request should carry, and directly
/// rather than through the outbox, whose bound is sized for live listening rather than a year of
/// backlog.
async fn backfill_history(client: &AgroClient, device_name: &str) {
    let Some(marker) = crate::paths::cache_dir().map(|dir| dir.join(BACKFILL_MARKER)) else {
        return;
    };
    if std::fs::read_to_string(&marker).is_ok_and(|done| done.trim() == client.username) {
        return;
    }

    let records = crate::history::load();
    if records.is_empty() {
        // Still marked: an empty log is a finished backfill, and leaving the marker off would mean
        // re-reading the file on every launch for ever.
        let _ = std::fs::write(&marker, &client.username);
        return;
    }

    for chunk in records.chunks(BACKFILL_CHUNK) {
        let batch: Vec<PendingScrobble> = chunk
            .iter()
            .map(|record| PendingScrobble {
                title: record.title.clone(),
                artist: record.artist.clone(),
                album: record.album.clone(),
                genres: record.genres.clone(),
                secs: record.secs,
                at: record.at,
            })
            .collect();
        // Give up on the first failure and leave the marker unwritten, so the whole backfill is
        // retried next launch rather than half of it being silently skipped.
        if client.record_scrobbles(device_name, &batch).await.is_err() {
            return;
        }
    }

    let _ = std::fs::write(&marker, &client.username);
}

/// Names the account whose history has already been uploaded from this machine.
const BACKFILL_MARKER: &str = "agro-history-backfilled";

/// Plays per backfill request. Comfortably under the server's own per-request cap.
const BACKFILL_CHUNK: usize = 400;

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
    // `has_credential`, not `passphrase`: a device paired from a QR keeps a device token and no
    // passphrase, and testing the passphrase alone made exactly that case look unconfigured — the
    // whole reporting task returned here and the device never appeared on the server at all.
    if !config.enabled || !config.has_credential() {
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

        // Before anything else this device reports, so the fleet's totals include the listening
        // that happened before Agro was switched on rather than starting from zero.
        backfill_history(&client, &device_label).await;

        let mut last_song_id = String::new();
        let mut last_paused = true;
        let mut last_update_time = std::time::Instant::now();
        let mut check_remote_tick = 0u64;
        let mut remote_freshness = RemoteFreshness::default();
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
                        let live = remote.device_id != client.device_id
                            && remote.is_playing
                            && remote_freshness.accept(&remote);
                        if !live {
                            remote_freshness.forget();
                        }
                        let mut store = remote_handoff_store.write().await;
                        *store = if live { Some(remote) } else { None };
                    } else {
                        remote_freshness.forget();
                        let mut store = remote_handoff_store.write().await;
                        *store = None;
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

#[cfg(test)]
mod token_reuse_tests {
    use super::*;

    /// Proves a device buys one credential, not one per operation.
    ///
    /// An `AgroClient` is constructed per operation, so a token cached on the client itself was
    /// thrown away immediately and the next call bought another from `/api/v1/login`. On the
    /// server that showed up as an app-password list filling with identically-named rows nobody
    /// could tell apart. This asserts the count on the server, because that is where the symptom
    /// was visible.
    ///
    /// Ignored by default — it needs a real server and an account to spend:
    ///
    /// ```text
    /// AGRO_TEST_URL=http://localhost:8797 AGRO_TEST_USER=beta AGRO_TEST_PASS=… \
    ///   XDG_CONFIG_HOME=$(mktemp -d) \
    ///   cargo test repeated_clients_share_one_device_token -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore]
    async fn repeated_clients_share_one_device_token() {
        let (Ok(url), Ok(user), Ok(pass)) = (
            std::env::var("AGRO_TEST_URL"),
            std::env::var("AGRO_TEST_USER"),
            std::env::var("AGRO_TEST_PASS"),
        ) else {
            eprintln!("set AGRO_TEST_URL, AGRO_TEST_USER and AGRO_TEST_PASS");
            return;
        };

        // Five separate clients, as five separate operations would build them.
        for _ in 0..5 {
            let client = AgroClient::new(url.clone(), user.clone(), pass.clone(), "wander-testbox".into());
            let _ = client.fetch_stats("ALL").await;
        }

        let listed = AgroClient::new(url, user.clone(), pass, "wander-testbox".into())
            .graphql(&json!({
                "query": "query D($u: String!) { appPasswords(userId: $u) { id label } }",
                "variables": { "u": user }
            }))
            .await
            .expect("listing app passwords");

        let tokens = listed["data"]["appPasswords"]
            .as_array()
            .expect("appPasswords array")
            .len();
        eprintln!("--- {tokens} credentials on the server ---");
        assert!(
            tokens <= 2,
            "six clients left {tokens} credentials behind; the token is not being reused"
        );
    }
}
