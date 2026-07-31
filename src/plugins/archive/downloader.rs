use anyhow::{Context, Result};
use reqwest::Client;
use std::path::{Path, PathBuf};

use super::api::{ArchiveFile, ArchiveItem};
use crate::plugins::sanitize_filename;

const USER_AGENT: &str = "wander-tui/0.1 (https://archive.org; music player)";

/// Download every file of an item into `<target_dir>/<item title>/`.
///
/// Reports progress through `on_file` so the UI can show which track is in
/// flight rather than freezing on a long multi-disc concert.
pub async fn download_archive_item(
    client: &Client,
    item: &ArchiveItem,
    files: &[ArchiveFile],
    target_dir: &Path,
    mut on_file: impl FnMut(usize, usize, &str),
) -> Result<PathBuf> {
    let album_dir = target_dir.join(sanitize_filename(&item.title));
    tokio::fs::create_dir_all(&album_dir)
        .await
        .with_context(|| format!("Failed to create download directory {}", album_dir.display()))?;

    for (index, file) in files.iter().enumerate() {
        let url = file.stream_url(&item.identifier);
        // Archive only ever hands back its own https download hosts; anything
        // else means the metadata was tampered with in transit.
        if !url.starts_with("https://") {
            anyhow::bail!("Security check failed: download URL must use HTTPS");
        }

        // The name may contain sub-directories; only the leaf becomes a file so
        // nothing can escape the album directory.
        let leaf = file.name.rsplit('/').next().unwrap_or(&file.name);
        let target_file = album_dir.join(sanitize_filename(leaf));
        if !target_file.starts_with(&album_dir) {
            anyhow::bail!("Security check failed: path traversal detected in file name");
        }

        on_file(index + 1, files.len(), leaf);

        let response = client
            .get(&url)
            .header("User-Agent", USER_AGENT)
            .send()
            .await
            .with_context(|| format!("Failed to download {url}"))?;

        if !response.status().is_success() {
            anyhow::bail!(
                "archive.org returned status code {} for {leaf}",
                response.status()
            );
        }

        let bytes = response
            .bytes()
            .await
            .with_context(|| format!("Failed to read bytes of {leaf}"))?;

        tokio::fs::write(&target_file, &bytes)
            .await
            .with_context(|| format!("Failed to write {}", target_file.display()))?;
    }

    Ok(album_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn album_dir_is_sanitized() {
        assert_eq!(sanitize_filename("Live at ../etc"), "Live at __etc");
    }
}
