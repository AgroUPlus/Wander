//! Synced-lyrics fetching, caching, and playback-position lookup.
//!
//! Lyrics come from the user's own Navidrome server via the OpenSubsonic
//! `songLyrics` extension. Everything here is about *timing* — which line is
//! current, and how far through it we are — not about the text itself.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// One timed line of a track's lyrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Line {
    /// Offset from the start of the track.
    pub at: Duration,
    pub text: String,
}

/// One rendering of a track's lyrics: a language, a translation, or a
/// romanisation. A track can be offered several.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Lyrics {
    pub synced: bool,
    pub lines: Vec<Line>,
    /// Language tag as the source reported it, e.g. `ja` or `ja-Latn`. Shown in
    /// the pane title so the user can tell which variant they are reading.
    #[serde(default)]
    pub lang: Option<String>,
}

/// Every variant a track offers, plus which one is being read.
///
/// Servers do sometimes carry a romanised or translated version alongside the
/// original; the old code sorted them, took the first, and threw the rest away.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LyricSet {
    pub variants: Vec<Lyrics>,
    #[serde(default)]
    pub active: usize,
}

/// Stands in for "no lyrics", so `Deref` can hand out a reference either way.
static NO_LYRICS: std::sync::LazyLock<Lyrics> = std::sync::LazyLock::new(Lyrics::default);

impl LyricSet {
    /// The variant currently being read.
    pub fn active(&self) -> &Lyrics {
        self.variants.get(self.active).unwrap_or(&NO_LYRICS)
    }

    /// Move to the next variant, wrapping. Returns its label for the status line.
    pub fn cycle(&mut self) -> Option<String> {
        if self.variants.len() < 2 {
            return None;
        }
        self.active = (self.active + 1) % self.variants.len();
        Some(self.label())
    }

    /// How to describe the active variant: its language, or its position when
    /// the source did not say.
    pub fn label(&self) -> String {
        match self.active().lang.as_deref() {
            Some(lang) => lang.to_string(),
            None => format!("{}/{}", self.active + 1, self.variants.len().max(1)),
        }
    }

    /// True when there is more than one thing to read.
    pub fn has_variants(&self) -> bool {
        self.variants.len() > 1
    }

    /// Add a variant and switch to it — how a fresh translation arrives.
    pub fn push_active(&mut self, lyrics: Lyrics) {
        self.variants.push(lyrics);
        self.active = self.variants.len() - 1;
    }
}

/// Lets every existing `app.lyrics.synced` / `.lines` read keep working while
/// the app actually holds a set of variants.
impl std::ops::Deref for LyricSet {
    type Target = Lyrics;

    fn deref(&self) -> &Self::Target {
        self.active()
    }
}

impl From<Lyrics> for LyricSet {
    fn from(lyrics: Lyrics) -> Self {
        if lyrics.is_empty() {
            return Self::default();
        }
        Self {
            variants: vec![lyrics],
            active: 0,
        }
    }
}

impl Lyrics {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Index of the line that should be highlighted at `position`.
    ///
    /// Returns `None` before the first timestamp, which is common while an
    /// intro plays. Uses binary search so this stays cheap at frame rate.
    pub fn active_at(&self, position: Duration) -> Option<usize> {
        if !self.synced || self.lines.is_empty() {
            return None;
        }
        match self.lines.binary_search_by(|line| line.at.cmp(&position)) {
            // Exactly on a line's timestamp.
            Ok(index) => Some(index),
            // `index` is the first line *after* position.
            Err(0) => None,
            Err(index) => Some(index - 1),
        }
    }

