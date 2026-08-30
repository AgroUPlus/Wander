//! The social surfaces the TUI reads: friends, the activity feed, the recap, and the drop inbox.
//!
//! Wander had none of these — the only social feature it carried was the jam, which is a room you
//! are in rather than a graph you belong to. Everything here is a read of somebody else's data,
//! which means every one of them can legitimately come back empty: each surface is gated on a
//! switch on the *subject's* account, and those default closed. An empty feed is the normal state
//! of a server whose users have not opted in, not a failure worth reporting.
//!
//! Sending a drop is the exception — it writes — and it is refused for a stranger with exactly the
//! same message the server uses for an account that does not exist. There is nothing more specific
//! to tell the user than that it did not go.

use anyhow::Result;
use base64::prelude::*;
use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::{Aead, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::agro::AgroClient;

const DROP_FIELDS: &str =
    "id fromUser toUser trackTitle artistName albumName note noteCiphertext isEncrypted createdAt readAt";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Friend {
    pub username: String,
    pub display_name: Option<String>,
    /// What they are playing, when they let that be seen.
    pub now_playing: Option<String>,
    /// The public identity key for E2EE drops.
    pub public_key: Option<String>,
}

impl Friend {
    /// What to put on screen. Falls back to the username, which always exists.
    pub fn label(&self) -> &str {
        self.display_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&self.username)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drop {
    pub id: String,
    pub from_user: String,
    pub to_user: String,
    pub track_title: String,
    pub artist_name: String,
    pub note: Option<String>,
    pub note_ciphertext: Option<String>,
    pub is_encrypted: bool,
    pub created_at: String,
    /// Always `None` on a drop this account sent — the server blanks it, because whether somebody
    /// opened what you gave them is information about them. Do not draw a "seen" marker from this.
    pub read_at: Option<String>,
}

impl Drop {
    pub fn is_unread(&self) -> bool {
        self.read_at.is_none()
    }
}

/// Seals a drop note to the recipient's public key using X25519-ChaCha20-Poly1305.
pub fn seal_note(recipient_pub_b64: &str, note: &str) -> Result<String> {
    let pub_bytes = BASE64_STANDARD.decode(recipient_pub_b64.trim())?;
    if pub_bytes.len() != 32 {
        anyhow::bail!("invalid recipient public key length");
    }
    let mut pub_arr = [0u8; 32];
    pub_arr.copy_from_slice(&pub_bytes);
    let recipient_pub = x25519_dalek::PublicKey::from(pub_arr);

    let ephemeral_secret = x25519_dalek::StaticSecret::random_from_rng(OsRng);
    let ephemeral_public = x25519_dalek::PublicKey::from(&ephemeral_secret);

    let shared_secret = ephemeral_secret.diffie_hellman(&recipient_pub);
    let mut hasher = Sha256::new();
    hasher.update(shared_secret.as_bytes());
    let key_bytes = hasher.finalize();

    let cipher = ChaCha20Poly1305::new_from_slice(&key_bytes)
        .map_err(|e| anyhow::anyhow!("cipher init error: {e}"))?;

    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, note.as_bytes())
        .map_err(|e| anyhow::anyhow!("encryption failed: {e}"))?;

    let mut payload = Vec::with_capacity(32 + 12 + ciphertext.len());
    payload.extend_from_slice(ephemeral_public.as_bytes());
    payload.extend_from_slice(&nonce_bytes);
    payload.extend_from_slice(&ciphertext);

    Ok(BASE64_STANDARD.encode(payload))
}

/// Opens an encrypted drop note using the local private identity key.
pub fn open_note(my_secret: &x25519_dalek::StaticSecret, ciphertext_b64: &str) -> Result<String> {
    let payload = BASE64_STANDARD.decode(ciphertext_b64.trim())?;
    if payload.len() < 32 + 12 + 16 {
        anyhow::bail!("ciphertext payload too short");
    }

    let mut ephemeral_pub_bytes = [0u8; 32];
    ephemeral_pub_bytes.copy_from_slice(&payload[0..32]);
    let ephemeral_pub = x25519_dalek::PublicKey::from(ephemeral_pub_bytes);

    let nonce_bytes = &payload[32..44];
    let ciphertext = &payload[44..];

    let shared_secret = my_secret.diffie_hellman(&ephemeral_pub);
    let mut hasher = Sha256::new();
    hasher.update(shared_secret.as_bytes());
    let key_bytes = hasher.finalize();

    let cipher = ChaCha20Poly1305::new_from_slice(&key_bytes)
        .map_err(|e| anyhow::anyhow!("cipher init error: {e}"))?;
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("decryption failed: {e}"))?;

    Ok(String::from_utf8(plaintext)?)
}

