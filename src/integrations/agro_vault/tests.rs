//! Wander and Wanda must agree on the vault byte for byte; most of these pin that.

use super::crypto::*;
use aes_gcm::aead::OsRng;
use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::prelude::*;

fn seal(plain: &[u8], key: &[u8; KEY_BYTES]) -> String {
    let mut nonce = [0u8; NONCE_BYTES];
    OsRng.fill_bytes(&mut nonce);
    let cipher = Aes256Gcm::new_from_slice(key).unwrap();
    let mut out = nonce.to_vec();
    out.extend(cipher.encrypt(Nonce::from_slice(&nonce), plain).unwrap());
    BASE64_STANDARD.encode(out)
}

#[test]
fn a_handoff_sealed_under_the_presence_subkey_opens() {
    let vault = [7u8; KEY_BYTES];
    let sealed = seal(
        br#"{"trackUri":"yt:abc","trackTitle":"Windowlicker","artistName":"Aphex Twin"}"#,
        &presence_key(&vault),
    );
    let opened = open_handoff(&sealed, &vault).unwrap();
    assert_eq!(opened.track_title.as_deref(), Some("Windowlicker"));
    assert_eq!(opened.artist_name.as_deref(), Some("Aphex Twin"));
    assert_eq!(opened.album_name, None);
}

#[test]
fn the_root_key_does_not_open_what_the_subkey_sealed() {
    let vault = [7u8; KEY_BYTES];
    let sealed = seal(br#"{"trackTitle":"x"}"#, &presence_key(&vault));
    assert!(open(&sealed, &vault).is_err());
}

#[test]
fn the_vault_key_unwraps_with_the_passphrase_and_not_without() {
    let salt = decode_hex("00112233445566778899aabbccddeeff").unwrap();
    let vault = [42u8; KEY_BYTES];
    let wrapped = seal(
        &vault,
        &wrapping_key(" four words go here ", &salt).unwrap(),
    );

    let right = wrapping_key("four words go here", &salt).unwrap();
    assert_eq!(*unwrap_vault_key(&wrapped, &right).unwrap(), vault);

    let wrong = wrapping_key("four words went there", &salt).unwrap();
    assert!(unwrap_vault_key(&wrapped, &wrong).is_err());
}

// Produced by Wanda's own `AgroVault` from the same inputs. If any of these stop matching,
// Wander and Wanda no longer agree on the format, and every private session reads as locked.
const SALT_HEX: &str = "00112233445566778899aabbccddeeff";
const PASSPHRASE: &str = "four words go here";
const WANDA_WRAPPING: &str = "d058e4c6e6fbbbb213214642e7e9ffadb66a3a394cc0c70c7d2de6dbf892f52b";
const WANDA_WRAPPED: &str =
    "aKAYKRSi5svZa0tpTzF3tCwiil9XMEaer6kdOX2ZqQgdGFB1MGIVVglwqGzuoHtFVYTe4zc0273i3Tk3";
const WANDA_PRESENCE: &str = "648f01944219d7c409886ab9522e7e90f9e247e8580eb3b3607e5000f4afb8c4";
const WANDA_SEALED: &str = "BOJfmaUiXalx4DmYoS/bwtMQrSwK2chvwJNeZpGba7Ma/qUGyEdFJbtry2jSsaeI09tKfeEzyZmS5iLIOCpUUCf3iBf/t6Ca0VvnZ3LFwk1Q2tY6tXhAL+kyyGncGi4PYnjqhn32Bw==";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn argon2_matches_wanda() {
    let salt = decode_hex(SALT_HEX).unwrap();
    assert_eq!(
        hex(&wrapping_key(PASSPHRASE, &salt).unwrap()[..]),
        WANDA_WRAPPING
    );
}

#[test]
fn a_vault_key_wrapped_by_wanda_unwraps() {
    let salt = decode_hex(SALT_HEX).unwrap();
    let wrapping = wrapping_key(PASSPHRASE, &salt).unwrap();
    assert_eq!(
        *unwrap_vault_key(WANDA_WRAPPED, &wrapping).unwrap(),
        [42u8; KEY_BYTES]
    );
}

#[test]
fn the_presence_subkey_matches_wanda() {
    assert_eq!(hex(&presence_key(&[42u8; KEY_BYTES])[..]), WANDA_PRESENCE);
}

#[test]
fn a_handoff_sealed_by_wanda_opens() {
    let opened = open_handoff(WANDA_SEALED, &[42u8; KEY_BYTES]).unwrap();
    assert_eq!(opened.track_uri.as_deref(), Some("yt:abc"));
    assert_eq!(opened.track_title.as_deref(), Some("Windowlicker"));
    assert_eq!(opened.artist_name.as_deref(), Some("Aphex Twin"));
}