    /// Progress through the active line, in `[0, 1]`.
    ///
    /// Drives the fade-in of a newly active line. The final line has no
    /// successor to measure against, so it uses a fixed nominal duration.
    pub fn line_progress(&self, position: Duration) -> f32 {
        const FINAL_LINE_NOMINAL: Duration = Duration::from_secs(4);

        let Some(index) = self.active_at(position) else {
            return 0.0;
        };
        let start = self.lines[index].at;
        let end = self
            .lines
            .get(index + 1)
            .map(|line| line.at)
            .unwrap_or(start + FINAL_LINE_NOMINAL);

        let span = end.saturating_sub(start).as_secs_f32();
        if span <= 0.0 {
            return 1.0;
        }
        (position.saturating_sub(start).as_secs_f32() / span).clamp(0.0, 1.0)
    }

    /// Build from the server's structured representation.
    ///
    /// Every variant is kept — a romanised or translated version is exactly what
    /// the user might want to read — with synced ones ordered first, since the
    /// scrolling view needs timings.
    pub fn from_structured(mut all: Vec<super::models::StructuredLyrics>) -> LyricSet {
        // Prefer synced lyrics; they are what the scrolling view needs.
        all.sort_by_key(|entry| !entry.synced);

        let variants: Vec<Lyrics> = all
            .into_iter()
            .map(|entry| {
                let mut lines: Vec<Line> = entry
                    .line
                    .into_iter()
                    .map(|line| Line {
                        at: Duration::from_millis(line.start.unwrap_or(0)),
                        text: line.value,
                    })
                    .collect();

                // Timestamps must be ascending for the binary search to be
                // valid; the server is not guaranteed to order them.
                if entry.synced {
                    lines.sort_by_key(|line| line.at);
                }

                Lyrics {
                    synced: entry.synced,
                    lines,
                    lang: entry.lang,
                }
            })
            .filter(|lyrics| !lyrics.is_empty())
            .collect();

        LyricSet {
            variants,
            active: 0,
        }
    }

    /// Parse the LRC format, used by embedded lyrics tags and `.lrc` sidecar
    /// files — the local library's equivalent of the server's `songLyrics`.
    ///
    /// A line may carry several timestamps (`[00:12.00][01:30.00]chorus`), and
    /// text with no timestamp at all is plain unsynced lyrics.
    pub fn parse_lrc(text: &str) -> Self {
        let mut lines: Vec<Line> = Vec::new();
        let mut plain: Vec<String> = Vec::new();

        for raw in text.lines() {
            let mut rest = raw.trim();
            let mut stamps: Vec<Duration> = Vec::new();

            // Consume the run of leading `[..]` tags.
            while let Some(close) = rest.strip_prefix('[').and_then(|r| r.find(']')) {
                let tag = &rest[1..close + 1];
                rest = rest[close + 2..].trim_start();
                if let Some(at) = parse_lrc_timestamp(tag) {
                    stamps.push(at);
                }
                // A non-timestamp tag is metadata (`[ar:…]`, `[ti:…]`) and is
                // simply skipped.
            }

            if stamps.is_empty() {
                if !rest.is_empty() {
                    plain.push(rest.to_string());
                }
                continue;
            }
            for at in stamps {
                lines.push(Line {
                    at,
                    text: rest.to_string(),
                });
            }
        }

        if !lines.is_empty() {
            lines.sort_by_key(|line| line.at);
            return Self {
                synced: true,
                lines,
                lang: None,
            };
        }

        Self {
            synced: false,
            lines: plain
                .into_iter()
                .map(|text| Line {
                    at: Duration::ZERO,
                    text,
                })
                .collect(),
            lang: None,
        }
    }
}

/// `mm:ss`, `mm:ss.xx` or `mm:ss.xxx` from inside an LRC tag.
fn parse_lrc_timestamp(tag: &str) -> Option<Duration> {
    let (minutes, rest) = tag.split_once(':')?;
    let minutes: u64 = minutes.trim().parse().ok()?;

    let (seconds, fraction) = match rest.split_once(['.', ':']) {
        Some((seconds, fraction)) => (seconds, fraction),
        None => (rest, ""),
    };
    let seconds: u64 = seconds.trim().parse().ok()?;

    // Two digits mean hundredths, three mean milliseconds.
    let millis: u64 = match fraction.len() {
        0 => 0,
        2 => fraction.parse::<u64>().ok()? * 10,
        3 => fraction.parse::<u64>().ok()?,
        _ => return None,
    };

    Some(Duration::from_millis(
        minutes * 60_000 + seconds * 1_000 + millis,
    ))
}

