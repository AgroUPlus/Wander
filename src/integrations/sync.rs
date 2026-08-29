//! Sending this machine's local music to Agro, and finding out what it is missing.
//!
//! Three steps with very different costs, deliberately kept apart:
//!
//! 1. **Hash** — reads every byte of every file. Expensive, done once per file, and cached on the
//!    [`LocalTrack`] so an unchanged file is never hashed twice.
//! 2. **Report** — metadata only. Cheap, idempotent, and worth doing on its own: it is what lets
//!    Agro answer "that other device has a track you don't" without any audio leaving this
//!    machine.
//! 3. **Upload** — the bytes, and only for files the server says it does not already have.
//!
//! The upload protocol is Agro's REST one rather than GraphQL: these are megabytes, and a JSON
//! envelope would both inflate them and prevent streaming.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use crate::library::local::index::LocalTrack;

/// Long enough for a large file on a slow link. The handoff client's own timeout is two seconds,
/// which is right for a heartbeat and useless for a 40 MB FLAC — hence a separate client.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Read in chunks so a file is never held in memory whole.
const HASH_BUFFER: usize = 64 * 1024;

/// A peer source that holds a file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerSource {
    pub device_id: String,
    pub petname: String,
    pub lan_address: Option<String>,
    #[serde(default)]
    pub is_online: bool,
    #[serde(default)]
    pub is_server_archive: bool,
}

/// A track another device holds that this one does not.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MissingTrack {
    pub content_hash: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    #[serde(default)]
    pub duration_ms: i64,
    #[serde(default)]
    pub size_bytes: i64,
    #[serde(default)]
    pub peer_sources: Vec<PeerSource>,
}

/// How the server says music should move between this account's devices.
///
/// The server decides, from whether it archives and whether a Navidrome is on file. Both clients
/// ask the same question and get the same answer, which is the point — this used to be inferred
/// separately on each device from local config that knew nothing about the deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncMode {
    /// Everything is streamable from Navidrome, so a local copy is a convenience rather than the
    /// only way to hear it. Downloads are not offered; freeing space is.
    Navidrome,
    /// The server keeps the files but there is nothing to stream from, so a device that lacks a
    /// recording is offered the file.
    #[default]
    PeerToPeer,
    /// The server holds the index only. It is not a durable copy, so it never suggests deleting
    /// one.
    IndexOnly,
}

impl SyncMode {
    /// Whether a device without a track should be offered the bytes.
    pub fn offers_downloads(self) -> bool {
        !matches!(self, SyncMode::Navidrome)
    }

    /// Whether a redundant local copy is safe to suggest removing.
    pub fn offers_reclaim(self) -> bool {
        matches!(self, SyncMode::Navidrome)
    }
}

/// What happened to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadOutcome {
    /// The server already had these bytes; nothing was transferred.
    AlreadyPresent,
    Uploaded,
    /// Sent as far as it got. The next attempt resumes rather than restarting.
    Partial { received: u64 },
}

#[derive(Clone)]
pub struct SyncClient {
    http: reqwest::Client,
    server: String,
    passphrase: String,
    username: String,
    device_id: String,
    token: std::sync::Arc<tokio::sync::RwLock<Option<String>>>,
}

