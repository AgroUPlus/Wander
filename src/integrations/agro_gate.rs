//! Holds back requests Agro has already refused.
//!
//! Every refused request used to be repeated: a 401 set off a fresh `/api/v1/login`, a failed
//! login was tried again on the next request, and the credential that was refused stayed in use.
//! A sync is many requests, so a revoked token — or any account with a second factor, which this
//! client cannot answer — produced two refused POSTs per request. Servers commonly sit behind
//! CrowdSec, whose `http-generic-401-bf` bans an address after six in quick succession, which took
//! the owner's whole network off the server within seconds.
//!
//! Remembered for the life of the process only. A restart, or a config change that builds new
//! clients with a different credential, tries again — which is also how a server that comes back
//! knowing the credential is noticed.

use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;

/// Fingerprints of credentials the server refused, as a bearer or as a login.
///
/// Fingerprints rather than the strings themselves, so this set is not one more place in memory
/// that holds a passphrase. Collisions do not matter: the worst case is one credential not being
/// tried until the next start.
static REFUSED: Mutex<Option<HashSet<u64>>> = Mutex::new(None);

fn fingerprint(parts: &[&str]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for part in parts {
        part.trim().hash(&mut hasher);
    }
    hasher.finish()
}

fn mark(key: u64) {
    if let Ok(mut guard) = REFUSED.lock() {
        guard.get_or_insert_with(HashSet::new).insert(key);
    }
}

fn marked(key: u64) -> bool {
    REFUSED
        .lock()
        .map(|guard| guard.as_ref().is_some_and(|set| set.contains(&key)))
        .unwrap_or(false)
}

/// The server answered 401 to this bearer credential.
pub fn bearer_refused(bearer: &str) {
    mark(fingerprint(&["bearer", bearer]));
}

/// Whether the server already refused this bearer credential in this process.
pub fn is_bearer_refused(bearer: &str) -> bool {
    marked(fingerprint(&["bearer", bearer]))
}

/// Records a 401 against the `Authorization` header that drew it. Any other status says nothing
/// about the credential.
pub fn note_status(authorization: &str, status: reqwest::StatusCode) {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        bearer_refused(authorization.trim().trim_start_matches("Bearer "));
    }
}

/// The server refused this login outright — wrong passphrase, inactive account, or a second
/// factor this client has no way to supply.
pub fn login_refused(server: &str, username: &str, passphrase: &str) {
    mark(fingerprint(&["login", server, username, passphrase]));
}

/// Whether this exact login was already refused in this process.
pub fn is_login_refused(server: &str, username: &str, passphrase: &str) -> bool {
    marked(fingerprint(&["login", server, username, passphrase]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_bearer_is_remembered_and_others_are_not_affected() {
        bearer_refused("gate-test-dead");
        assert!(is_bearer_refused("gate-test-dead"));
        assert!(is_bearer_refused(" gate-test-dead "));
        assert!(!is_bearer_refused("gate-test-alive"));
    }

    #[test]
    fn a_refused_login_is_remembered_per_server_user_and_passphrase() {
        login_refused("https://a", "kim", "gate-test-pass");
        assert!(is_login_refused("https://a", "kim", "gate-test-pass"));
        assert!(!is_login_refused("https://a", "kim", "gate-test-other"));
        assert!(!is_login_refused("https://b", "kim", "gate-test-pass"));
        // A bearer and a login never answer for each other.
        assert!(!is_bearer_refused("gate-test-pass"));
    }
}
