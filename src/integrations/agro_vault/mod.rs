//! The account's vault key, and opening what a sealed session hides with it.
//!
//! When Wanda holds the vault key it plays a *private session*: the handoff it publishes carries
//! "Private Session" and an empty artist in the plaintext fields, and the real track inside
//! `encryptedPayload`, which only a device holding the same vault key can open. Without this module
//! Wander could only ever show the placeholder — in its own player bar and in Discord.
//!
//! The format is Wanda's (`AgroVault`), and has to stay byte-for-byte the same:
//!
//!   * The server keeps the vault key wrapped under the passphrase: AES-256-GCM, keyed by
//!     Argon2id (v1.3, 64 MiB, three passes, four lanes) over the trimmed passphrase and a hex salt.
//!   * A handoff is sealed under a subkey, HKDF-SHA256 of the vault key with no salt and the info
//!     `agro/v1/presence`, so the root key is never used to encrypt anything itself.
//!   * Every sealed value is standard base64 of `nonce (12) || ciphertext || tag (16)`.
//!
//! The passphrase is used once and dropped. What is kept is the vault key itself, in the OS
//! keyring — never the config file, which holds nothing that would open a sealed session.

use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use base64::prelude::*;
use serde_json::json;
use zeroize::Zeroizing;

use crate::integrations::agro::AgroClient;

mod crypto;
#[cfg(test)]
mod tests;

use crypto::{KEY_BYTES, decode_hex, unwrap_vault_key, wrapping_key};
pub use crypto::{SealedHandoff, VaultKey, open_handoff};

/// The keyring service the vault key is stored under, apart from the server password's.
const KEYRING_SERVICE: &str = "wander-agro-vault";

/// What the settings panel says about private sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultState {
    /// Not looked yet this run.
    Unknown,
    /// No key on this machine: private sessions show as "Private Session".
    Locked,
    Unlocked,
}

static STATE: AtomicU8 = AtomicU8::new(0);
/// The key in memory, with the account it belongs to, so a switch of account cannot reuse it.
static KEY: RwLock<Option<(String, VaultKey)>> = RwLock::new(None);
/// One automatic unlock per run. A passphrase that failed once fails every time, and each try
/// costs a 64 MiB Argon2 pass and a request.
static AUTO_TRIED: AtomicBool = AtomicBool::new(false);

pub fn state() -> VaultState {
    match STATE.load(Ordering::Relaxed) {
        1 => VaultState::Locked,
        2 => VaultState::Unlocked,
        _ => VaultState::Unknown,
    }
}

fn set_state(state: VaultState) {
    let raw = match state {
        VaultState::Unknown => 0,
        VaultState::Locked => 1,
        VaultState::Unlocked => 2,
    };
    STATE.store(raw, Ordering::Relaxed);
}

/// Whose key: one Agro account on one server.
fn account(username: &str, server: &str) -> String {
    format!(
        "{}@{}",
        username.trim().to_lowercase(),
        server.trim().trim_end_matches('/')
    )
}

/// The vault key for this client's account: from memory, else from the keyring. Blocking.
pub fn key_for(client: &AgroClient) -> Option<VaultKey> {
    let who = account(&client.username, &client.server);
    if let Ok(guard) = KEY.read()
        && let Some((owner, key)) = guard.as_ref()
        && *owner == who
    {
        return Some(key.clone());
    }
    // Looked once and found nothing: the handoff is polled every few seconds, and the keyring is
    // not asked again until an unlock puts a key in memory.
    if state() == VaultState::Locked {
        return None;
    }
    let stored = {
        let who = who.clone();
        crate::paths::off_runtime(move || {
            let secret = keyring::Entry::new(KEYRING_SERVICE, &who)
                .context("opening keyring entry")?
                .get_password()
                .context("no vault key in the keyring")?;
            Ok(Zeroizing::new(secret))
        })
    };
    let key = stored.ok().and_then(|encoded| {
        let bytes = Zeroizing::new(BASE64_STANDARD.decode(encoded.trim()).ok()?);
        let key: [u8; KEY_BYTES] = bytes.as_slice().try_into().ok()?;
        Some(Zeroizing::new(key))
    });
    match key {
        Some(key) => {
            remember(who, key.clone());
            Some(key)
        }
        None => {
            set_state(VaultState::Locked);
            None
        }
    }
}

fn remember(who: String, key: VaultKey) {
    if let Ok(mut guard) = KEY.write() {
        *guard = Some((who, key));
    }
    set_state(VaultState::Unlocked);
}

/// Fetches the account's wrapped vault key, opens it with `passphrase`, and keeps it.
pub async fn unlock(client: &AgroClient, passphrase: &str) -> Result<()> {
    let answer = client
        .graphql(&json!({ "query": "query { vaultKeyEnvelope { vaultSalt vaultKeyWrapped } }" }))
        .await?;
    let envelope = &answer["data"]["vaultKeyEnvelope"];
    if envelope.is_null() {
        bail!("this account has no vault key yet: set one up in Wanda first");
    }
    let salt = decode_hex(
        envelope["vaultSalt"]
            .as_str()
            .context("the vault key arrived without its salt")?,
    )?;
    let wrapped = envelope["vaultKeyWrapped"]
        .as_str()
        .context("the vault key arrived without its seal")?
        .to_string();
    let passphrase = Zeroizing::new(passphrase.to_string());

    // Argon2 at 64 MiB is hundreds of milliseconds of CPU: kept off the async workers.
    let key = tokio::task::spawn_blocking(move || {
        let wrapping = wrapping_key(&passphrase, &salt)?;
        unwrap_vault_key(&wrapped, &wrapping)
    })
    .await
    .map_err(|_| anyhow!("the unlock task panicked"))??;

    let who = account(&client.username, &client.server);
    let encoded = Zeroizing::new(BASE64_STANDARD.encode(&key[..]));
    {
        let who = who.clone();
        crate::paths::off_runtime(move || {
            keyring::Entry::new(KEYRING_SERVICE, &who)
                .context("opening keyring entry")?
                .set_password(&encoded)
                .context("storing the vault key in the OS keyring")
        })?;
    }
    remember(who, key);
    Ok(())
}

/// Unlocks with the passphrase in the config, once a run, when the config still holds one.
///
/// A device signed in with its passphrase keeps it in the config, so asking for it again would
/// only be friction. One paired by token has none, and is unlocked from the settings panel.
pub async fn unlock_with_config_passphrase(client: &AgroClient, passphrase: &str) -> bool {
    if passphrase.trim().is_empty() || AUTO_TRIED.swap(true, Ordering::Relaxed) {
        return false;
    }
    unlock(client, passphrase).await.is_ok()
}

/// Forgets the key on this machine, from memory and from the keyring.
pub fn forget(username: &str, server: &str) -> Result<()> {
    let who = account(username, server);
    if let Ok(mut guard) = KEY.write() {
        *guard = None;
    }
    set_state(VaultState::Locked);
    crate::paths::off_runtime(move || {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, &who) {
            let _ = entry.delete_credential();
        }
        Ok(())
    })
}