/// Read a cache entry written by either the current or the previous format.
///
/// Entries used to be a bare `Lyrics`; discarding them all on upgrade would mean
/// re-fetching every track's lyrics — including the negative results that exist
/// precisely to avoid that.
fn parse_cached(raw: &str) -> Option<LyricSet> {
    if let Ok(set) = serde_json::from_str::<LyricSet>(raw) {
        return Some(set);
    }
    serde_json::from_str::<Lyrics>(raw).ok().map(LyricSet::from)
}

/// On-disk cache, including negative results so a track without lyrics is not
/// re-requested on every play.
pub struct LyricsCache {
    dir: PathBuf,
}

impl LyricsCache {
    pub fn new() -> Result<Self> {
        let dir = crate::paths::cache_dir()
            .ok_or_else(|| anyhow::anyhow!("could not determine cache directory"))?
            .join("lyrics");
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn path_for(&self, song_id: &str) -> PathBuf {
        let safe: String = song_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir.join(format!("{safe}.json"))
    }

    pub fn get(&self, song_id: &str) -> Option<LyricSet> {
        let raw = std::fs::read_to_string(self.path_for(song_id)).ok()?;
        parse_cached(&raw)
    }

    pub fn put(&self, song_id: &str, lyrics: &LyricSet) {
        // A cache write failure must never disturb playback.
        if let Ok(raw) = serde_json::to_string(lyrics) {
            let _ = std::fs::write(self.path_for(song_id), raw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lyrics(times_ms: &[u64]) -> Lyrics {
        Lyrics {
            synced: true,
            lines: times_ms
                .iter()
                .enumerate()
                .map(|(i, ms)| Line {
                    at: Duration::from_millis(*ms),
                    text: format!("line {i}"),
                })
                .collect(),
            lang: None,
        }
    }

    #[test]
    fn lrc_timestamps_parse_in_every_precision() {
        assert_eq!(
            parse_lrc_timestamp("01:23"),
            Some(Duration::from_millis(83_000))
        );
        assert_eq!(
            parse_lrc_timestamp("01:23.45"),
            Some(Duration::from_millis(83_450))
        );
        assert_eq!(
            parse_lrc_timestamp("01:23.456"),
            Some(Duration::from_millis(83_456))
        );
        // Metadata tags share the bracket syntax but are not timestamps.
        assert_eq!(parse_lrc_timestamp("ar:Someone"), None);
        assert_eq!(parse_lrc_timestamp("nonsense"), None);
    }

    #[test]
    fn lrc_parses_into_sorted_synced_lines() {
        let parsed = Lyrics::parse_lrc(
            "[ar:Artist]\n[00:30.00]second\n[00:10.00]first\n\n[00:50.00]third\n",
        );
        assert!(parsed.synced);
        let texts: Vec<&str> = parsed.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["first", "second", "third"]);
    }

    /// One line repeated at several times is how LRC writes a chorus.
    #[test]
    fn lrc_repeats_a_line_for_each_timestamp() {
        let parsed = Lyrics::parse_lrc("[00:10.00][01:00.00]chorus");
        assert_eq!(parsed.lines.len(), 2);
        assert!(parsed.lines.iter().all(|l| l.text == "chorus"));
        assert_eq!(parsed.lines[1].at, Duration::from_secs(60));
    }

    #[test]
    fn lyrics_without_timestamps_are_unsynced() {
        let parsed = Lyrics::parse_lrc("just some words\nand more words\n");
        assert!(!parsed.synced);
        assert_eq!(parsed.lines.len(), 2);
        // Unsynced lyrics must not drive the scrolling highlight.
        assert_eq!(parsed.active_at(Duration::from_secs(30)), None);
    }

    #[test]
    fn nothing_is_active_before_the_first_timestamp() {
        let l = lyrics(&[5_000, 10_000]);
        assert_eq!(l.active_at(Duration::from_secs(1)), None);
    }

    #[test]
    fn the_active_line_advances_with_playback() {
        let l = lyrics(&[0, 5_000, 10_000, 15_000]);
        assert_eq!(l.active_at(Duration::from_secs(0)), Some(0));
        assert_eq!(l.active_at(Duration::from_secs(7)), Some(1));
        assert_eq!(l.active_at(Duration::from_secs(10)), Some(2), "exact hit");
        assert_eq!(
            l.active_at(Duration::from_secs(999)),
            Some(3),
            "past the end"
        );
    }

    #[test]
    fn unsynced_lyrics_have_no_active_line() {
        let mut l = lyrics(&[0, 5_000]);
        l.synced = false;
        assert_eq!(l.active_at(Duration::from_secs(3)), None);
    }

    #[test]
    fn empty_lyrics_are_safe_to_query() {
        let l = Lyrics::default();
        assert_eq!(l.active_at(Duration::from_secs(3)), None);
        assert_eq!(l.line_progress(Duration::from_secs(3)), 0.0);
        assert!(l.is_empty());
    }

    #[test]
    fn line_progress_spans_zero_to_one_across_a_line() {
        let l = lyrics(&[0, 10_000]);
        assert!((l.line_progress(Duration::from_secs(0)) - 0.0).abs() < 1e-6);
        assert!((l.line_progress(Duration::from_secs(5)) - 0.5).abs() < 1e-6);
        // Clamped, never overshooting into the next line.
        assert!((l.line_progress(Duration::from_secs(9)) - 0.9).abs() < 1e-6);
    }

    #[test]
    fn the_final_line_still_reports_progress() {
        let l = lyrics(&[0]);
        let progress = l.line_progress(Duration::from_secs(2));
        assert!(progress > 0.0 && progress <= 1.0, "got {progress}");
    }

    #[test]
    fn out_of_order_timestamps_are_sorted() {
        use crate::subsonic::models::{LyricLine, StructuredLyrics};
        let structured = StructuredLyrics {
            synced: true,
            lang: None,
            display_artist: None,
            display_title: None,
            line: vec![
                LyricLine {
                    start: Some(9_000),
                    value: "third".into(),
                },
                LyricLine {
                    start: Some(1_000),
                    value: "first".into(),
                },
                LyricLine {
                    start: Some(5_000),
                    value: "second".into(),
                },
            ],
        };
        let parsed = Lyrics::from_structured(vec![structured]);
        let times: Vec<u64> = parsed
            .lines
            .iter()
            .map(|l| l.at.as_millis() as u64)
            .collect();
        assert_eq!(
            times,
            vec![1_000, 5_000, 9_000],
            "must ascend for binary search"
        );
        // And lookups therefore land on the right line.
        assert_eq!(parsed.active_at(Duration::from_secs(6)), Some(1));
    }

    #[test]
    fn synced_lyrics_are_preferred_over_unsynced() {
        use crate::subsonic::models::{LyricLine, StructuredLyrics};
        let plain = StructuredLyrics {
            synced: false,
            lang: Some("xxx".into()),
            display_artist: None,
            display_title: None,
            line: vec![LyricLine {
                start: None,
                value: "a".into(),
            }],
        };
        let timed = StructuredLyrics {
            synced: true,
            lang: Some("en".into()),
            display_artist: None,
            display_title: None,
            line: vec![LyricLine {
                start: Some(0),
                value: "b".into(),
            }],
        };
        assert!(Lyrics::from_structured(vec![plain, timed]).synced);
    }

    #[test]
    fn every_variant_the_server_offers_is_kept() {
        use crate::subsonic::models::{LyricLine, StructuredLyrics};
        fn variant(lang: &str, synced: bool) -> StructuredLyrics {
            StructuredLyrics {
                synced,
                lang: Some(lang.into()),
                display_artist: None,
                display_title: None,
                line: vec![LyricLine {
                    start: Some(0),
                    value: lang.into(),
                }],
            }
        }

        let set = Lyrics::from_structured(vec![
            variant("ja", false),
            variant("ja-Latn", true),
            variant("en", true),
        ]);
        assert_eq!(set.variants.len(), 3, "a romanisation must not be dropped");
        // Synced first, since that is what the scrolling view needs.
        assert!(set.active().synced, "should open on a synced variant");
        assert!(set.has_variants());

        // And cycling reaches all of them, then wraps.
        let mut seen = vec![set.label()];
        let mut set = set;
        for _ in 0..2 {
            seen.push(set.cycle().expect("more than one variant"));
        }
        seen.sort();
        assert_eq!(seen, ["en", "ja", "ja-Latn"]);
        set.cycle();
        assert_eq!(set.active, 0, "cycling should wrap");
    }

    #[test]
    fn a_single_variant_has_nothing_to_cycle_to() {
        let mut set = LyricSet::from(Lyrics::parse_lrc("[00:01.00]only"));
        assert_eq!(set.variants.len(), 1);
        assert!(!set.has_variants());
        assert_eq!(set.cycle(), None);
        assert_eq!(set.active, 0);
    }

    #[test]
    fn empty_lyrics_become_an_empty_set_rather_than_a_blank_variant() {
        let set = LyricSet::from(Lyrics::default());
        assert!(set.variants.is_empty());
        assert!(set.is_empty(), "reads through to the active variant");
        assert_eq!(Lyrics::from_structured(Vec::new()).variants.len(), 0);
    }

    #[test]
    fn a_translation_is_appended_and_becomes_the_active_variant() {
        let mut set = LyricSet::from(Lyrics::parse_lrc("[00:01.00]hola"));
        set.push_active(Lyrics {
            synced: true,
            lines: vec![Line {
                at: Duration::from_secs(1),
                text: "hello".into(),
            }],
            lang: Some("en (machine)".into()),
        });
        assert_eq!(set.active, 1);
        assert_eq!(set.active().lines[0].text, "hello");
        assert_eq!(set.label(), "en (machine)");
    }

    /// Cache files written before variants existed hold a bare `Lyrics`.
    #[test]
    fn cache_entries_in_the_old_format_still_load() {
        let old = r#"{"synced":true,"lines":[{"at":{"secs":1,"nanos":0},"text":"hi"}]}"#;
        let set = parse_cached(old).expect("old entries must survive the upgrade");
        assert_eq!(set.variants.len(), 1);
        assert_eq!(set.active().lines[0].text, "hi");

        // Including the negative results that stop us re-fetching every play.
        let empty = parse_cached(r#"{"synced":false,"lines":[]}"#).expect("negative result");
        assert!(empty.is_empty());
        assert!(parse_cached("not json").is_none());
    }

    #[test]
    fn a_set_written_by_this_version_round_trips() {
        let mut set = LyricSet::from(Lyrics::parse_lrc("[00:01.00]hola"));
        set.push_active(Lyrics {
            synced: true,
            lines: vec![Line {
                at: Duration::from_secs(1),
                text: "hello".into(),
            }],
            lang: Some("en (machine)".into()),
        });
        let raw = serde_json::to_string(&set).unwrap();
        let back = parse_cached(&raw).expect("round trip");
        assert_eq!(back.variants.len(), 2);
        assert_eq!(back.active, 1, "the chosen variant is remembered");
    }

    #[test]
    fn cache_paths_are_flat_and_sanitised() {
        let cache = LyricsCache {
            dir: PathBuf::from("/tmp/wander-lyrics"),
        };
        let path = cache.path_for("../../etc/passwd");
        assert_eq!(path.parent().unwrap(), PathBuf::from("/tmp/wander-lyrics"));
        assert!(!path.to_string_lossy().contains(".."));
    }
}
