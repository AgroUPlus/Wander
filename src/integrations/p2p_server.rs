//! Embedded local P2P HTTP server for Wander.
//!
//! Listens on a local port (default 8701) to serve audio files directly over LAN to peer devices
//! (such as Wanda on Android). Also handles streaming files to Agro's Ephemeral Relay when remote.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub static GLOBAL_INDEX: std::sync::LazyLock<P2PTrackIndex> =
    std::sync::LazyLock::new(P2PTrackIndex::new);

#[derive(Clone, Default)]
pub struct P2PTrackIndex {
    by_hash: Arc<RwLock<HashMap<String, PathBuf>>>,
}

impl P2PTrackIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&self, tracks: &[crate::library::local::index::LocalTrack]) {
        let mut guard = self.by_hash.write().unwrap();
        for track in tracks {
            if let Some(hash) = &track.content_hash {
                guard.insert(hash.clone(), track.path.clone());
            }
        }
    }

    pub fn get_path(&self, hash: &str) -> Option<PathBuf> {
        let guard = self.by_hash.read().unwrap();
        guard.get(hash).cloned()
    }
}

pub struct P2PServer {
    port: u16,
    index: P2PTrackIndex,
}

impl P2PServer {
    pub fn spawn(port: u16, index: P2PTrackIndex) -> Self {
        let server = Self {
            port,
            index: index.clone(),
        };
        tokio::spawn(async move {
            let addr = format!("0.0.0.0:{port}");
            let Ok(listener) = TcpListener::bind(&addr).await else {
                return;
            };
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    continue;
                };
                let index = index.clone();
                tokio::spawn(handle_connection(socket, index));
            }
        });
        server
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

/// Reads one request off the socket and answers it. Any malformed or unknown request gets a
/// response rather than a dropped connection — a peer that is told 400 stops waiting.
async fn handle_connection(mut socket: tokio::net::TcpStream, index: P2PTrackIndex) {
    let mut buffer = [0u8; 4096];
    let Ok(n) = socket.read(&mut buffer).await else {
        return;
    };
    let request = String::from_utf8_lossy(&buffer[..n]);
    let Some((method, path)) = parse_request_line(&request) else {
        return;
    };

    if method == "GET" && path == "/p2p/ping" {
        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\nConnection: close\r\n\r\npong";
        let _ = socket.write_all(response.as_bytes()).await;
        return;
    }

    if method == "GET" && path.starts_with("/p2p/fetch/") {
        let hash = path
            .trim_start_matches("/p2p/fetch/")
            .split('?')
            .next()
            .unwrap_or("");
        if let Some(file_path) = index.get_path(hash) {
            if serve_file(&mut socket, &file_path).await {
                return;
            }
        }
        let not_found =
            "HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nNot Found";
        let _ = socket.write_all(not_found.as_bytes()).await;
        return;
    }

    let bad_req =
        "HTTP/1.1 400 Bad Request\r\nContent-Length: 11\r\nConnection: close\r\n\r\nBad Request";
    let _ = socket.write_all(bad_req.as_bytes()).await;
}

/// Splits the method and path out of the request line, or [`None`] if there isn't one.
fn parse_request_line(request: &str) -> Option<(&str, &str)> {
    let first_line = request.lines().next()?;
    let mut parts = first_line.split_whitespace();
    Some((parts.next()?, parts.next()?))
}

/// Streams a file back with its length. Returns whether the response was started — a caller that
/// gets `false` still owes the peer a 404, since nothing has been written yet.
async fn serve_file(socket: &mut tokio::net::TcpStream, file_path: &std::path::Path) -> bool {
    let Ok(mut file) = tokio::fs::File::open(file_path).await else {
        return false;
    };
    let Ok(meta) = file.metadata().await else {
        return false;
    };
    let len = meta.len();
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
    );
    if socket.write_all(header.as_bytes()).await.is_err() {
        return true;
    }
    let mut file_buf = [0u8; 64 * 1024];
    loop {
        match file.read(&mut file_buf).await {
            Ok(0) => break,
            Ok(bytes_read) => {
                if socket.write_all(&file_buf[..bytes_read]).await.is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    true
}

/// Streams an audio file chunk-by-chunk to Agro's Ephemeral Relay endpoint.
pub async fn send_relay_stream(
    server: &str,
    api_key: &str,
    session_id: &str,
    file_path: std::path::PathBuf,
) -> anyhow::Result<()> {
    let file = tokio::fs::File::open(file_path).await?;
    let stream = tokio_util::io::ReaderStream::new(file);
    let body = reqwest::Body::wrap_stream(stream);
    let client = reqwest::Client::new();
    let url = format!(
        "{}/api/v1/relay/{session_id}/send",
        server.trim_end_matches('/')
    );
    let res = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/octet-stream")
        .body(body)
        .send()
        .await?;
    if !res.status().is_success() {
        // Reported rather than swallowed. This is the only end of the relay that knows why nothing
        // was sent, and until it spoke up the receiving device just sat on an open socket that
        // delivered no bytes and eventually timed out.
        let status = res.status();
        let detail = res.text().await.unwrap_or_default();
        anyhow::bail!("relay send refused: HTTP {status} {detail}");
    }
    Ok(())
}

/// Helper to detect local network IP address for P2P registration.
pub fn detect_local_ip() -> Option<String> {
    // Try local LAN targets first so we bind to the LAN interface (e.g. 192.168.x.x), and only
    // fall back to the public route when none of them answer.
    const LAN_TARGETS: [&str; 4] = [
        "192.168.1.1:80",
        "192.168.0.1:80",
        "10.0.0.1:80",
        "192.168.1.254:80",
    ];
    LAN_TARGETS
        .iter()
        .find_map(|target| route_source_ip(target).filter(|ip| is_lan_candidate(ip)))
        .or_else(|| route_source_ip("8.8.8.8:80"))
}

/// The local address the OS would use to reach `target`, if it is a usable one.
///
/// Nothing is sent: connecting a UDP socket only picks the route.
fn route_source_ip(target: &str) -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect(target).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    if ip.is_loopback() || ip.is_unspecified() {
        None
    } else {
        Some(ip.to_string())
    }
}

/// Ignores Tailscale (100.x) and Docker (172.17/18) addresses, which are reachable from here but
/// not from the peer we are advertising ourselves to.
fn is_lan_candidate(ip: &str) -> bool {
    !ip.starts_with("100.") && !ip.starts_with("172.17.") && !ip.starts_with("172.18.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_non_loopback_ip_or_none() {
        if let Some(ip) = detect_local_ip() {
            assert!(!ip.starts_with("127."));
            assert!(!ip.is_empty());
        }
    }

    #[tokio::test]
    async fn p2p_server_answers_ping() {
        let index = P2PTrackIndex::new();
        let port = 8799;
        let _server = P2PServer::spawn(port, index);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        if let Ok(res) = client
            .get(format!("http://127.0.0.1:{port}/p2p/ping"))
            .send()
            .await
        {
            assert_eq!(res.status(), 200);
            let text = res.text().await.unwrap();
            assert_eq!(text, "pong");
        }
    }
}
