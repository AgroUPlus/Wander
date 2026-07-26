//! Local listening history.
//!
//! Subsonic reports play *counts* but never a play *log*, so the Home tab's
//! statistics — time listened, streaks, what you played this week — have to be
//! recorded here. One JSON object per line, appended on each completed play:
//! append-only means a crash mid-write costs at most the last line, and the
//! file stays readable with `tail`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

use crate::subsonic::models::Song;

/// Roughly a year of heavy listening. Older lines are dropped on load so the
/// file cannot grow without bound.
const MAX_RECORDS: usize = 20_000;
const DAY: i64 = 86_400;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayRecord {
    pub song_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    #[serde(default)]
    pub genres: Vec<String>,
    /// Unix timestamp of when the play completed.
    pub at: i64,
    /// Track length in seconds, i.e. how much was listened to.
    pub secs: u32,
}

impl PlayRecord {
    pub fn from_song(song: &Song) -> Self {
        Self {
            song_id: song.id.clone(),
            title: song.title.clone(),
            artist: song.artist_or_unknown().to_string(),
            album: song.album_or_unknown().to_string(),
            genres: song.genre_names(),
            at: now(),
            secs: song.duration,
        }
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn path() -> Option<PathBuf> {
    Some(crate::paths::cache_dir()?.join("history.jsonl"))
}

/// Append one play. Failures are silent: losing a statistics line must never
/// interrupt playback.
pub fn append(record: &PlayRecord) {
    let Some(path) = path() else { return };
    let Ok(line) = serde_json::to_string(record) else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

pub fn load() -> Vec<PlayRecord> {
    let Some(path) = path() else {
        return Vec::new();
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut records: Vec<PlayRecord> = raw
        .lines()
        // Skip anything unparsable rather than discarding the whole history: a
        // torn final line from a crash should cost one play, not all of them.
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    if records.len() > MAX_RECORDS {
        records.drain(..records.len() - MAX_RECORDS);
    }
    records
}

/// Size of the log, so growth can be detected without reading it.
pub fn size() -> u64 {
    path()
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|meta| meta.len())
        .unwrap_or(0)
}

/// Records appended since byte `offset`, plus the new offset.
///
/// The player writes plays from its own task; this lets the UI pick them up
/// without re-reading a log that only ever grows at the end.
pub fn load_since(offset: u64) -> (Vec<PlayRecord>, u64) {
    use std::io::{Read, Seek, SeekFrom};

    let Some(path) = path() else {
        return (Vec::new(), offset);
    };
    let Ok(mut file) = std::fs::File::open(path) else {
        return (Vec::new(), offset);
    };
    let end = file.metadata().map(|m| m.len()).unwrap_or(0);
    // A shorter file means it was rotated or truncated; start over.
    if end < offset {
        return (load(), end);
    }
    if end == offset || file.seek(SeekFrom::Start(offset)).is_err() {
        return (Vec::new(), end);
    }

    let mut raw = String::new();
    if file.read_to_string(&mut raw).is_err() {
        return (Vec::new(), offset);
    }
    let records = raw
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    (records, end)
}

/// How many day buckets the charts cover.
pub const SPARKLINE_DAYS: usize = 14;
pub const HEATMAP_DAYS: usize = 56;

/// Everything the Home tab shows, computed in one pass.
#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub secs_today: u64,
    pub secs_week: u64,
    pub secs_total: u64,
    pub plays_total: usize,
    /// Consecutive days up to today with at least one play.
    pub streak: u32,
    pub top_artists: Vec<(String, u32)>,
    pub top_albums: Vec<(String, u32)>,
    pub top_tracks: Vec<(String, u32)>,
    pub top_genres: Vec<(String, u32)>,
    /// Seconds listened per day, oldest first, ending with today.
    pub by_day: Vec<u64>,
    /// Seconds listened per day over the heatmap window, oldest first.
    pub heatmap: Vec<u64>,
    /// Seconds listened per hour of the day, index 0 = 00:00 UTC.
    pub by_hour: [u64; 24],
}

impl Stats {
    /// Fold the track currently playing into the totals, so the Home numbers
    /// advance while listening instead of only at track changes.
    ///
    /// Applied to a copy at render time rather than written into the history:
    /// the play is not finished, and may yet be skipped.
    pub fn with_in_progress(&self, secs: u64, hour: usize) -> Stats {
        let mut live = self.clone();
        live.secs_today += secs;
        live.secs_week += secs;
        live.secs_total += secs;
        if let Some(today) = live.by_day.last_mut() {
            *today += secs;
        }
        if let Some(today) = live.heatmap.last_mut() {
            *today += secs;
        }
        live.by_hour[hour.min(23)] += secs;
        live
    }
}

pub fn stats(records: &[PlayRecord], top_n: usize) -> Stats {
    let now = now();
    // Local midnight is not knowable without a timezone database, so "today"
    // means the last 24 hours. It is the number a listener actually wants.
    let day_ago = now - DAY;
    let week_ago = now - 7 * DAY;

    let mut stats = Stats {
        plays_total: records.len(),
        by_day: vec![0; SPARKLINE_DAYS],
        heatmap: vec![0; HEATMAP_DAYS],
        ..Default::default()
    };
    let mut artists: HashMap<&str, u32> = HashMap::new();
    let mut albums: HashMap<String, u32> = HashMap::new();
    let mut tracks: HashMap<String, u32> = HashMap::new();
    let mut genres: HashMap<&str, u32> = HashMap::new();

    for record in records {
        stats.secs_total += record.secs as u64;
        if record.at >= day_ago {
            stats.secs_today += record.secs as u64;
        }
        if record.at >= week_ago {
            stats.secs_week += record.secs as u64;
        }
        *artists.entry(record.artist.as_str()).or_default() += 1;
        *albums
            .entry(format!("{} — {}", record.album, record.artist))
            .or_default() += 1;
        *tracks
            .entry(format!("{} — {}", record.title, record.artist))
            .or_default() += 1;
        for genre in &record.genres {
            *genres.entry(genre.as_str()).or_default() += 1;
        }

        // Bucket by whole days back from now; the last slot is "today".
        let days_ago = (now - record.at).div_euclid(DAY);
        if days_ago >= 0 {
            if (days_ago as usize) < SPARKLINE_DAYS {
                stats.by_day[SPARKLINE_DAYS - 1 - days_ago as usize] += record.secs as u64;
            }
            if (days_ago as usize) < HEATMAP_DAYS {
                stats.heatmap[HEATMAP_DAYS - 1 - days_ago as usize] += record.secs as u64;
            }
        }
        stats.by_hour[(record.at.rem_euclid(DAY) / 3600) as usize] += record.secs as u64;
    }

    stats.top_artists = rank(artists.into_iter().map(|(k, v)| (k.to_string(), v)), top_n);
    stats.top_albums = rank(albums.into_iter(), top_n);
    stats.top_tracks = rank(tracks.into_iter(), top_n);
    stats.top_genres = rank(genres.into_iter().map(|(k, v)| (k.to_string(), v)), top_n);
    stats.streak = streak(records, now);
    stats
}

fn rank(entries: impl Iterator<Item = (String, u32)>, top_n: usize) -> Vec<(String, u32)> {
    let mut entries: Vec<(String, u32)> = entries.collect();
    // Name as a tiebreak so the list does not jitter between equal counts.
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    entries.truncate(top_n);
    entries
}

/// Consecutive 24-hour buckets ending now that contain at least one play.
fn streak(records: &[PlayRecord], now: i64) -> u32 {
    if records.is_empty() {
        return 0;
    }
    let days: std::collections::HashSet<i64> = records
        .iter()
        .map(|r| (now - r.at).div_euclid(DAY))
        .collect();
    let mut streak = 0;
    while days.contains(&streak) {
        streak += 1;
    }
    streak as u32
}

/// Genres worth offering as a daily mix, most-listened first.
pub fn mix_genres(stats: &Stats, count: usize) -> Vec<String> {
    stats
        .top_genres
        .iter()
        .take(count)
        .map(|(g, _)| g.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(artist: &str, genre: &str, ago_secs: i64, secs: u32) -> PlayRecord {
        PlayRecord {
            song_id: format!("{artist}-{ago_secs}"),
            title: format!("track {ago_secs}"),
            artist: artist.to_string(),
            album: "album".to_string(),
            genres: vec![genre.to_string()],
            at: now() - ago_secs,
            secs,
        }
    }

    #[test]
    fn sums_listening_time_per_window() {
        let records = vec![
            record("a", "rock", 60, 100),
            record("b", "rock", 3 * DAY, 200),
            record("c", "jazz", 30 * DAY, 400),
        ];
        let stats = stats(&records, 5);
        assert_eq!(stats.secs_today, 100);
        assert_eq!(stats.secs_week, 300, "today's play counts in the week too");
        assert_eq!(stats.secs_total, 700);
        assert_eq!(stats.plays_total, 3);
    }

    #[test]
    fn ranks_artists_by_play_count() {
        let records = vec![
            record("a", "rock", 60, 10),
            record("b", "rock", 120, 10),
            record("a", "rock", 180, 10),
        ];
        let stats = stats(&records, 2);
        assert_eq!(stats.top_artists[0], ("a".to_string(), 2));
        assert_eq!(stats.top_artists[1], ("b".to_string(), 1));
    }

    #[test]
    fn streak_counts_consecutive_days_and_stops_at_a_gap() {
        let records = vec![
            record("a", "rock", 60, 10),
            record("a", "rock", DAY + 60, 10),
            // Nothing on day 2, so the streak ends at two.
            record("a", "rock", 3 * DAY + 60, 10),
        ];
        assert_eq!(stats(&records, 5).streak, 2);
    }

    #[test]
    fn daily_buckets_put_the_newest_play_last() {
        let records = vec![
            record("a", "rock", 60, 100),          // today
            record("a", "rock", 2 * DAY + 60, 50), // two days ago
        ];
        let stats = stats(&records, 5);
        assert_eq!(stats.by_day.len(), SPARKLINE_DAYS);
        assert_eq!(*stats.by_day.last().unwrap(), 100, "today is the last slot");
        assert_eq!(stats.by_day[SPARKLINE_DAYS - 3], 50);
        assert_eq!(stats.by_day.iter().sum::<u64>(), 150);
    }

    #[test]
    fn plays_older_than_the_window_are_left_out_of_the_charts() {
        let records = vec![record("a", "rock", 100 * DAY, 999)];
        let stats = stats(&records, 5);
        assert_eq!(stats.by_day.iter().sum::<u64>(), 0);
        assert_eq!(stats.heatmap.iter().sum::<u64>(), 0);
        assert_eq!(
            stats.secs_total, 999,
            "but they still count towards the total"
        );
    }

    #[test]
    fn hourly_buckets_cover_the_whole_day() {
        let records: Vec<PlayRecord> = (0..24).map(|h| record("a", "rock", h * 3600, 60)).collect();
        let stats = stats(&records, 5);
        assert_eq!(stats.by_hour.iter().sum::<u64>(), 24 * 60);
        assert!(
            stats.by_hour.iter().all(|&secs| secs == 60),
            "one play per hour"
        );
    }

    #[test]
    fn the_track_in_progress_is_added_to_every_running_total() {
        let base = stats(&[record("a", "rock", 60, 100)], 5);
        let live = base.with_in_progress(30, 5);

        assert_eq!(live.secs_today, base.secs_today + 30);
        assert_eq!(live.secs_week, base.secs_week + 30);
        assert_eq!(live.secs_total, base.secs_total + 30);
        assert_eq!(
            *live.by_day.last().unwrap(),
            *base.by_day.last().unwrap() + 30
        );
        assert_eq!(live.by_hour[5], base.by_hour[5] + 30);
        assert_eq!(
            live.plays_total, base.plays_total,
            "it has not finished yet"
        );
    }

    #[test]
    fn an_out_of_range_hour_cannot_index_past_the_day() {
        let live = Stats::default().with_in_progress(10, 99);
        assert_eq!(live.by_hour[23], 10);
    }

    #[test]
    fn an_empty_history_yields_zeroes_rather_than_panicking() {
        let stats = stats(&[], 5);
        assert_eq!(stats.streak, 0);
        assert!(stats.top_artists.is_empty());
        assert!(mix_genres(&stats, 4).is_empty());
    }
}
