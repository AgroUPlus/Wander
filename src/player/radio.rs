//! Radio mode: an endless stream of tracks that fit what is playing.
//!
//! Scores candidates against a seed track by genre and mood overlap, artist
//! affinity, release-year proximity, whether the user has starred them, and how
//! often they have been played, plus a small random term so the same "best"
//! tracks do not surface every time.
//!
//! Two rules keep it from degenerating into "play the rest of this album",
//! which is what a pure similarity score does:
//!
//! * a **saturation penalty** that grows with how often an artist or album has
//!   already appeared in the recent stream, and
//! * a **novelty quota** reserving part of every batch for tracks the listener
//!   has never played.

use crate::subsonic::models::Song;
use std::collections::{HashMap, HashSet};

/// Weights, in rough order of how strongly each should pull.
const GENRE_WEIGHT: f32 = 3.0;
const MOOD_WEIGHT: f32 = 2.0;
const STARRED_WEIGHT: f32 = 2.0;
const ARTIST_WEIGHT: f32 = 2.5;
/// Same-album tracks are related, but only mildly worth seeking out.
const ALBUM_WEIGHT: f32 = 0.5;
/// Full value for the same year, fading to nothing a decade out.
const YEAR_WEIGHT: f32 = 1.0;
const YEAR_SPAN: f32 = 10.0;
/// Subtracted per prior appearance of the same artist, and of the same album.
const ARTIST_SATURATION: f32 = 4.0;
const ALBUM_SATURATION: f32 = 3.0;
/// Ceiling on the random term, small enough not to swamp real signal.
const JITTER: f32 = 1.5;
/// Share of each batch reserved for tracks that have never been played.
const NOVELTY_SHARE: f32 = 0.3;

/// What the stream has played or queued lately, so picks can avoid piling onto
/// the same artist or album.
#[derive(Debug, Default)]
pub struct Context {
    /// Lowercased artist name to number of recent appearances.
    pub artists: HashMap<String, u32>,
    /// Album id to number of recent appearances.
    pub albums: HashMap<String, u32>,
    /// Artists the server considers close to the seed's, lowercased.
    pub similar_artists: HashSet<String>,
}

impl Context {
    /// Build from the recent tail of the queue, newest last.
    pub fn from_recent(recent: &[Song]) -> Self {
        let mut context = Self::default();
        for song in recent {
            context.note(song);
        }
        context
    }

    pub fn note(&mut self, song: &Song) {
        *self
            .artists
            .entry(song.artist_or_unknown().to_lowercase())
            .or_default() += 1;
        if let Some(album) = song.album_id.as_ref() {
            *self.albums.entry(album.clone()).or_default() += 1;
        }
    }

    fn saturation(&self, song: &Song) -> f32 {
        let artist = self
            .artists
            .get(&song.artist_or_unknown().to_lowercase())
            .copied()
            .unwrap_or(0) as f32;
        let album = song
            .album_id
            .as_ref()
            .and_then(|id| self.albums.get(id))
            .copied()
            .unwrap_or(0) as f32;
        ARTIST_SATURATION * artist + ALBUM_SATURATION * album
    }
}