impl SyncClient {
    pub fn new(server: &str, username: &str, passphrase: &str, device_id: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(UPLOAD_TIMEOUT)
            .build()
            .context("building the sync HTTP client")?;
        Ok(Self {
            http,
            server: server.trim_end_matches('/').to_string(),
            passphrase: passphrase.to_string(),
            username: username.to_string(),
            device_id: device_id.to_string(),
            token: std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::integrations::agro::cached_token(),
            )),
        })
    }

    async fn auth_header(&self) -> String {
        let read = self.token.read().await;
        if let Some(tok) = read.as_ref() {
            return format!("Bearer {tok}");
        }
        drop(read);
        format!("Bearer {}", self.passphrase.trim())
    }

    async fn try_exchange(&self) -> bool {
        if let Ok(token) =
            crate::integrations::agro::exchange_token(&self.server, &self.username, &self.passphrase, &self.device_id)
                .await
        {
            let mut write = self.token.write().await;
            *write = Some(token);
            return true;
        }
        false
    }

    // ── Metadata ────────────────────────────────────────────────────────────────────────────

    /// Tells the server about these files without sending their audio.
    ///
    /// What backs index-only mode: the server learns who has what, and can answer "what am I
    /// missing" from metadata alone.
    pub async fn report_holdings(&self, tracks: &[&LocalTrack]) -> Result<i64> {
        let mutation = "mutation Report($userId: String!, $deviceId: String!, $tracks: [HoldingInput!]!) { \
                        reportHoldings(userId: $userId, deviceId: $deviceId, tracks: $tracks) }";
        let holdings: Vec<serde_json::Value> = tracks
            .iter()
            .filter_map(|t| {
                let hash = t.content_hash.as_ref()?;
                Some(json!({
                    "contentHash": hash,
                    "title": t.title,
                    "artist": t.artist.as_deref().unwrap_or("Unknown Artist"),
                    "album": t.album,
                    "albumArtist": t.album_artist,
                    "trackNo": t.track,
                    "discNo": t.disc,
                    "year": t.year,
                    "genre": t.genre,
                    "durationMs": t.duration as i64 * 1000,
                    "sizeBytes": t.size as i64,
                    "format": t.suffix,
                    "bitrateKbps": t.bit_rate,
                    "localRef": t.path.to_string_lossy(),
                }))
            })
            .collect();

        if holdings.is_empty() {
            return Ok(0);
        }

        let data = self
            .graphql(
                mutation,
                json!({
                    "userId": self.username,
                    "deviceId": self.device_id,
                    "tracks": holdings,
                }),
            )
            .await?;
        let count = data
            .get("reportHoldings")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        Ok(count)
    }

    /// What another of this account's devices holds that this machine lacks.
    ///
    /// The server decides, matching on the recording rather than the bytes, so a different rip of
    /// something already held here does not come back.
    pub async fn missing_here(&self, limit: i64) -> Result<Vec<MissingTrack>> {
        let query = "query Missing($userId: String!, $deviceId: String!, $limit: Int) { \
                     missingOnDevice(userId: $userId, deviceId: $deviceId, limit: $limit) { \
                     contentHash title artist album durationMs sizeBytes \
                     peerSources { deviceId petname lanAddress isOnline isServerArchive } } }";
        let data = self
            .graphql(
                query,
                json!({
                    "userId": self.username,
                    "deviceId": self.device_id,
                    "limit": limit,
                }),
            )
            .await?;
        let missing = data
            .get("missingOnDevice")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        Ok(serde_json::from_value(missing).unwrap_or_default())
    }

    /// How this deployment expects music to move between devices.
    ///
    /// Asked rather than assumed. Whether the account has a Navidrome, and whether the server
    /// keeps the bytes at all, are facts the server holds — deciding locally is how the desktop
    /// and the phone ended up behaving differently on the same account.
    pub async fn sync_mode(&self) -> Result<SyncMode> {
        let query = "query Mode($userId: String!) { syncMode(userId: $userId) }";
        let data = self
            .graphql(query, json!({ "userId": self.username }))
            .await?;
        Ok(match data.get("syncMode").and_then(|m| m.as_str()) {
            Some("NAVIDROME") => SyncMode::Navidrome,
            Some("INDEX_ONLY") => SyncMode::IndexOnly,
            // An unknown value from a newer server reads as "offer the files", the conservative
            // choice: it never suggests deleting anything.
            _ => SyncMode::PeerToPeer,
        })
    }

    /// Files this machine holds that the server has already filed, and could therefore let go of.
    ///
    /// The server checks its own disk before answering, so this is more than "the index says we
    /// uploaded it once".
    pub async fn reclaimable(&self, limit: i64) -> Result<Vec<MissingTrack>> {
        let query = "query Reclaimable($userId: String!, $deviceId: String!, $limit: Int) { \
                     reclaimable(userId: $userId, deviceId: $deviceId, limit: $limit) { \
                     contentHash title artist album durationMs sizeBytes } }";
        let data = self
            .graphql(
                query,
                json!({
                    "userId": self.username,
                    "deviceId": self.device_id,
                    "limit": limit,
                }),
            )
            .await?;
        let tracks = data
            .get("reclaimable")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        Ok(serde_json::from_value(tracks).unwrap_or_default())
    }

    async fn graphql(&self, query: &str, variables: serde_json::Value) -> Result<serde_json::Value> {
        let url = format!("{}/graphql", self.server);
        let mut auth = self.auth_header().await;
        let mut response = self
            .http
            .post(&url)
            .header("Authorization", &auth)
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .context("reaching the Agro server")?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED && self.try_exchange().await {
            auth = self.auth_header().await;
            response = self
                .http
                .post(&url)
                .header("Authorization", &auth)
                .json(&json!({ "query": query, "variables": variables }))
                .send()
                .await
                .context("reaching the Agro server")?;
        }

        if !response.status().is_success() {
            return Err(anyhow!("Agro refused the request ({})", response.status()));
        }
        let body: serde_json::Value = response.json().await.context("reading Agro's reply")?;

        // GraphQL answers a rejected request with HTTP 200 and an `errors` array, so the status
        // alone would report a refusal as a success.
        if let Some(errors) = body.get("errors").and_then(|e| e.as_array())
            && !errors.is_empty()
        {
            let message = errors
                .first()
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Err(anyhow!("{message}"));
        }
        Ok(body.get("data").cloned().unwrap_or(serde_json::Value::Null))
    }

    // ── Bytes ───────────────────────────────────────────────────────────────────────────────

    /// Sends one file, resuming if a previous attempt was cut short.
    pub async fn upload(&self, track: &LocalTrack) -> Result<UploadOutcome> {
        let hash = track
            .content_hash
            .as_ref()
            .ok_or_else(|| anyhow!("\"{}\" has not been hashed yet", track.title))?;

        let upload_url = format!("{}/api/v1/library/upload", self.server);
        let mut auth = self.auth_header().await;
        let body_json = json!({
            "deviceId": self.device_id,
            "contentHash": hash,
            "sizeBytes": track.size as i64,
            "title": track.title,
            "artist": track.artist.clone().unwrap_or_else(|| "Unknown Artist".into()),
            "album": track.album,
            "albumArtist": track.album_artist,
            "trackNo": track.track,
            "discNo": track.disc,
            "year": track.year,
            "genre": track.genre,
            "durationMs": track.duration as i64 * 1000,
            "format": track.suffix,
            "bitrateKbps": track.bit_rate,
            "localRef": track.path.to_string_lossy(),
            "extension": track.suffix,
        });

        let mut begin = self
            .http
            .post(&upload_url)
            .header("Authorization", &auth)
            .json(&body_json)
            .send()
            .await
            .context("starting the upload")?;

        if begin.status() == reqwest::StatusCode::UNAUTHORIZED && self.try_exchange().await {
            auth = self.auth_header().await;
            begin = self
                .http
                .post(&upload_url)
                .header("Authorization", &auth)
                .json(&body_json)
                .send()
                .await
                .context("starting the upload")?;
        }

        if !begin.status().is_success() {
            return Err(anyhow!("the server refused the upload ({})", begin.status()));
        }
        let begin: serde_json::Value = begin.json().await.context("reading the upload reply")?;

        match begin.get("status").and_then(|s| s.as_str()) {
            // By far the most common answer once a library has been sent once, and it costs one
            // small request instead of the whole file.
            Some("exists") => return Ok(UploadOutcome::AlreadyPresent),
            Some("upload") => {}
            _ => return Err(anyhow!("the server sent an unexpected reply")),
        }

        let upload_id = begin
            .get("uploadId")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow!("missing uploadId in server reply"))?;
        let offset = begin
            .get("received")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        self.upload_part(upload_id, &track.path, track.size, offset).await
    }

    async fn upload_part(
        &self,
        upload_id: &str,
        path: &Path,
        size: u64,
        offset: u64,
    ) -> Result<UploadOutcome> {
        use tokio::io::AsyncSeekExt;
        use tokio_util::io::ReaderStream;

        let mut file = tokio::fs::File::open(path)
            .await
            .with_context(|| format!("opening {}", path.display()))?;
        if offset > 0 {
            // Resuming: skip what the server already holds rather than re-sending it.
            file.seek(std::io::SeekFrom::Start(offset))
                .await
                .context("seeking to the resume point")?;
        }

        let auth = self.auth_header().await;
        let response = self
            .http
            .put(format!(
                "{}/api/v1/library/upload/{upload_id}",
                self.server
            ))
            .header("Authorization", &auth)
            .header("x-agro-offset", offset.to_string())
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", (size.saturating_sub(offset)).to_string())
            .body(reqwest::Body::wrap_stream(ReaderStream::new(file)))
            .send()
            .await
            .context("sending the file")?;

        if !response.status().is_success() {
            return Err(anyhow!("the transfer failed ({})", response.status()));
        }
        let body: serde_json::Value = response.json().await.context("reading the reply")?;
        match body.get("status").and_then(|s| s.as_str()) {
            Some("archived") | Some("spooled") => Ok(UploadOutcome::Uploaded),
            Some("partial") => Ok(UploadOutcome::Partial {
                received: body.get("received").and_then(|v| v.as_u64()).unwrap_or(0),
            }),
            _ => Err(anyhow!("the server sent an unexpected reply")),
        }
    }

    /// Downloads a file the server holds, into [`dir`], filed by artist and album.
    ///
    /// Streams to a `.part` alongside the destination and renames on success, so an interrupted
    /// download never leaves something that looks like a playable track — the local scanner would
    /// otherwise index a truncated file and the user would find it by playing silence.
    ///
    /// The hash is re-checked against what arrived. A corrupted transfer that kept its name would
    /// be indistinguishable from the real thing forever after.
    pub async fn fetch(&self, track: &MissingTrack, dir: &Path) -> Result<std::path::PathBuf> {
        let mut p2p_response: Option<reqwest::Response> = None;

        // 1. Try direct LAN P2P transfer if peer has a reachable LAN address
        for source in &track.peer_sources {
            if let Some(lan) = &source.lan_address {
                let p2p_url = format!("http://{lan}/p2p/fetch/{}", track.content_hash);
                let auth = self.auth_header().await;
                if let Ok(res) = self
                    .http
                    .get(&p2p_url)
                    .header("Authorization", &auth)
                    .timeout(Duration::from_secs(4))
                    .send()
                    .await
                {
                    if res.status().is_success() {
                        p2p_response = Some(res);
                        break;
                    }
                }
            }
        }

        // 2. If LAN P2P is not reachable, try ephemeral relay pipe on Agro server
        if p2p_response.is_none() {
            if let Some(peer) = track.peer_sources.iter().find(|s| !s.is_server_archive) {
                let open_url = format!("{}/api/v1/relay/open", self.server);
                let auth = self.auth_header().await;
                let open_body = json!({
                    "contentHash": track.content_hash,
                    "fromDevice": peer.device_id,
                    "toDevice": self.device_id,
                });
                if let Ok(open_res) = self
                    .http
                    .post(&open_url)
                    .header("Authorization", &auth)
                    .json(&open_body)
                    .timeout(Duration::from_secs(5))
                    .send()
                    .await
                {
                    if open_res.status().is_success() {
                        if let Ok(val) = open_res.json::<serde_json::Value>().await {
                            if let Some(session_id) = val.get("sessionId").and_then(|s| s.as_str()) {
                                let recv_url = format!("{}/api/v1/relay/{session_id}/receive", self.server);
                                if let Ok(res) = self
                                    .http
                                    .get(&recv_url)
                                    .header("Authorization", &auth)
                                    .send()
                                    .await
                                {
                                    if res.status().is_success() {
                                        p2p_response = Some(res);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // 3. Fall back to permanent server archive
        let response = match p2p_response {
            Some(res) => res,
            None => {
                let fetch_url = format!("{}/api/v1/library/fetch/{}", self.server, track.content_hash);
                let mut auth = self.auth_header().await;
                let mut res = self
                    .http
                    .get(&fetch_url)
                    .header("Authorization", &auth)
                    .send()
                    .await
                    .context("asking the server for the file")?;

                if res.status() == reqwest::StatusCode::UNAUTHORIZED && self.try_exchange().await {
                    auth = self.auth_header().await;
                    res = self
                        .http
                        .get(&fetch_url)
                        .header("Authorization", &auth)
                        .send()
                        .await
                        .context("asking the server for the file")?;
                }

                if !res.status().is_success() {
                    return Err(anyhow!(
                        "the server would not hand that file over ({})",
                        res.status()
                    ));
                }
                res
            }
        };

        let artist = crate::plugins::sanitize_filename(&track.artist);
        let album = crate::plugins::sanitize_filename(track.album.as_deref().unwrap_or("Unknown Album"));
        let title = crate::plugins::sanitize_filename(&track.title);
        let folder = dir.join(&artist).join(&album);
        tokio::fs::create_dir_all(&folder)
            .await
            .with_context(|| format!("creating {}", folder.display()))?;

        // The extension is unknown until the bytes are here, and guessing from the title would be
        // worse than a generic one the scanner can still probe.
        let extension = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .and_then(|mime| match mime {
                "audio/flac" => Some("flac"),
                "audio/mpeg" => Some("mp3"),
                "audio/ogg" | "audio/opus" => Some("opus"),
                _ => None,
            })
            .unwrap_or("flac");

        let destination = folder.join(format!("{title}.{extension}"));
        let partial = folder.join(format!("{title}.{extension}.part"));

        let mut file = tokio::fs::File::create(&partial)
            .await
            .with_context(|| format!("creating {}", partial.display()))?;
        let mut stream = response.bytes_stream();
        use futures_util::StreamExt;
        use tokio::io::AsyncWriteExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("receiving the file")?;
            file.write_all(&chunk).await.context("writing the file")?;
        }
        file.flush().await.context("flushing the file")?;
        drop(file);

        let actual = {
            let partial = partial.clone();
            tokio::task::spawn_blocking(move || hash_file(&partial)).await??
        };
        if actual != track.content_hash {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(anyhow!("the downloaded file did not match its hash"));
        }

        tokio::fs::rename(&partial, &destination)
            .await
            .context("putting the file in place")?;
        Ok(destination)
    }
}

/// SHA-256 of a file's contents.
///
/// Blocking, and the caller is expected to run it on the blocking pool — this reads the whole file
/// and would stall the async runtime otherwise.
pub fn hash_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("opening {} to hash it", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; HASH_BUFFER];
    loop {
        let read = file.read(&mut buffer).context("reading the file to hash it")?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    // Lowercase hex: the server validates the format and compares as a string.
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn hashes_match_the_known_sha256_of_the_content() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"hello").unwrap();
        file.flush().unwrap();
        assert_eq!(
            hash_file(file.path()).unwrap(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn identical_content_at_different_paths_hashes_the_same() {
        let mut a = tempfile::NamedTempFile::new().unwrap();
        let mut b = tempfile::NamedTempFile::new().unwrap();
        a.write_all(b"same bytes").unwrap();
        b.write_all(b"same bytes").unwrap();
        a.flush().unwrap();
        b.flush().unwrap();
        // The whole point: two machines file the same recording in different places.
        assert_eq!(hash_file(a.path()).unwrap(), hash_file(b.path()).unwrap());
    }
}

/// Exercises the real protocol against a running Agro.
///
/// Ignored by default — it needs a server. Run with the address and token in the environment:
/// 
#[cfg(test)]
mod live {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn live_upload_round_trip() {
        let (Ok(url), Ok(token)) = (
            std::env::var("AGRO_TEST_URL"),
            std::env::var("AGRO_TEST_TOKEN"),
        ) else {
            eprintln!("set AGRO_TEST_URL and AGRO_TEST_TOKEN");
            return;
        };
        let path = std::path::PathBuf::from(std::env::var("AGRO_TEST_FILE").unwrap());
        let size = std::fs::metadata(&path).unwrap().len();
        let hash = hash_file(&path).unwrap();

        let track = LocalTrack {
            path,
            mtime: 0,
            size,
            title: "Breed".into(),
            artist: Some("Nirvana".into()),
            album_artist: Some("Nirvana".into()),
            album: Some("Nevermind".into()),
            track: Some(4),
            disc: None,
            year: Some(1991),
            genre: None,
            duration: 183,
            bit_rate: 900,
            suffix: Some("flac".into()),
            content_hash: Some(hash),
        };

        let client = SyncClient::new(&url, "alpha", &token, "wander-testbox").unwrap();

        let reported = client.report_holdings(&[&track]).await.unwrap();
        assert_eq!(reported, 1, "the server accepted the holding");

        let outcome = client.upload(&track).await.unwrap();
        assert!(
            matches!(outcome, UploadOutcome::Uploaded | UploadOutcome::AlreadyPresent),
            "unexpected outcome: {outcome:?}"
        );

        // Second time round the bytes must not move again.
        assert_eq!(client.upload(&track).await.unwrap(), UploadOutcome::AlreadyPresent);

        let missing = client.missing_here(50).await.unwrap();
        assert!(
            !missing.iter().any(|t| t.title == "Breed"),
            "a track this device just reported must not come back as missing"
        );
    }
}
