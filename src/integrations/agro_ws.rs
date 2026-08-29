//! Agro's live channel, so new music arrives instead of being discovered.
//!
//! Without this, sync was poll-only: a pass ran at startup and whenever someone opened Settings and
//! pressed a key. Upload something from the phone and this machine learned about it the next time
//! it was restarted, which is not what "background sync" should mean.
//!
//! The socket carries no audio and issues no commands — every message is a hint that something
//! changed, and the app answers by asking the server what it should now know. That keeps the
//! authority on the server and means a dropped or duplicated message costs a redundant query
//! rather than a wrong state.
//!
//! Runs forever with a reconnect: a laptop suspends, a server restarts, and a client that gives up
//! on the first failure is a client that is disconnected for the rest of the session.

use futures_util::StreamExt;
use serde::Deserialize;
use std::time::Duration;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

/// First wait after a failure. Doubles up to [`MAX_BACKOFF`].
const BASE_BACKOFF: Duration = Duration::from_secs(2);

/// Ceiling on the reconnect wait. A server that has been down for an hour should still be picked
/// up within a minute of coming back.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// What the server told us, reduced to the parts this client acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveMessage {
    /// Somebody suggested, approved or removed something in the jam.
    JamUpdated,
    /// The room moved to a new track. Decided by the server, so this is an instruction.
    JamNowPlaying,
    /// This device is missing things another device has.
    SyncOffer {
        count: usize,
        albums: Vec<String>,
    },
    /// The library changed. Worth re-checking what we are missing.
    LibraryUpdated,
    /// A friend handed this account a song.
    ///
    /// Carries the description rather than only an id, so the notification can name the track
    /// without a round trip. The inbox is still re-read afterwards: this frame is news about one
    /// drop, not a statement about the whole inbox.
    TrackDrop {
        from: String,
        title: String,
        artist: String,
    },
    /// A peer device requested a relay stream for a track this device holds.
    RelayRequest {
        session_id: String,
        content_hash: String,
        to_device: String,
    },
}

