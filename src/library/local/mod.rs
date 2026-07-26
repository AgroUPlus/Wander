//! [`Library`] over a folder of music files.
//!
//! Everything is answered from an in-memory [`LocalIndex`]; the only I/O at
//! request time is reading cover art or lyrics off disk. Starring and ratings
//! have no server to record them, so they live in a small JSON file beside the
//! index.

pub mod index;
pub mod scan;

use anyhow::{Result, bail};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use super::{Capabilities, LOCAL_PLAYLIST_PREFIX, Library, Source};
use crate::subsonic::lyrics::{LyricSet, Lyrics};
use crate::subsonic::models::{Album, Artist, Genre, Playlist, SearchResult3, Song};
use index::{LocalIndex, LocalTrack, hash_str};

/// Cover art filenames to look for beside a track, in preference order.
const COVER_NAMES: &[&str] = &["cover", "folder", "front", "album", "albumart"];
const COVER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

/// User marks that a local file has nowhere else to live.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
struct LocalMarks {
    /// Song id -> ISO-ish timestamp, matching the shape `Song.starred` takes.
    #[serde(default)]
    starred: HashMap<String, String>,
    #[serde(default)]
    ratings: HashMap<String, u8>,
}

impl LocalMarks {
    fn path() -> Option<PathBuf> {
        crate::paths::cache_dir().map(|d| d.join("local_marks.json"))
    }

    fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string(self) {
            let _ = std::fs::write(&path, text);
        }
    }
}

pub struct LocalLibrary {
    index: RwLock<Arc<LocalIndex>>,
    marks: RwLock<LocalMarks>,
    playlist_dir: RwLock<Option<PathBuf>>,
}

impl LocalLibrary {
    /// Build from an already-scanned index.
    pub fn new(index: LocalIndex, playlist_dir: Option<PathBuf>) -> Self {
        Self {
            index: RwLock::new(Arc::new(index)),
            marks: RwLock::new(LocalMarks::load()),
            playlist_dir: RwLock::new(playlist_dir),
        }
    }

    /// Load the persisted index, so startup does not have to wait for a scan.
    pub fn from_cache(playlist_dir: Option<PathBuf>) -> Self {
        Self::new(LocalIndex::load(), playlist_dir)
    }

    pub fn set_index(&self, index: LocalIndex) {
        *self.index.write().unwrap() = Arc::new(index);
    }

    pub fn set_playlist_dir(&self, dir: Option<PathBuf>) {
        *self.playlist_dir.write().unwrap() = dir;
    }

    pub fn index(&self) -> Arc<LocalIndex> {
        Arc::clone(&self.index.read().unwrap())
    }

    pub fn track_count(&self) -> usize {
        self.index().tracks.len()
    }

    pub fn album_count(&self) -> usize {
        self.index().albums().len()
    }

    /// Apply the local star/rating marks to a song on its way out.
    fn decorate(&self, mut song: Song) -> Song {
        let marks = self.marks.read().unwrap();
        song.starred = marks.starred.get(&song.id).cloned();
        song.user_rating = marks.ratings.get(&song.id).copied();
        song
    }

    fn decorate_all(&self, songs: Vec<Song>) -> Vec<Song> {
        songs.into_iter().map(|s| self.decorate(s)).collect()
    }

    fn find(&self, id: &str) -> Option<LocalTrack> {
        self.index().track(id).cloned()
    }

    fn playlist_dir(&self) -> Option<PathBuf> {
        self.playlist_dir.read().unwrap().clone()
    }
}