/// How well `candidate` fits after `seed`, given what has played recently.
///
/// `jitter` is injected rather than sampled inside so the scoring is
/// deterministic and testable.
pub fn score(seed: &Song, candidate: &Song, context: &Context, jitter: f32) -> f32 {
    let seed_genres: HashSet<String> = seed.genre_names().into_iter().collect();
    let candidate_genres: HashSet<String> = candidate.genre_names().into_iter().collect();
    let genre_overlap = seed_genres.intersection(&candidate_genres).count() as f32;

    let seed_moods: HashSet<String> = seed.moods.iter().map(|m| m.to_lowercase()).collect();
    let candidate_moods: HashSet<String> =
        candidate.moods.iter().map(|m| m.to_lowercase()).collect();
    let mood_overlap = seed_moods.intersection(&candidate_moods).count() as f32;

    let artist = candidate.artist_or_unknown().to_lowercase();
    let same_artist = artist == seed.artist_or_unknown().to_lowercase();
    // A server-suggested neighbour counts as much as the seed's own artist,
    // without the pull towards replaying the same discography.
    let artist_affinity = if same_artist || context.similar_artists.contains(&artist) {
        1.0
    } else {
        0.0
    };
    let same_album = candidate.album_id.is_some() && candidate.album_id == seed.album_id;

    let year_affinity = match (seed.year, candidate.year) {
        (Some(a), Some(b)) => (1.0 - (a as f32 - b as f32).abs() / YEAR_SPAN).max(0.0),
        _ => 0.0,
    };

    let starred = if candidate.is_starred() { 1.0 } else { 0.0 };
    // Diminishing returns: a track played 100 times is not 10x better than one
    // played 10 times.
    let popularity = (1.0 + candidate.play_count as f32).ln();

    GENRE_WEIGHT * genre_overlap
        + MOOD_WEIGHT * mood_overlap
        + STARRED_WEIGHT * starred
        + ARTIST_WEIGHT * artist_affinity
        + if same_album { ALBUM_WEIGHT } else { 0.0 }
        + YEAR_WEIGHT * year_affinity
        + popularity
        + jitter
        - context.saturation(candidate)
}

