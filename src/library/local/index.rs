//! The scanned local collection, and how it is grouped and persisted.
//!
//! The index is the whole local library: browsing, search and playback all
//! answer from it in memory. It is written to the cache directory so a restart
//! does not need a rescan.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::library::{LOCAL_ALBUM_PREFIX, LOCAL_ARTIST_PREFIX, LOCAL_PREFIX};
use crate::subsonic::models::{Album, Artist, Genre, ItemGenre, Song};

/// One scanned file. Kept separate from [`Song`] because the path and the
/// change-detection stamps have nowhere to live on the Subsonic model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalTrack {
    pub path: PathBuf,
    /// Modification time as seconds since the epoch, paired with `size` to
    /// decide whether a rescan needs to re-read this file's tags.
    pub mtime: u64,
    pub size: u64,

    pub title: String,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub track: Option<u32>,
    pub disc: Option<u32>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub duration: u32,
    pub bit_rate: u32,
    pub suffix: Option<String>,
}

impl LocalTrack {
    /// Stable id for this file. Derived from the path so it survives a rescan
    /// and so a persisted queue still resolves after a restart.
    pub fn id(&self) -> String {
        format!("{LOCAL_PREFIX}{}", path_hash(&self.path))
    }

    /// The artist an album should be filed under. Falls back to the track
    /// artist so a release without an album-artist tag is not lost.
    pub fn effective_album_artist(&self) -> &str {
        self.album_artist
            .as_deref()
            .or(self.artist.as_deref())
            .unwrap_or("Unknown Artist")
    }

    pub fn album_key(&self) -> String {
        format!(
            "{}\u{1}{}",
            self.effective_album_artist().to_lowercase(),
            self.album
                .as_deref()
                .unwrap_or("Unknown Album")
                .to_lowercase()
        )
    }

    pub fn album_id(&self) -> String {
        format!("{LOCAL_ALBUM_PREFIX}{}", hash_str(&self.album_key()))
    }

    pub fn artist_id(&self) -> String {
        format!(
            "{LOCAL_ARTIST_PREFIX}{}",
            hash_str(&self.effective_album_artist().to_lowercase())
        )
    }

    /// Project onto the Subsonic model the rest of the app speaks.
    pub fn to_song(&self) -> Song {
        Song {
            id: self.id(),
            title: self.title.clone(),
            album: self.album.clone(),
            album_id: Some(self.album_id()),
            artist: self.artist.clone().or_else(|| self.album_artist.clone()),
            artist_id: Some(self.artist_id()),
            // The cover is resolved from the track itself (embedded picture or
            // a file beside it), so the song id doubles as the cover id.
            cover_art: Some(self.id()),
            duration: self.duration,
            bit_rate: self.bit_rate,
            track: self.track,
            year: self.year,
            genre: self.genre.clone(),
            suffix: self.suffix.clone(),
            content_type: None,
            size: self.size,
            starred: None,
            user_rating: None,
            play_count: 0,
            genres: self
                .genre
                .iter()
                .map(|g| ItemGenre { name: g.clone() })
                .collect(),
            moods: Vec::new(),
        }
    }
}

/// The persisted index. `tracks` is the source of truth; every other view is
/// derived from it at load time.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct LocalIndex {
    pub tracks: Vec<LocalTrack>,
    /// Roots this index was built from, so a changed configuration is detected
    /// and the stale entries are dropped rather than lingering forever.
    #[serde(default)]
    pub roots: Vec<PathBuf>,
}