#[async_trait]
impl Library for LocalLibrary {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            // No server to report plays to; `history.jsonl` already records
            // them locally, which is what the Home tab statistics read.
            scrobble: false,
            // Nothing can serve a file on this machine to the public internet.
            share: false,
            // No similarity service. Radio mode falls back to genre, starred
            // and random, all of which this backend does support.
            similarity: false,
            playlist_write: true,
            rating: true,
        }
    }

    async fn open(&self, song: &Song, _offset: Duration, _force_transcode: bool) -> Result<Source> {
        // Offset is ignored deliberately: a local file is seekable, so the
        // decoder seeks within the stream rather than asking for a re-encode.
        match self.find(&song.id) {
            Some(track) if track.path.exists() => Ok(Source::File(track.path)),
            Some(track) => bail!(
                "{} is no longer on disk; rescan the library",
                track.path.display()
            ),
            None => bail!("this track is not in the local index; rescan the library"),
        }
    }

    async fn artists(&self) -> Result<Vec<Artist>> {
        Ok(self.index().artists())
    }

    async fn artist_albums(&self, artist_id: &str) -> Result<Vec<Album>> {
        Ok(self.index().artist_albums(artist_id))
    }

    async fn album_songs(&self, album_id: &str) -> Result<Vec<Song>> {
        Ok(self.decorate_all(self.index().album_songs(album_id)))
    }

    async fn album_list(&self, kind: &str, size: u32, offset: u32) -> Result<Vec<Album>> {
        let mut albums = self.index().albums();
        match kind {
            // Nothing local records an "added" date beyond file mtime, and the
            // index is already sorted by name, so the orderings that do not
            // apply degrade to alphabetical rather than to nothing.
            "newest" | "recent" => albums.sort_by_key(|a| std::cmp::Reverse(a.year.unwrap_or(0))),
            "random" => {
                for i in (1..albums.len()).rev() {
                    albums.swap(i, rand::random_range(0..=i));
                }
            }
            "starred" => {
                let marks = self.marks.read().unwrap();
                let starred_albums: std::collections::HashSet<String> = self
                    .index()
                    .tracks
                    .iter()
                    .filter(|t| marks.starred.contains_key(&t.id()))
                    .map(|t| t.album_id())
                    .collect();
                albums.retain(|a| starred_albums.contains(&a.id));
            }
            _ => {}
        }
        Ok(albums
            .into_iter()
            .skip(offset as usize)
            .take(size as usize)
            .collect())
    }

    async fn all_songs(&self, count: u32, offset: u32) -> Result<Vec<Song>> {
        Ok(self.decorate_all(
            self.index()
                .songs()
                .into_iter()
                .skip(offset as usize)
                .take(count as usize)
                .collect(),
        ))
    }

    async fn random_songs(&self, count: u32, genre: Option<&str>) -> Result<Vec<Song>> {
        let index = self.index();
        let mut songs: Vec<Song> = match genre {
            Some(genre) => {
                let wanted = genre.to_lowercase();
                index
                    .tracks
                    .iter()
                    .filter(|t| {
                        t.genre
                            .as_deref()
                            .is_some_and(|g| g.to_lowercase() == wanted)
                    })
                    .map(|t| t.to_song())
                    .collect()
            }
            None => index.songs(),
        };
        for i in (1..songs.len()).rev() {
            songs.swap(i, rand::random_range(0..=i));
        }
        songs.truncate(count as usize);
        Ok(self.decorate_all(songs))
    }

    async fn starred_songs(&self) -> Result<Vec<Song>> {
        let starred: Vec<String> = self.marks.read().unwrap().starred.keys().cloned().collect();
        let index = self.index();
        Ok(starred
            .iter()
            .filter_map(|id| index.track(id))
            .map(|t| self.decorate(t.to_song()))
            .collect())
    }

    async fn songs_by_genre(&self, genre: &str, count: u32, offset: u32) -> Result<Vec<Song>> {
        let wanted = genre.to_lowercase();
        let index = self.index();
        Ok(self.decorate_all(
            index
                .tracks
                .iter()
                .filter(|t| {
                    t.genre
                        .as_deref()
                        .is_some_and(|g| g.to_lowercase() == wanted)
                })
                .skip(offset as usize)
                .take(count as usize)
                .map(|t| t.to_song())
                .collect(),
        ))
    }

    async fn genres(&self) -> Result<Vec<Genre>> {
        Ok(self.index().genres())
    }

    async fn top_songs(&self, artist_name: &str, count: u32) -> Result<Vec<Song>> {
        // No play counts on disk, so "top" is simply this artist's tracks.
        // Radio treats the result as candidates, which is all it needs.
        let wanted = artist_name.to_lowercase();
        let index = self.index();
        Ok(self.decorate_all(
            index
                .tracks
                .iter()
                .filter(|t| {
                    t.effective_album_artist().to_lowercase() == wanted
                        || t.artist
                            .as_deref()
                            .is_some_and(|a| a.to_lowercase() == wanted)
                })
                .take(count as usize)
                .map(|t| t.to_song())
                .collect(),
        ))
    }

    async fn search(&self, query: &str, count: u32) -> Result<SearchResult3> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Ok(SearchResult3::default());
        }
        let index = self.index();
        let limit = count as usize;

        let song: Vec<Song> = index
            .tracks
            .iter()
            .filter(|t| {
                t.title.to_lowercase().contains(&needle)
                    || t.artist
                        .as_deref()
                        .is_some_and(|a| a.to_lowercase().contains(&needle))
                    || t.album
                        .as_deref()
                        .is_some_and(|a| a.to_lowercase().contains(&needle))
            })
            .take(limit)
            .map(|t| t.to_song())
            .collect();

        Ok(SearchResult3 {
            artist: index
                .artists()
                .into_iter()
                .filter(|a| a.name.to_lowercase().contains(&needle))
                .take(limit)
                .collect(),
            album: index
                .albums()
                .into_iter()
                .filter(|a| a.name.to_lowercase().contains(&needle))
                .take(limit)
                .collect(),
            song: self.decorate_all(song),
        })
    }

    async fn lyrics(&self, song_id: &str) -> Result<LyricSet> {
        let Some(track) = self.find(song_id) else {
            return Ok(LyricSet::default());
        };
        // A local file carries one set of words; variants come from translating.
        let lyrics: Lyrics = tokio::task::spawn_blocking(move || read_lyrics(&track.path))
            .await
            .unwrap_or_default();
        Ok(lyrics.into())
    }

    async fn cover_art(&self, cover_id: &str, _size: u32) -> Result<Vec<u8>> {
        let Some(track) = self.find(cover_id) else {
            bail!("no local track owns cover art {cover_id}");
        };
        let path = track.path.clone();
        tokio::task::spawn_blocking(move || read_cover(&path))
            .await
            .unwrap_or_else(|err| bail!("cover art read panicked: {err}"))
    }

    async fn playlists(&self) -> Result<Vec<Playlist>> {
        let Some(dir) = self.playlist_dir() else {
            return Ok(Vec::new());
        };
        let index = self.index();
        let mut playlists = Vec::new();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(Vec::new());
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("m3u8")
                && path.extension().and_then(|e| e.to_str()) != Some("m3u")
            {
                continue;
            }
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Playlist")
                .to_string();
            let songs = read_m3u(&path, &index);
            playlists.push(Playlist {
                id: playlist_id(&name),
                name,
                song_count: songs.len() as u32,
                duration: songs.iter().map(|s| s.duration).sum(),
                cover_art: songs.first().and_then(|s| s.cover_art.clone()),
            });
        }
        playlists.sort_by_key(|p| p.name.to_lowercase());
        Ok(playlists)
    }

    async fn playlist_songs(&self, playlist_id: &str) -> Result<Vec<Song>> {
        let Some(path) = self.playlist_path(playlist_id) else {
            return Ok(Vec::new());
        };
        Ok(self.decorate_all(read_m3u(&path, &self.index())))
    }

    async fn create_playlist(&self, name: &str, song_ids: &[String]) -> Result<()> {
        let Some(dir) = self.playlist_dir() else {
            bail!("set a playlist folder in Settings to save local playlists");
        };
        std::fs::create_dir_all(&dir)?;
        let paths = self.paths_for(song_ids);
        write_m3u(&dir.join(format!("{}.m3u8", sanitise(name))), &paths)
    }

    async fn add_to_playlist(&self, playlist_id: &str, song_ids: &[String]) -> Result<()> {
        let Some(path) = self.playlist_path(playlist_id) else {
            bail!("that local playlist no longer exists");
        };
        let mut paths: Vec<PathBuf> = read_m3u_paths(&path);
        paths.extend(self.paths_for(song_ids));
        write_m3u(&path, &paths)
    }

    async fn remove_from_playlist(&self, playlist_id: &str, indices: &[usize]) -> Result<()> {
        let Some(path) = self.playlist_path(playlist_id) else {
            bail!("that local playlist no longer exists");
        };
        let mut paths = read_m3u_paths(&path);
        // Descending, so each removal cannot shift the ones still to come.
        let mut indices = indices.to_vec();
        indices.sort_unstable_by(|a, b| b.cmp(a));
        for index in indices {
            if index < paths.len() {
                paths.remove(index);
            }
        }
        write_m3u(&path, &paths)
    }

    async fn set_starred(&self, song_id: &str, starred: bool) -> Result<()> {
        {
            let mut marks = self.marks.write().unwrap();
            if starred {
                // `Song.starred` is only ever tested for presence, so the unix
                // timestamp the history log already uses is enough here.
                marks
                    .starred
                    .insert(song_id.to_string(), crate::history::now().to_string());
            } else {
                marks.starred.remove(song_id);
            }
            marks.save();
        }
        Ok(())
    }

    async fn set_rating(&self, song_id: &str, rating: u8) -> Result<()> {
        {
            let mut marks = self.marks.write().unwrap();
            if rating == 0 {
                marks.ratings.remove(song_id);
            } else {
                marks.ratings.insert(song_id.to_string(), rating.min(5));
            }
            marks.save();
        }
        Ok(())
    }
}

