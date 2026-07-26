use anyhow::{Context, Result};
use std::path::PathBuf;

// Metadata caching lives in `App`: its `artists`, `albums`, `playlists`, and
// per-selection song lists are loaded once at startup and only re-fetched when
// the user asks for a refresh, which is the right trade-off for a personal
// library that changes rarely.

/// On-disk cache for cover art, keyed by cover ID and requested size.
pub struct CoverCache {
    dir: PathBuf,
}

impl CoverCache {
    pub fn new() -> Result<Self> {
        let dir = crate::paths::cache_dir()
            .context("could not determine cache directory")?
            .join("covers");
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating cover cache at {}", dir.display()))?;
        Ok(Self { dir })
    }

    fn path_for(&self, cover_id: &str, size: u32) -> PathBuf {
        // Cover IDs are opaque server strings and may contain path separators,
        // so sanitise before using one as a filename.
        let safe: String = cover_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir.join(format!("{safe}-{size}"))
    }

    pub fn get(&self, cover_id: &str, size: u32) -> Option<Vec<u8>> {
        std::fs::read(self.path_for(cover_id, size)).ok()
    }

    /// On-disk path, but only if the cover is already cached.
    ///
    /// MPRIS clients (the desktop bar, lock screen) fetch art by URL, and a
    /// `file://` path to our cache avoids handing them a URL containing our
    /// Navidrome auth token.
    pub fn cached_path(&self, cover_id: &str, size: u32) -> Option<PathBuf> {
        let path = self.path_for(cover_id, size);
        path.exists().then_some(path)
    }

    pub fn put(&self, cover_id: &str, size: u32, bytes: &[u8]) {
        // A cache write failure must never interrupt playback or rendering.
        let _ = std::fs::write(self.path_for(cover_id, size), bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitises_cover_ids_into_flat_filenames() {
        let cache = CoverCache {
            dir: PathBuf::from("/tmp/wander-test"),
        };
        let path = cache.path_for("al-../../etc/passwd", 300);
        let name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(name, "al-______etc_passwd-300");
        assert_eq!(path.parent().unwrap(), PathBuf::from("/tmp/wander-test"));
    }

    #[test]
    fn different_sizes_get_different_entries() {
        let cache = CoverCache {
            dir: PathBuf::from("/tmp/wander-test"),
        };
        assert_ne!(cache.path_for("abc", 100), cache.path_for("abc", 300));
    }
}