/// One line of the activity feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedItem {
    pub username: String,
    /// The sentence to show, composed by the server so every client says the same thing.
    pub summary: String,
    pub at: String,
}

/// The circle's shared recap, reduced to the parts a terminal can show usefully.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recap {
    pub members: Vec<String>,
    pub anthem: Option<String>,
    pub trendsetter: Option<String>,
    /// `alice & bob — 72%`, already formatted.
    pub matrix: Vec<String>,
}

fn string(value: &serde_json::Value, key: &str) -> Option<String> {
    value[key]
        .as_str()
        .map(str::to_string)
        .filter(|text| !text.is_empty())
}

fn parse_drop(value: &serde_json::Value, my_secret: Option<&x25519_dalek::StaticSecret>) -> Option<Drop> {
    let raw_note = string(value, "note");
    let note_ciphertext = string(value, "noteCiphertext");
    let is_encrypted = value["isEncrypted"].as_bool().unwrap_or(false);

    let decrypted_note = if is_encrypted {
        if let (Some(secret), Some(cipher_b64)) = (my_secret, &note_ciphertext) {
            match open_note(secret, cipher_b64) {
                Ok(plain) => Some(plain),
                Err(_) => Some("[Encrypted Note - Key Mismatch]".to_string()),
            }
        } else {
            Some("[Encrypted Note]".to_string())
        }
    } else {
        raw_note
    };

    Some(Drop {
        id: string(value, "id")?,
        from_user: string(value, "fromUser").unwrap_or_default(),
        to_user: string(value, "toUser").unwrap_or_default(),
        track_title: string(value, "trackTitle").unwrap_or_default(),
        artist_name: string(value, "artistName").unwrap_or_default(),
        note: decrypted_note,
        note_ciphertext,
        is_encrypted,
        created_at: string(value, "createdAt").unwrap_or_default(),
        read_at: string(value, "readAt"),
    })
}