impl LocalLibrary {
    /// Resolve a playlist id back to the file it names.
    ///
    /// The id is a hash of the name rather than the path, so a playlist that is
    /// renamed on disk becomes a different playlist — which is what the user
    /// sees anyway.
    fn playlist_path(&self, playlist_id: &str) -> Option<PathBuf> {
        let dir = self.playlist_dir()?;
        std::fs::read_dir(&dir).ok()?.flatten().find_map(|entry| {
            let path = entry.path();
            let name = path.file_stem()?.to_str()?;
            (playlist_id == playlist_id_ref(name)).then_some(path)
        })
    }

    fn paths_for(&self, song_ids: &[String]) -> Vec<PathBuf> {
        let index = self.index();
        song_ids
            .iter()
            .filter_map(|id| index.track(id).map(|t| t.path.clone()))
            .collect()
    }
}

fn playlist_id(name: &str) -> String {
    format!("{LOCAL_PLAYLIST_PREFIX}{}", hash_str(name))
}

fn playlist_id_ref(name: &str) -> String {
    playlist_id(name)
}

/// Strip anything that cannot go in a filename, so a playlist called
/// "Rock / Metal" does not try to create a directory.
fn sanitise(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_()[]".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim().to_string();
    if trimmed.is_empty() {
        "Playlist".to_string()
    } else {
        trimmed
    }
}

