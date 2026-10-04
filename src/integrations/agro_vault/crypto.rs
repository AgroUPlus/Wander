//! The vault's format, byte for byte as Wanda's `AgroVault` writes it; see the module above.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{Context, Result, anyhow, bail};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::prelude::*;
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

const ARGON2_MEMORY_KIB: u32 = 65_536;
const ARGON2_ITERATIONS: u32 = 3;
const ARGON2_PARALLELISM: u32 = 4;
pub(super) const KEY_BYTES: usize = 32;
pub(super) const NONCE_BYTES: usize = 12;
const INFO_PRESENCE: &[u8] = b"agro/v1/presence";

pub type VaultKey = Zeroizing<[u8; KEY_BYTES]>;

/// The key that wraps the vault key, from the account passphrase. Slow on purpose.
pub fn wrapping_key(passphrase: &str, salt: &[u8]) -> Result<VaultKey> {
    let params = Params::new(
        ARGON2_MEMORY_KIB,
        ARGON2_ITERATIONS,
        ARGON2_PARALLELISM,
        Some(KEY_BYTES),
    )
    .map_err(|e| anyhow!("Argon2 parameters: {e}"))?;
    let mut out = Zeroizing::new([0u8; KEY_BYTES]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.trim().as_bytes(), salt, out.as_mut())
        .map_err(|e| anyhow!("Argon2: {e}"))?;
    Ok(out)
}

/// The presence subkey a handoff is sealed under.
pub fn presence_key(vault_key: &[u8; KEY_BYTES]) -> VaultKey {
    let mut out = Zeroizing::new([0u8; KEY_BYTES]);
    Hkdf::<Sha256>::new(None, vault_key)
        .expand(INFO_PRESENCE, out.as_mut())
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    out
}

/// Opens `base64(nonce || ciphertext || tag)`. Fails loudly: a wrong key is an error, not a value.
pub fn open(sealed: &str, key: &[u8; KEY_BYTES]) -> Result<Zeroizing<Vec<u8>>> {
    let bytes = BASE64_STANDARD
        .decode(sealed.trim())
        .context("the sealed value is not base64")?;
    if bytes.len() <= NONCE_BYTES {
        bail!("the sealed value is too short");
    }
    let (nonce, body) = bytes.split_at(NONCE_BYTES);
    let cipher = Aes256Gcm::new_from_slice(key).expect("the key is 32 bytes");
    cipher
        .decrypt(Nonce::from_slice(nonce), body)
        .map(Zeroizing::new)
        .map_err(|_| anyhow!("the sealed value could not be opened with this key"))
}

/// Recovers the vault key from the envelope the server holds.
pub fn unwrap_vault_key(wrapped: &str, wrapping_key: &[u8; KEY_BYTES]) -> Result<VaultKey> {
    let plain = open(wrapped, wrapping_key).context("wrong passphrase, or a damaged vault key")?;
    let key: [u8; KEY_BYTES] = plain
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("the vault key is not 32 bytes"))?;
    Ok(Zeroizing::new(key))
}

pub(super) fn decode_hex(hex: &str) -> Result<Vec<u8>> {
    let hex = hex.trim();
    if !hex.len().is_multiple_of(2) {
        bail!("the vault salt is not hex");
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).context("the vault salt is not hex"))
        .collect()
}

/// What a sealed handoff really says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedHandoff {
    pub track_uri: Option<String>,
    pub track_title: Option<String>,
    pub artist_name: Option<String>,
    pub album_name: Option<String>,
}

/// Opens a handoff's `encryptedPayload` with the vault key.
pub fn open_handoff(sealed: &str, vault_key: &[u8; KEY_BYTES]) -> Result<SealedHandoff> {
    let plain = open(sealed, &presence_key(vault_key))?;
    let fields: serde_json::Value =
        serde_json::from_slice(&plain).context("the sealed handoff is not JSON")?;
    let text = |key: &str| {
        fields
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(String::from)
    };
    Ok(SealedHandoff {
        track_uri: text("trackUri"),
        track_title: text("trackTitle"),
        artist_name: text("artistName"),
        album_name: text("albumName"),
    })
}
