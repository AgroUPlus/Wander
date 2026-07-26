//! Where the app keeps its files, and how it survived being renamed.
//!
//! The project was called `naviplay` before it became `wander`. That rename
//! moves the config directory, the caches and the keyring entry, so anyone
//! upgrading would silently lose their server URL and stored password. The
//! old locations are therefore adopted once, on first run under the new name.

use anyhow::{Context, Result};
use std::path::PathBuf;

/// Name used for directories, the keyring, MPRIS and the Subsonic client id.
pub const APP_NAME: &str = "wander";
/// What the project used to be called.
const LEGACY_NAME: &str = "naviplay";

fn dirs_for(name: &str) -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("", "", name)
}

/// The config directory, migrating the old one across on first run.
pub fn config_dir() -> Result<PathBuf> {
    let dirs = dirs_for(APP_NAME).context("could not determine config directory")?;
    let path = dirs.config_dir().to_path_buf();
    adopt_legacy(&path, || {
        dirs_for(LEGACY_NAME).map(|d| d.config_dir().to_path_buf())
    });
    Ok(path)
}

/// The cache directory (covers, lyrics, play history, saved queue).
pub fn cache_dir() -> Option<PathBuf> {
    let dirs = dirs_for(APP_NAME)?;
    let path = dirs.cache_dir().to_path_buf();
    adopt_legacy(&path, || {
        dirs_for(LEGACY_NAME).map(|d| d.cache_dir().to_path_buf())
    });
    let _ = std::fs::create_dir_all(&path);
    Some(path)
}

/// Rename the old directory into place, once, if the new one does not exist.
///
/// A rename rather than a copy: leaving both would mean edits to one silently
/// not applying. If it fails — different filesystems, permissions — the app
/// simply starts fresh, which is why nothing here is fatal.
fn adopt_legacy(target: &PathBuf, legacy: impl FnOnce() -> Option<PathBuf>) {
    if target.exists() {
        return;
    }
    let Some(legacy) = legacy() else { return };
    if !legacy.exists() {
        return;
    }
    if let Some(parent) = target.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::rename(&legacy, target);
}

/// Read a password from the OS keyring, falling back to the old service name.
///
/// Keyring entries cannot be renamed in place, so the old one is read and
/// re-stored under the new name the first time it is needed.
pub fn keyring_password(username: &str) -> Result<String> {
    let entry = keyring::Entry::new(APP_NAME, username).context("opening keyring entry")?;
    if let Ok(password) = entry.get_password() {
        return Ok(password);
    }

    let legacy = keyring::Entry::new(LEGACY_NAME, username).context("opening keyring entry")?;
    let password = legacy
        .get_password()
        .context("no password in config and none found in the OS keyring")?;
    // Best effort: if this fails the fallback above still works next time.
    let _ = entry.set_password(&password);
    Ok(password)
}

pub fn store_keyring_password(username: &str, password: &str) -> Result<()> {
    keyring::Entry::new(APP_NAME, username)
        .context("opening keyring entry")?
        .set_password(password)
        .context("storing the password in the OS keyring")
}

/// Forget the stored password, so the settings panel can undo a mistake.
///
/// The legacy entry goes too; leaving it would let the fallback in
/// [`keyring_password`] silently resurrect the password on the next launch.
pub fn delete_keyring_password(username: &str) -> Result<()> {
    if let Ok(entry) = keyring::Entry::new(APP_NAME, username) {
        let _ = entry.delete_credential();
    }
    if let Ok(entry) = keyring::Entry::new(LEGACY_NAME, username) {
        let _ = entry.delete_credential();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_existing_directory_is_left_alone() {
        let base = std::env::temp_dir().join(format!("wander-test-{}", std::process::id()));
        let target = base.join("new");
        let legacy = base.join("old");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(target.join("marker"), "new").unwrap();
        std::fs::write(legacy.join("marker"), "old").unwrap();

        adopt_legacy(&target, || Some(legacy.clone()));

        assert_eq!(
            std::fs::read_to_string(target.join("marker")).unwrap(),
            "new"
        );
        assert!(legacy.exists(), "the old directory is not touched");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_old_directory_is_adopted_when_there_is_no_new_one() {
        let base = std::env::temp_dir().join(format!("wander-test-adopt-{}", std::process::id()));
        let target = base.join("new");
        let legacy = base.join("old");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("config.toml"), "url = 'x'").unwrap();

        adopt_legacy(&target, || Some(legacy.clone()));

        assert_eq!(
            std::fs::read_to_string(target.join("config.toml")).unwrap(),
            "url = 'x'"
        );
        assert!(!legacy.exists(), "moved, not copied");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_missing_old_directory_is_not_an_error() {
        let base = std::env::temp_dir().join(format!("wander-test-none-{}", std::process::id()));
        let target = base.join("new");
        adopt_legacy(&target, || Some(base.join("does-not-exist")));
        assert!(!target.exists(), "nothing to adopt, nothing created");
    }
}