fn read_m3u_paths(path: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let base = path.parent().unwrap_or(Path::new("."));
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let entry = Path::new(line);
            // Relative entries are the norm in M3U and resolve against the
            // playlist's own directory.
            if entry.is_absolute() {
                entry.to_path_buf()
            } else {
                base.join(entry)
            }
        })
        .collect()
}

/// Read an M3U into songs, matching entries against the index so they carry
/// full metadata rather than just a path.
fn read_m3u(path: &Path, index: &LocalIndex) -> Vec<Song> {
    let by_path: HashMap<&Path, &LocalTrack> =
        index.tracks.iter().map(|t| (t.path.as_path(), t)).collect();

    read_m3u_paths(path)
        .into_iter()
        .filter_map(|entry| {
            by_path
                .get(entry.as_path())
                // Fall back to the canonical form, since an entry may reach the
                // same file by a different route (symlink, `..`, `./`).
                .or_else(|| {
                    let canonical = entry.canonicalize().ok()?;
                    by_path.get(canonical.as_path())
                })
                .map(|t| t.to_song())
        })
        .collect()
}

fn write_m3u(path: &Path, paths: &[PathBuf]) -> Result<()> {
    let mut text = String::from("#EXTM3U\n");
    for entry in paths {
        text.push_str(&entry.to_string_lossy());
        text.push('\n');
    }
    std::fs::write(path, text)?;
    Ok(())
}

/// Cover art for a track: the embedded picture first, then a likely-looking
/// image beside it.
fn read_cover(track: &Path) -> Result<Vec<u8>> {
    use lofty::file::TaggedFileExt;
    use lofty::prelude::ItemKey;
    let _ = ItemKey::AlbumArtist;

    if let Ok(tagged) = lofty::read_from_path(track)
        && let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag())
        && let Some(picture) = tag.pictures().first()
    {
        return Ok(picture.data().to_vec());
    }

    let dir = track.parent().unwrap_or(Path::new("."));
    for name in COVER_NAMES {
        for extension in COVER_EXTENSIONS {
            let candidate = dir.join(format!("{name}.{extension}"));
            if let Ok(bytes) = std::fs::read(&candidate) {
                return Ok(bytes);
            }
        }
    }
    bail!("no cover art next to {}", track.display())
}

