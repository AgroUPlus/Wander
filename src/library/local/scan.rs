//! Walking the configured music folders and reading tags.
//!
//! Blocking work by nature — filesystem traversal plus a tag read per file —
//! so callers run it on `spawn_blocking` and watch progress through a callback
//! rather than waiting for the whole scan.

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::{Accessor, ItemKey};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::index::{LocalIndex, LocalTrack};

/// Extensions worth opening. Mirrors the decoders the player actually has, so
/// the index never lists something that cannot be played.
const AUDIO_EXTENSIONS: &[&str] = &[
    "opus", "mp3", "flac", "m4a", "m4b", "mp4", "aac", "alac", "ogg", "oga", "wav", "wave", "mka",
    "webm",
];

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Scan `roots`, reusing tag data from `previous` for files that have not
/// changed.
///
/// `progress` is called with the running file count so a long first scan shows
/// movement instead of appearing hung.
pub fn scan(
    roots: &[PathBuf],
    previous: &LocalIndex,
    mut progress: impl FnMut(usize),
) -> LocalIndex {
    // Index the previous scan by path so the incremental check is a lookup
    // rather than a linear search per file.
    let known: HashMap<&Path, &LocalTrack> = previous
        .tracks
        .iter()
        .map(|t| (t.path.as_path(), t))
        .collect();

    let mut tracks = Vec::new();
    let mut seen = 0usize;

    for root in roots {
        let walker = walkdir::WalkDir::new(root)
            .follow_links(true)
            .into_iter()
            // A permission error on one directory must not abort the scan.
            .filter_map(|entry| entry.ok());

        for entry in walker {
            let path = entry.path();
            if !entry.file_type().is_file() || !is_audio(path) {
                continue;
            }

            let (mtime, size) = match stamp(path) {
                Some(stamp) => stamp,
                None => continue,
            };

            // Unchanged file: keep the tags we already read.
            if let Some(previous) = known.get(path)
                && previous.mtime == mtime
                && previous.size == size
            {
                tracks.push((*previous).clone());
                seen += 1;
                progress(seen);
                continue;
            }

            if let Some(track) = read_track(path, mtime, size) {
                tracks.push(track);
            }
            seen += 1;
            progress(seen);
        }
    }

    LocalIndex {
        tracks,
        roots: roots.to_vec(),
    }
}

/// Pull the year out of a date tag. The value may be a bare `1999` or a full
/// `1999-05-12`, and the ecosystem stores both under the same key.
fn parse_year(value: &str) -> Option<u32> {
    let digits: String = value.trim().chars().take(4).collect();
    digits.parse().ok().filter(|y| (1000..=9999).contains(y))
}

fn stamp(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some((mtime, meta.len()))
}

/// Read one file's tags into a [`LocalTrack`].
///
/// An unreadable or untagged file still becomes a track — filename as title —
/// because a file the user can hear should be a file they can find.
fn read_track(path: &Path, mtime: u64, size: u64) -> Option<LocalTrack> {
    let suffix = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());

    let filename_title = || {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string()
    };

    let tagged = match lofty::read_from_path(path) {
        Ok(tagged) => tagged,
        // Not decodable by lofty, but possibly still playable; keep it listed
        // with what the filename tells us.
        Err(_) => {
            return Some(LocalTrack {
                path: path.to_path_buf(),
                mtime,
                size,
                title: filename_title(),
                artist: None,
                album_artist: None,
                album: None,
                track: None,
                disc: None,
                year: None,
                genre: None,
                duration: 0,
                bit_rate: 0,
                suffix,
            });
        }
    };

    let properties = tagged.properties();
    let duration = properties.duration().as_secs() as u32;
    let bit_rate = properties.audio_bitrate().unwrap_or(0);

    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());

    let (title, artist, album_artist, album, track, disc, year, genre) = match tag {
        Some(tag) => (
            tag.title()
                .map(|t| t.to_string())
                .unwrap_or_else(filename_title),
            tag.artist().map(|a| a.to_string()),
            tag.get_string(ItemKey::AlbumArtist).map(|a| a.to_string()),
            tag.album().map(|a| a.to_string()),
            tag.track(),
            tag.disk(),
            tag.get_string(ItemKey::Year)
                .or_else(|| tag.get_string(ItemKey::RecordingDate))
                .and_then(parse_year),
            tag.genre().map(|g| g.to_string()),
        ),
        None => (filename_title(), None, None, None, None, None, None, None),
    };

    Some(LocalTrack {
        path: path.to_path_buf(),
        mtime,
        size,
        title,
        artist,
        album_artist,
        album,
        track,
        disc,
        year,
        genre,
        duration,
        bit_rate,
        suffix,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_playable_extensions_are_scanned() {
        assert!(is_audio(Path::new("/music/a.flac")));
        assert!(
            is_audio(Path::new("/music/a.MP3")),
            "extension match is case-insensitive"
        );
        assert!(!is_audio(Path::new("/music/cover.jpg")));
        assert!(!is_audio(Path::new("/music/album.cue")));
        assert!(!is_audio(Path::new("/music/no-extension")));
    }

    /// The whole point of the (mtime, size) stamp: a second scan over an
    /// unchanged library must not re-read a single tag.
    #[test]
    fn unchanged_files_are_reused_from_the_previous_index() {
        let dir = std::env::temp_dir().join(format!("wander-scan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("track.mp3");
        std::fs::write(&file, b"not really an mp3").unwrap();

        let roots = vec![dir.clone()];
        let first = scan(&roots, &LocalIndex::default(), |_| {});
        assert_eq!(first.tracks.len(), 1);

        // Mark the cached entry so we can tell a reuse from a re-read.
        let mut cached = first.clone();
        cached.tracks[0].title = "cached".into();
        let second = scan(&roots, &cached, |_| {});
        assert_eq!(second.tracks[0].title, "cached");

        std::fs::remove_dir_all(&dir).ok();
    }
}