/// Agro's envelope. `msg_type` is the discriminator; everything else rides in `payload`.
#[derive(Deserialize)]
struct Envelope {
    msg_type: String,
    #[serde(default)]
    payload: serde_json::Value,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct OfferPayload {
    #[serde(default)]
    count: usize,
    #[serde(default)]
    albums: Vec<String>,
}

/// Opens the socket and keeps it open. The receiver yields for as long as the app runs.
///
/// `token` goes in the query string rather than a header because a WebSocket handshake cannot
/// carry custom headers in every client, and Agro accepts `?token=` for exactly that reason.
pub fn spawn(server: &str, token_or_pass: &str, device_id: &str, username: Option<&str>) -> UnboundedReceiver<LiveMessage> {
    let (tx, rx) = mpsc::unbounded_channel();
    let server = server.to_string();
    let token_or_pass = token_or_pass.to_string();
    let device_id = device_id.to_string();
    let username = username.map(|s| s.to_string());

    tokio::spawn(async move {
        // A reconnect loop that exchanges on every failure mints a credential per failed attempt,
        // and a server that is simply down fails a great many times. Start from whatever token the
        // process already has.
        let mut active_token =
            crate::integrations::agro::cached_token().unwrap_or_else(|| token_or_pass.clone());
        let mut backoff = BASE_BACKOFF;
        loop {
            let url = socket_url(&server, &active_token, &device_id);
            let ok = if let Some(ref u) = url {
                listen(u, &tx).await.is_ok()
            } else {
                false
            };

            if ok {
                // A clean end still means reconnecting, but without punishing the next attempt.
                backoff = BASE_BACKOFF;
            } else if let Some(ref u) = username {
                // If connecting failed, attempt to exchange passphrase for a device token
                if let Ok(new_tok) = crate::integrations::agro::exchange_token(&server, u, &token_or_pass, &device_id).await {
                    active_token = new_tok;
                }
            }

            if tx.is_closed() {
                return;
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    });

    rx
}

/// One connection's lifetime. Returns when the socket closes, for any reason.
async fn listen(
    url: &str,
    tx: &mpsc::UnboundedSender<LiveMessage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let request = url.into_client_request()?;
    let (stream, _) = tokio_tungstenite::connect_async(request).await?;
    let (_write, mut read) = stream.split();

    while let Some(frame) = read.next().await {
        let text = match frame? {
            Message::Text(text) => text.to_string(),
            Message::Close(_) => return Ok(()),
            // Ping/Pong are handled by the library; anything else is not ours to interpret.
            _ => continue,
        };
        if let Some(message) = parse(&text) {
            // A closed receiver means the app is gone.
            if tx.send(message).is_err() {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Turns one frame into something worth acting on, or nothing.
fn parse(text: &str) -> Option<LiveMessage> {
    let envelope: Envelope = serde_json::from_str(text).ok()?;
    match envelope.msg_type.as_str() {
        "SYNC_OFFER" => {
            let offer: OfferPayload = serde_json::from_value(envelope.payload).unwrap_or_default();
            Some(LiveMessage::SyncOffer {
                count: offer.count,
                albums: offer.albums,
            })
        }
        "LIBRARY_UPDATED" => Some(LiveMessage::LibraryUpdated),
        "TRACK_DROP" => Some(LiveMessage::TrackDrop {
            from: envelope.payload["from"].as_str().unwrap_or("someone").to_string(),
            title: envelope.payload["trackTitle"].as_str().unwrap_or_default().to_string(),
            artist: envelope.payload["artistName"].as_str().unwrap_or_default().to_string(),
        }),
        "JAM_UPDATED" => Some(LiveMessage::JamUpdated),
        "JAM_NOW_PLAYING" => Some(LiveMessage::JamNowPlaying),
        "RELAY_REQUEST" => Some(LiveMessage::RelayRequest {
            session_id: envelope.payload["sessionId"].as_str().unwrap_or_default().to_string(),
            content_hash: envelope.payload["contentHash"].as_str().unwrap_or_default().to_string(),
            to_device: envelope.payload["toDevice"].as_str().unwrap_or_default().to_string(),
        }),
        // HANDOFF, NODE_UPDATE and SETTINGS_SYNC are someone else's business today. Ignoring them
        // by name rather than by accident means adding one later is a single arm.
        _ => None,
    }
}

/// `https://host` → `wss://host/ws/sync?token=…&device=…`.
fn socket_url(server: &str, token: &str, device_id: &str) -> Option<String> {
    let trimmed = server.trim().trim_end_matches('/');
    let base = if let Some(rest) = trimmed.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        return None;
    };
    if base.len() <= "wss://".len() {
        return None;
    }
    Some(format!(
        "{base}/ws/sync?token={}&device={}",
        urlencoding::encode(token),
        urlencoding::encode(device_id)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_socket_url_from_either_scheme() {
        assert_eq!(
            socket_url("https://agro.example.com/", "tok en", "wander-desktop").unwrap(),
            "wss://agro.example.com/ws/sync?token=tok%20en&device=wander-desktop"
        );
        assert!(socket_url("http://127.0.0.1:1674", "t", "d")
            .unwrap()
            .starts_with("ws://127.0.0.1:1674/ws/sync"));
    }

    #[test]
    fn refuses_an_address_that_is_not_http() {
        assert!(socket_url("agro.example.com", "t", "d").is_none());
        assert!(socket_url("ftp://agro.example.com", "t", "d").is_none());
        assert!(socket_url("https://", "t", "d").is_none());
        assert!(socket_url("", "t", "d").is_none());
    }

    #[test]
    fn reads_an_offer() {
        let text = r#"{"msg_type":"SYNC_OFFER","payload":{"count":3,"albums":["Limerence"],
                       "sample":["Marzuku — Agony"]},"user_id":"alpha"}"#;
        assert_eq!(
            parse(text),
            Some(LiveMessage::SyncOffer {
                count: 3,
                albums: vec!["Limerence".to_string()],
            })
        );
    }

    /// Connects to a real Agro and waits for it to say something.
    ///
    /// Ignored by default — it needs a server and something to happen on it. Run it, then upload a
    /// track from another device:
    ///
    /// ```text
    /// AGRO_TEST_URL=https://agro.example.com AGRO_TEST_TOKEN=… \
    ///   cargo test live_socket_receives_an_offer -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore]
    async fn live_socket_receives_an_offer() {
        let (Ok(url), Ok(token)) = (
            std::env::var("AGRO_TEST_URL"),
            std::env::var("AGRO_TEST_TOKEN"),
        ) else {
            eprintln!("set AGRO_TEST_URL and AGRO_TEST_TOKEN");
            return;
        };
        let device = std::env::var("AGRO_TEST_DEVICE").unwrap_or_else(|_| "wander-testbox".into());

        let mut rx = spawn(&url, &token, &device, None);
        eprintln!("listening as {device}; upload something from another device…");

        // Collects for a while rather than returning on the first frame: an album should produce
        // several LIBRARY_UPDATEDs but exactly one SYNC_OFFER, and only a window shows that.
        let mut offers = 0;
        let mut updates = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        while let Ok(Some(message)) = tokio::time::timeout_at(deadline, rx.recv()).await {
            eprintln!("received: {message:?}");
            match message {
                LiveMessage::SyncOffer { .. } => offers += 1,
                LiveMessage::LibraryUpdated => updates += 1,
                LiveMessage::JamUpdated | LiveMessage::JamNowPlaying => {}
                LiveMessage::TrackDrop { .. } => {}
                LiveMessage::RelayRequest { .. } => {}
            }
        }
        eprintln!("--- {updates} library updates, {offers} sync offers ---");
        assert!(updates + offers > 0, "nothing arrived in 45s");
    }

    #[test]
    fn survives_a_payload_it_does_not_recognise() {
        // Older servers send an offer with no album list; that must still be an offer.
        assert_eq!(
            parse(r#"{"msg_type":"SYNC_OFFER","payload":{"count":1}}"#),
            Some(LiveMessage::SyncOffer {
                count: 1,
                albums: vec![]
            })
        );
        assert_eq!(
            parse(r#"{"msg_type":"LIBRARY_UPDATED","payload":{}}"#),
            Some(LiveMessage::LibraryUpdated)
        );
        // A drop names who sent what. A frame missing the sender is still a drop — the inbox is
        // re-read either way, and refusing to parse it would lose the only notice we get.
        assert_eq!(
            parse(
                r#"{"msg_type":"TRACK_DROP","payload":{"from":"beta","trackTitle":"Xtal","artistName":"Aphex Twin"}}"#
            ),
            Some(LiveMessage::TrackDrop {
                from: "beta".into(),
                title: "Xtal".into(),
                artist: "Aphex Twin".into(),
            })
        );
        assert_eq!(
            parse(r#"{"msg_type":"TRACK_DROP","payload":{}}"#),
            Some(LiveMessage::TrackDrop {
                from: "someone".into(),
                title: String::new(),
                artist: String::new(),
            })
        );
        assert_eq!(parse(r#"{"msg_type":"HANDOFF","payload":{}}"#), None);
        assert_eq!(parse("not json"), None);
    }
}