impl LocalIndex {
    pub fn path() -> Option<PathBuf> {
        crate::paths::cache_dir().map(|d| d.join("local_index.json"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let Some(path) = Self::path() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&path, serde_json::to_string(self)?)?;
        Ok(())
    }

    /// Look a track up by the id the rest of the app carries around.
    pub fn track(&self, id: &str) -> Option<&LocalTrack> {
        self.tracks.iter().find(|t| t.id() == id)
    }

    pub fn songs(&self) -> Vec<Song> {
        self.tracks.iter().map(|t| t.to_song()).collect()
    }

    /// Albums, grouped on (album artist, album) so a release is not shattered
    /// by per-track artist tags — the usual failure mode for compilations.
    pub fn albums(&self) -> Vec<Album> {
        let mut by_key: HashMap<String, Album> = HashMap::new();
        for track in &self.tracks {
            let entry = by_key.entry(track.album_key()).or_insert_with(|| Album {
                id: track.album_id(),
                name: track
                    .album
                    .clone()
                    .unwrap_or_else(|| "Unknown Album".into()),
                artist: Some(track.effective_album_artist().to_string()),
                artist_id: Some(track.artist_id()),
                cover_art: Some(track.id()),
                song_count: 0,
                duration: 0,
                year: track.year,
                genre: track.genre.clone(),
                music_brainz_id: None,
            });
            entry.song_count += 1;
            entry.duration += track.duration;
            // Earliest year wins: reissue tags on a few tracks should not
            // relabel the release.
            if let Some(year) = track.year {
                entry.year = Some(entry.year.map_or(year, |y| y.min(year)));
            }
        }
        let mut albums: Vec<Album> = by_key.into_values().collect();
        albums.sort_by_key(|a| a.name.to_lowercase());
        albums
    }

    pub fn artists(&self) -> Vec<Artist> {
        let mut by_id: HashMap<String, (Artist, std::collections::HashSet<String>)> =
            HashMap::new();
        for track in &self.tracks {
            let id = track.artist_id();
            let entry = by_id.entry(id.clone()).or_insert_with(|| {
                (
                    Artist {
                        id,
                        name: track.effective_album_artist().to_string(),
                        album_count: 0,
                        cover_art: Some(track.id()),
                    },
                    std::collections::HashSet::new(),
                )
            });
            entry.1.insert(track.album_key());
        }
        let mut artists: Vec<Artist> = by_id
            .into_values()
            .map(|(mut artist, albums)| {
                artist.album_count = albums.len() as u32;
                artist
            })
            .collect();
        artists.sort_by_key(|a| a.name.to_lowercase());
        artists
    }

    pub fn genres(&self) -> Vec<Genre> {
        let mut counts: HashMap<String, (u32, std::collections::HashSet<String>)> = HashMap::new();
        for track in &self.tracks {
            if let Some(genre) = track.genre.as_deref().filter(|g| !g.trim().is_empty()) {
                let entry = counts
                    .entry(genre.to_string())
                    .or_insert((0, std::collections::HashSet::new()));
                entry.0 += 1;
                entry.1.insert(track.album_key());
            }
        }
        let mut genres: Vec<Genre> = counts
            .into_iter()
            .map(|(value, (songs, albums))| Genre {
                value,
                song_count: songs,
                album_count: albums.len() as u32,
            })
            .collect();
        genres.sort_by_key(|g| std::cmp::Reverse(g.song_count));
        genres
    }

    /// Tracks on an album, in disc/track order so an album plays as recorded.
    pub fn album_songs(&self, album_id: &str) -> Vec<Song> {
        let mut tracks: Vec<&LocalTrack> = self
            .tracks
            .iter()
            .filter(|t| t.album_id() == album_id)
            .collect();
        tracks.sort_by_key(|t| (t.disc.unwrap_or(1), t.track.unwrap_or(0), t.title.clone()));
        tracks.iter().map(|t| t.to_song()).collect()
    }

    pub fn artist_albums(&self, artist_id: &str) -> Vec<Album> {
        self.albums()
            .into_iter()
            .filter(|a| a.artist_id.as_deref() == Some(artist_id))
            .collect()
    }
}

/// Short, stable, filename-safe hash. Reuses the md5 already pulled in for
/// Subsonic auth rather than adding another digest.
pub fn hash_str(input: &str) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn path_hash(path: &Path) -> String {
    hash_str(&path.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(album_artist: &str, artist: &str, album: &str, title: &str) -> LocalTrack {
        LocalTrack {
            path: PathBuf::from(format!("/music/{album_artist}/{album}/{title}.flac")),
            mtime: 0,
            size: 100,
            title: title.into(),
            artist: Some(artist.into()),
            album_artist: Some(album_artist.into()),
            album: Some(album.into()),
            track: Some(1),
            disc: None,
            year: Some(1999),
            genre: Some("Jazz".into()),
            duration: 200,
            bit_rate: 900,
            suffix: Some("flac".into()),
        }
    }

    #[test]
    fn ids_round_trip_through_the_index() {
        let index = LocalIndex {
            tracks: vec![track(
                "Miles Davis",
                "Miles Davis",
                "Kind of Blue",
                "So What",
            )],
            roots: Vec::new(),
        };
        let song = index.songs().remove(0);
        assert!(crate::library::is_local_id(&song.id));
        assert_eq!(index.track(&song.id).unwrap().title, "So What");
    }

    /// A compilation tags every track with a different artist; grouping on the
    /// track artist would turn one album into a dozen one-track albums.
    #[test]
    fn compilation_stays_one_album() {
        let index = LocalIndex {
            tracks: vec![
                track("Various Artists", "Alice", "Now 42", "A"),
                track("Various Artists", "Bob", "Now 42", "B"),
                track("Various Artists", "Carol", "Now 42", "C"),
            ],
            roots: Vec::new(),
        };
        let albums = index.albums();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].song_count, 3);
        assert_eq!(albums[0].artist.as_deref(), Some("Various Artists"));
    }

    #[test]
    fn albums_with_the_same_name_by_different_artists_stay_separate() {
        let index = LocalIndex {
            tracks: vec![
                track("Artist One", "Artist One", "Greatest Hits", "A"),
                track("Artist Two", "Artist Two", "Greatest Hits", "B"),
            ],
            roots: Vec::new(),
        };
        assert_eq!(index.albums().len(), 2);
        assert_eq!(index.artists().len(), 2);
    }

    #[test]
    fn album_songs_come_back_in_disc_and_track_order() {
        let mut a = track("X", "X", "Album", "second");
        a.disc = Some(1);
        a.track = Some(2);
        let mut b = track("X", "X", "Album", "first");
        b.disc = Some(1);
        b.track = Some(1);
        let mut c = track("X", "X", "Album", "third");
        c.disc = Some(2);
        c.track = Some(1);

        let index = LocalIndex {
            tracks: vec![c, a, b],
            roots: Vec::new(),
        };
        let album_id = index.albums()[0].id.clone();
        let titles: Vec<String> = index
            .album_songs(&album_id)
            .into_iter()
            .map(|s| s.title)
            .collect();
        assert_eq!(titles, ["first", "second", "third"]);
    }
}