/// Choose the best `count` candidates for a seed track.
///
/// Anything already queued is excluded so radio mode does not loop back onto
/// tracks the listener just heard. Part of the batch is reserved for unplayed
/// tracks, so the stream keeps introducing new music instead of settling into
/// the listener's existing favourites.
pub fn pick(
    seed: &Song,
    candidates: Vec<Song>,
    exclude: &HashSet<String>,
    context: &Context,
    count: usize,
) -> Vec<Song> {
    // De-duplicate: candidate pools are unioned from several endpoints and
    // routinely overlap.
    let mut seen: HashSet<String> = HashSet::new();
    let mut scored: Vec<(f32, Song)> = candidates
        .into_iter()
        .filter(|song| {
            song.id != seed.id && !exclude.contains(&song.id) && seen.insert(song.id.clone())
        })
        .map(|song| {
            let jitter = rand::random_range(0.0..JITTER);
            (score(seed, &song, context, jitter), song)
        })
        .collect();

    // Highest score first; NaN cannot occur since every term is finite.
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let novelty_target = (count as f32 * NOVELTY_SHARE).round() as usize;
    let mut picked: Vec<Song> = Vec::with_capacity(count);
    // Track the running artist/album mix so one artist cannot dominate the
    // batch even when every one of their tracks scores well.
    let mut running = Context {
        artists: context.artists.clone(),
        albums: context.albums.clone(),
        similar_artists: HashSet::new(),
    };

    // Fill the novelty quota first, then the rest by score. Rescoring against
    // `running` after each pick is what spreads artists across the batch.
    for novelty_pass in [true, false] {
        while picked.len() < count {
            let best = scored
                .iter()
                .enumerate()
                .filter(|(_, (_, song))| !novelty_pass || song.play_count == 0)
                .map(|(index, (_, song))| (index, running.saturation(song)))
                .min_by(|a, b| {
                    // Among equally-saturated candidates, keep the original
                    // (score-sorted) order.
                    a.1.partial_cmp(&b.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(a.0.cmp(&b.0))
                });
            let Some((index, _)) = best else { break };
            let (_, song) = scored.remove(index);
            running.note(&song);
            picked.push(song);
            if novelty_pass && picked.len() >= novelty_target {
                break;
            }
        }
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::models::ItemGenre;

    fn song(id: &str) -> Song {
        Song {
            id: id.into(),
            title: id.into(),
            album: None,
            album_id: None,
            artist: None,
            artist_id: None,
            cover_art: None,
            duration: 200,
            bit_rate: 0,
            track: None,
            year: None,
            genre: None,
            suffix: None,
            content_type: None,
            size: 0,
            starred: None,
            user_rating: None,
            play_count: 0,
            genres: Vec::new(),
            moods: Vec::new(),
        }
    }

    fn with_genre(id: &str, genre: &str) -> Song {
        let mut s = song(id);
        s.genres = vec![ItemGenre { name: genre.into() }];
        s
    }

    /// A realistic pool (many artists, all previously played, so the novelty
    /// quota cannot be met) must still yield a full batch rather than a short
    /// one — a short batch is what makes radio look like it has stalled.
    #[test]
    fn a_fully_played_pool_still_fills_a_batch() {
        let seed = by("seed", "Seed Artist", "seed-album");
        let mut candidates = Vec::new();
        for i in 0..100 {
            let mut s = by(
                &format!("c{i}"),
                &format!("Artist {}", i % 20),
                &format!("al{}", i % 30),
            );
            s.play_count = 3;
            candidates.push(s);
        }
        let picked = pick(&seed, candidates, &HashSet::new(), &Context::default(), 15);
        assert_eq!(picked.len(), 15, "radio should fill a full batch");
    }

    /// Saturation penalties must throttle a one-artist pool, not empty it.
    #[test]
    fn a_single_artist_pool_still_fills_a_batch() {
        let seed = by("seed", "A", "al");
        let candidates: Vec<Song> = (0..50).map(|i| by(&format!("c{i}"), "A", "al")).collect();
        let picked = pick(&seed, candidates, &HashSet::new(), &Context::default(), 15);
        assert_eq!(picked.len(), 15, "one-artist pool should still fill");
    }

    fn by(id: &str, artist: &str, album: &str) -> Song {
        let mut s = with_genre(id, "Rock");
        s.artist = Some(artist.into());
        s.album_id = Some(album.into());
        s
    }

    fn ctx() -> Context {
        Context::default()
    }

    #[test]
    fn shared_genre_scores_above_unrelated() {
        let seed = with_genre("seed", "J-Pop");
        let match_ = with_genre("a", "J-Pop");
        let other = with_genre("b", "Gaming");
        assert!(score(&seed, &match_, &ctx(), 0.0) > score(&seed, &other, &ctx(), 0.0));
    }

    #[test]
    fn genre_matching_is_case_insensitive() {
        let seed = with_genre("seed", "J-Pop");
        let lower = with_genre("a", "j-pop");
        assert!(score(&seed, &lower, &ctx(), 0.0) >= GENRE_WEIGHT);
    }

    #[test]
    fn starred_tracks_outrank_equivalent_unstarred_ones() {
        let seed = with_genre("seed", "Rock");
        let plain = with_genre("a", "Rock");
        let mut starred = with_genre("b", "Rock");
        starred.starred = Some("2026-01-01T00:00:00Z".into());
        assert!(score(&seed, &starred, &ctx(), 0.0) > score(&seed, &plain, &ctx(), 0.0));
    }

    #[test]
    fn play_count_helps_but_with_diminishing_returns() {
        let seed = song("seed");
        let mut once = song("a");
        once.play_count = 1;
        let mut lots = song("b");
        lots.play_count = 100;

        let gain_first = score(&seed, &once, &ctx(), 0.0) - score(&seed, &song("c"), &ctx(), 0.0);
        let gain_rest = score(&seed, &lots, &ctx(), 0.0) - score(&seed, &once, &ctx(), 0.0);
        assert!(gain_first > 0.0);
        // 99 extra plays must not be worth ~99x the first one.
        assert!(gain_rest < gain_first * 10.0, "popularity should saturate");
    }

    #[test]
    fn moods_contribute_to_the_score() {
        let mut seed = song("seed");
        seed.moods = vec!["calm".into()];
        let mut matching = song("a");
        matching.moods = vec!["Calm".into()];
        assert!(score(&seed, &matching, &ctx(), 0.0) >= MOOD_WEIGHT);
    }

    #[test]
    fn a_server_suggested_artist_scores_like_the_seeds_own() {
        let seed = by("seed", "Alpha", "alb1");
        let neighbour = by("a", "Beta", "alb2");
        let stranger = by("b", "Gamma", "alb3");

        let mut context = Context::default();
        context.similar_artists.insert("beta".to_string());
        assert!(score(&seed, &neighbour, &context, 0.0) > score(&seed, &stranger, &context, 0.0));
    }

    #[test]
    fn nearby_release_years_score_above_distant_ones() {
        let mut seed = with_genre("seed", "Rock");
        seed.year = Some(2000);
        let mut close = with_genre("a", "Rock");
        close.year = Some(2001);
        let mut distant = with_genre("b", "Rock");
        distant.year = Some(1970);
        assert!(score(&seed, &close, &ctx(), 0.0) > score(&seed, &distant, &ctx(), 0.0));
    }

    #[test]
    fn an_over_represented_artist_is_penalised() {
        let seed = by("seed", "Alpha", "alb1");
        let repeat = by("a", "Alpha", "alb1");
        let fresh = by("b", "Delta", "alb9");

        // Three of the last tracks were already this artist and album.
        let recent = vec![by("r1", "Alpha", "alb1"), by("r2", "Alpha", "alb1")];
        let context = Context::from_recent(&recent);
        assert!(
            score(&seed, &fresh, &context, 0.0) > score(&seed, &repeat, &context, 0.0),
            "saturation must outweigh same-artist affinity"
        );
    }

    #[test]
    fn pick_excludes_the_seed_and_anything_queued() {
        let seed = with_genre("seed", "Rock");
        let candidates = vec![
            seed.clone(),
            with_genre("queued", "Rock"),
            with_genre("fresh", "Rock"),
        ];
        let exclude: HashSet<String> = ["queued".to_string()].into_iter().collect();

        let picked = pick(&seed, candidates, &exclude, &ctx(), 10);
        let ids: Vec<&str> = picked.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["fresh"]);
    }

    #[test]
    fn pick_drops_duplicate_ids_from_overlapping_pools() {
        let seed = with_genre("seed", "Rock");
        let candidates = vec![with_genre("a", "Rock"), with_genre("a", "Rock")];
        assert_eq!(
            pick(&seed, candidates, &HashSet::new(), &ctx(), 10).len(),
            1
        );
    }

    #[test]
    fn pick_respects_the_requested_count() {
        let seed = with_genre("seed", "Rock");
        let candidates: Vec<Song> = (0..20)
            .map(|i| by(&format!("s{i}"), &format!("artist{i}"), &format!("alb{i}")))
            .collect();
        assert_eq!(pick(&seed, candidates, &HashSet::new(), &ctx(), 5).len(), 5);
    }

    #[test]
    fn pick_spreads_a_batch_across_artists_instead_of_dumping_one_album() {
        let seed = by("seed", "Alpha", "alb1");
        let mut candidates: Vec<Song> = (0..10)
            .map(|i| {
                let mut s = by(&format!("same{i}"), "Alpha", "alb1");
                s.play_count = 5;
                s
            })
            .collect();
        candidates.extend((0..10).map(|i| {
            let mut s = by(
                &format!("other{i}"),
                &format!("Artist{i}"),
                &format!("alb{i}"),
            );
            s.play_count = 5;
            s
        }));

        let picked = pick(&seed, candidates, &HashSet::new(), &ctx(), 6);
        let from_seed_artist = picked
            .iter()
            .filter(|s| s.artist.as_deref() == Some("Alpha"))
            .count();
        assert!(
            from_seed_artist <= 2,
            "one album should not fill the batch, got {from_seed_artist} of 6"
        );
    }

    #[test]
    fn pick_reserves_part_of_the_batch_for_unplayed_tracks() {
        let seed = with_genre("seed", "Rock");
        // Played tracks score higher, so without a quota they would take all 10.
        let mut candidates: Vec<Song> = (0..20)
            .map(|i| {
                let mut s = by(&format!("played{i}"), &format!("A{i}"), &format!("alb{i}"));
                s.play_count = 50;
                s
            })
            .collect();
        candidates
            .extend((0..20).map(|i| by(&format!("new{i}"), &format!("B{i}"), &format!("nalb{i}"))));

        let picked = pick(&seed, candidates, &HashSet::new(), &ctx(), 10);
        let unplayed = picked.iter().filter(|s| s.play_count == 0).count();
        assert!(
            unplayed >= 3,
            "expected a novelty quota, got {unplayed} of 10"
        );
    }

    #[test]
    fn pick_on_empty_candidates_is_empty() {
        let seed = song("seed");
        assert!(pick(&seed, Vec::new(), &HashSet::new(), &ctx(), 5).is_empty());
    }
}