/// Lyrics for a track: an embedded tag first, then a sidecar `.lrc`.
fn read_lyrics(track: &Path) -> Lyrics {
    use lofty::file::TaggedFileExt;
    use lofty::prelude::ItemKey;

    if let Ok(tagged) = lofty::read_from_path(track)
        && let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag())
        && let Some(text) = tag.get_string(ItemKey::Lyrics)
        && !text.trim().is_empty()
    {
        return Lyrics::parse_lrc(text);
    }

    if let Ok(text) = std::fs::read_to_string(track.with_extension("lrc"))
        && !text.trim().is_empty()
    {
        return Lyrics::parse_lrc(&text);
    }

    Lyrics::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_names_become_safe_filenames() {
        assert_eq!(sanitise("Rock / Metal"), "Rock _ Metal");
        // Dots go too, so a playlist name can never walk out of its folder.
        assert_eq!(sanitise("../../etc/passwd"), "______etc_passwd");
        assert!(!sanitise("../../etc/passwd").contains(['.', '/']));
        assert_eq!(sanitise("   "), "Playlist");
        assert_eq!(sanitise("Chill (2024)"), "Chill (2024)");
    }

    #[test]
    fn m3u_round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("wander-m3u-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let playlist = dir.join("test.m3u8");
        let paths = vec![
            PathBuf::from("/music/a.flac"),
            PathBuf::from("/music/b.flac"),
        ];

        write_m3u(&playlist, &paths).unwrap();
        assert_eq!(read_m3u_paths(&playlist), paths);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Write a minimal but genuinely valid 16-bit mono WAV, so the scan runs
    /// against a real audio file rather than a stub the tag reader rejects.
    fn write_wav(path: &Path, seconds: u32) {
        const RATE: u32 = 8000;
        let samples = RATE * seconds;
        let data_len = samples * 2;
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend((36 + data_len).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes()); // PCM
        wav.extend(1u16.to_le_bytes()); // mono
        wav.extend(RATE.to_le_bytes());
        wav.extend((RATE * 2).to_le_bytes()); // byte rate
        wav.extend(2u16.to_le_bytes()); // block align
        wav.extend(16u16.to_le_bytes()); // bits per sample
        wav.extend(b"data");
        wav.extend(data_len.to_le_bytes());
        for i in 0..samples {
            let value = ((i as f32 * 0.05).sin() * 8000.0) as i16;
            wav.extend(value.to_le_bytes());
        }
        std::fs::write(path, wav).unwrap();
    }

    /// The whole local path in one go: a file on disk becomes an indexed song
    /// with a routable id, and playback can open it again from that id.
    #[tokio::test]
    async fn a_scanned_file_becomes_a_playable_song() {
        let dir = std::env::temp_dir().join(format!("wander-e2e-{}", std::process::id()));
        let album = dir.join("Some Artist").join("Some Album");
        std::fs::create_dir_all(&album).unwrap();
        write_wav(&album.join("A Song.wav"), 1);

        let roots = vec![dir.clone()];
        let index = crate::library::local::scan::scan(&roots, &LocalIndex::default(), |_| {});
        assert_eq!(index.tracks.len(), 1, "the file should be indexed");

        let library = LocalLibrary::new(index, None);
        let songs = library.all_songs(10, 0).await.unwrap();
        assert_eq!(songs.len(), 1);

        let song = &songs[0];
        // Untagged, so the filename carries the title.
        assert_eq!(song.title, "A Song");
        assert!(
            crate::library::is_local_id(&song.id),
            "a local song must carry a routable local id"
        );

        match library.open(song, Duration::ZERO, false).await.unwrap() {
            Source::File(path) => assert!(path.exists()),
            Source::Http { .. } => panic!("a local track must not open over HTTP"),
        }

        // Starring has no server to go to, so it must round-trip locally.
        library.set_starred(&song.id, true).await.unwrap();
        assert!(library.all_songs(10, 0).await.unwrap()[0].is_starred());
        library.set_starred(&song.id, false).await.unwrap();
        assert!(!library.all_songs(10, 0).await.unwrap()[0].is_starred());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A track whose file has since been deleted must fail with something the
    /// user can act on, not play silence.
    #[tokio::test]
    async fn a_missing_file_reports_a_useful_error() {
        let index = LocalIndex {
            tracks: vec![LocalTrack {
                path: PathBuf::from("/definitely/not/here.flac"),
                mtime: 0,
                size: 0,
                title: "Gone".into(),
                artist: None,
                album_artist: None,
                album: None,
                track: None,
                disc: None,
                year: None,
                genre: None,
                duration: 0,
                bit_rate: 0,
                suffix: Some("flac".into()),
            }],
            roots: Vec::new(),
        };
        let library = LocalLibrary::new(index, None);
        let song = library.all_songs(1, 0).await.unwrap().remove(0);
        let err = library
            .open(&song, Duration::ZERO, false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("rescan"), "got: {err}");
    }

    /// Relative entries are the common case for portable playlists and must
    /// resolve against the playlist file, not the process working directory.
    #[test]
    fn relative_m3u_entries_resolve_against_the_playlist() {
        let dir = std::env::temp_dir().join(format!("wander-m3u-rel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let playlist = dir.join("test.m3u8");
        std::fs::write(&playlist, "#EXTM3U\nsub/track.flac\n").unwrap();

        assert_eq!(read_m3u_paths(&playlist), vec![dir.join("sub/track.flac")]);

        std::fs::remove_dir_all(&dir).ok();
    }
}