impl AgroClient {
    /// Accepted friends, with whatever each is playing that they allow to be seen.
    pub async fn friends(&self) -> Result<Vec<Friend>> {
        let answer = self
            .graphql(&json!({
                "query": "{ friends { profile { username displayName publicKey } nowPlaying { trackTitle artistName } } }"
            }))
            .await?;
        Ok(answer["data"]["friends"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| {
                        let profile = &row["profile"];
                        Some(Friend {
                            username: string(profile, "username")?,
                            display_name: string(profile, "displayName"),
                            now_playing: string(&row["nowPlaying"], "trackTitle").map(|title| {
                                match string(&row["nowPlaying"], "artistName") {
                                    Some(artist) => format!("{title} — {artist}"),
                                    None => title,
                                }
                            }),
                            public_key: string(profile, "publicKey"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// What friends have been into lately. Only those who opened `showActivity` appear.
    pub async fn friend_activity(&self) -> Result<Vec<FeedItem>> {
        let answer = self
            .graphql(&json!({
                "query": "{ friendActivity { username summary at } }"
            }))
            .await?;
        Ok(answer["data"]["friendActivity"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| {
                        Some(FeedItem {
                            username: string(row, "username")?,
                            summary: string(row, "summary")?,
                            at: string(row, "at").unwrap_or_default(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The circle's recap for a period, flattened into lines this client can print.
    pub async fn circle_recap(&self, period: &str) -> Result<Recap> {
        let answer = self
            .graphql(&json!({
                "query": "query R($p: String) { circleRecap(period: $p) { \
                          members \
                          anthem { title artist plays } \
                          trendsetter { username firsts } \
                          matrix { a b score } } }",
                "variables": { "p": period }
            }))
            .await?;
        let recap = &answer["data"]["circleRecap"];
        Ok(Recap {
            members: recap["members"]
                .as_array()
                .map(|names| {
                    names
                        .iter()
                        .filter_map(|name| name.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            anthem: string(&recap["anthem"], "title").map(|title| {
                let artist = string(&recap["anthem"], "artist").unwrap_or_default();
                let plays = recap["anthem"]["plays"].as_i64().unwrap_or(0);
                format!("{title} — {artist} ({plays} plays)")
            }),
            trendsetter: string(&recap["trendsetter"], "username").map(|who| {
                let firsts = recap["trendsetter"]["firsts"].as_i64().unwrap_or(0);
                format!("{who}, first to {firsts}")
            }),
            matrix: recap["matrix"]
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .filter_map(|row| {
                            let a = row["a"].as_str()?;
                            let b = row["b"].as_str()?;
                            let score = row["score"].as_i64().unwrap_or(0);
                            Some(format!("{a} & {b} — {score}%"))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// Drops sent to this account and not yet archived.
    pub async fn inbox(&self, my_secret: Option<&x25519_dalek::StaticSecret>) -> Result<Vec<Drop>> {
        let answer = self
            .graphql(&json!({
                "query": format!("{{ inbox {{ {DROP_FIELDS} }} }}")
            }))
            .await?;
        Ok(answer["data"]["inbox"]
            .as_array()
            .map(|rows| rows.iter().filter_map(|r| parse_drop(r, my_secret)).collect())
            .unwrap_or_default())
    }

    /// Hands a track to a friend, encrypting any note with the recipient's public key.
    pub async fn drop_track(
        &self,
        to: &str,
        title: &str,
        artist: &str,
        album: Option<&str>,
        track_uri: Option<&str>,
        note: Option<&str>,
        recipient_pubkey: Option<&str>,
    ) -> Result<()> {
        let (sealed_ciphertext, is_encrypted) = match note.filter(|text| !text.trim().is_empty()) {
            Some(text) => {
                if let Some(pubkey) = recipient_pubkey.filter(|k| !k.trim().is_empty()) {
                    let cipher = seal_note(pubkey, text)?;
                    (Some(cipher), true)
                } else {
                    anyhow::bail!(
                        "Recipient @{to} has not published their E2EE encryption key yet. Plaintext notes are disabled."
                    );
                }
            }
            None => (None, false),
        };

        let answer = self
            .graphql(&json!({
                "query": format!(
                    "mutation D($to: String!, $t: String!, $a: String!, $al: String, \
                     $u: String, $nc: String, $enc: Boolean) {{ \
                     dropTrack(to: $to, trackTitle: $t, artistName: $a, albumName: $al, \
                     trackUri: $u, noteCiphertext: $nc, isEncrypted: $enc) {{ {DROP_FIELDS} }} }}"
                ),
                "variables": {
                    "to": to.trim().to_lowercase(),
                    "t": title,
                    "a": artist,
                    "al": album,
                    "u": track_uri,
                    "nc": sealed_ciphertext,
                    "enc": is_encrypted,
                }
            }))
            .await?;
        if let Some(message) = answer["errors"][0]["message"].as_str() {
            anyhow::bail!("{message}");
        }
        Ok(())
    }

    /// Marks a drop read. Answers `false` for one that is not this account's to mark.
    pub async fn mark_drop_read(&self, id: &str) -> Result<bool> {
        let answer = self
            .graphql(&json!({
                "query": "mutation M($id: String!) { markDropRead(id: $id) }",
                "variables": { "id": id }
            }))
            .await?;
        Ok(answer["data"]["markDropRead"].as_bool().unwrap_or(false))
    }

    /// Takes a drop out of the inbox. The sender's record of having sent it survives.
    pub async fn archive_drop(&self, id: &str) -> Result<bool> {
        let answer = self
            .graphql(&json!({
                "query": "mutation A($id: String!) { archiveDrop(id: $id) }",
                "variables": { "id": id }
            }))
            .await?;
        Ok(answer["data"]["archiveDrop"].as_bool().unwrap_or(false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_and_open_note_roundtrip() {
        let recipient_secret = x25519_dalek::StaticSecret::random_from_rng(OsRng);
        let recipient_public = x25519_dalek::PublicKey::from(&recipient_secret);
        let recipient_pub_b64 = BASE64_STANDARD.encode(recipient_public.as_bytes());

        let secret_message = "Check out this confidential unreleased track!";
        let ciphertext = seal_note(&recipient_pub_b64, secret_message).unwrap();

        assert_ne!(ciphertext, secret_message);
        let decrypted = open_note(&recipient_secret, &ciphertext).unwrap();
        assert_eq!(decrypted, secret_message);
    }

    #[test]
    fn legacy_unencrypted_drop_parses_plain_note() {
        let drop_json = json!({
            "id": "drop-123",
            "fromUser": "alice",
            "toUser": "bob",
            "trackTitle": "Old Song",
            "artistName": "Old Artist",
            "note": "Listen to this!",
            "noteCiphertext": null,
            "isEncrypted": false,
            "createdAt": "2025-01-01T00:00:00Z",
            "readAt": null
        });

        let parsed = parse_drop(&drop_json, None).unwrap();
        assert_eq!(parsed.note.as_deref(), Some("Listen to this!"));
        assert!(!parsed.is_encrypted);
    }
}
