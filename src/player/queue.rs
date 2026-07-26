use crate::subsonic::models::Song;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Off,
    /// Wrap around to the start when the last track finishes.
    All,
    /// Replay the current track indefinitely.
    One,
}

impl Repeat {
    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }
}

/// The play queue.
///
/// Shuffle is modelled as a separate play *order* over the same songs rather
/// than by permuting `songs`, so toggling shuffle off restores the original
/// order and the on-screen queue never reorders under the user.
#[derive(Debug, Default)]
pub struct Queue {
    songs: Vec<Song>,
    order: Vec<usize>,
    /// Position within `order`, not within `songs`.
    position: Option<usize>,
    pub repeat: Repeat,
    pub shuffle: bool,
    /// Radio mode: keep the queue topped up with tracks similar to what is
    /// playing, so playback never simply runs out.
    pub radio: bool,
}

impl Queue {
    pub fn songs(&self) -> &[Song] {
        &self.songs
    }

    pub fn len(&self) -> usize {
        self.songs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.songs.is_empty()
    }

    /// How many tracks remain after the current one.
    ///
    /// Radio mode uses this to decide when to fetch more.
    pub fn remaining(&self) -> usize {
        match self.position {
            Some(position) => self.order.len().saturating_sub(position + 1),
            None => self.order.len(),
        }
    }

    /// IDs of every song in the queue, for excluding recent plays.
    pub fn ids(&self) -> std::collections::HashSet<String> {
        self.songs.iter().map(|song| song.id.clone()).collect()
    }

    /// The last `count` tracks in play order up to and including the current
    /// one, oldest first. Radio mode uses this to avoid repeating artists.
    pub fn recent(&self, count: usize) -> Vec<Song> {
        let end = self.position.map_or(self.order.len(), |p| p + 1);
        let start = end.saturating_sub(count);
        self.order[start..end]
            .iter()
            .map(|&i| self.songs[i].clone())
            .collect()
    }

    /// Index into `songs` of the currently playing track.
    pub fn current_index(&self) -> Option<usize> {
        self.position.map(|p| self.order[p])
    }

    pub fn current(&self) -> Option<&Song> {
        self.current_index().map(|i| &self.songs[i])
    }

    /// The track that would play next, without advancing.
    ///
    /// Used to warm the cover cache ahead of the track change.
    pub fn peek_next(&self) -> Option<&Song> {
        let position = self.position?;
        let next = match self.repeat {
            Repeat::One => position,
            _ if position + 1 < self.order.len() => position + 1,
            Repeat::All => 0,
            _ => return None,
        };
        self.order.get(next).map(|&i| &self.songs[i])
    }

    pub fn restore(&mut self, songs: Vec<Song>, index: usize) {
        self.clear();
        let len = songs.len();
        self.songs = songs;
        self.order = (0..len).collect();
        if len > 0 {
            self.position = Some(index.min(len - 1));
        }
    }

    pub fn clear(&mut self) {
        self.songs.clear();
        self.order.clear();
        self.position = None;
    }

    pub fn extend(&mut self, songs: impl IntoIterator<Item = Song>) {
        let before = self.songs.len();
        self.songs.extend(songs);
        // Append the new indices to the play order. When shuffling, splice them
        // into the remaining (unplayed) portion so freshly added tracks don't
        // all queue up at the end in order.
        let new: Vec<usize> = (before..self.songs.len()).collect();
        if self.shuffle {
            let tail_start = self.position.map_or(0, |p| p + 1);
            for index in new {
                let at = if tail_start >= self.order.len() {
                    self.order.len()
                } else {
                    rand::random_range(tail_start..=self.order.len())
                };
                self.order.insert(at, index);
            }
        } else {
            self.order.extend(new);
        }
    }

    /// Append `songs` but play them immediately after the current track.
    ///
    /// Unlike [`Queue::extend`], which lets radio mode fill the tail, this is
    /// the explicit "play next" the user asked for, so order is preserved.
    pub fn insert_next(&mut self, songs: impl IntoIterator<Item = Song>) {
        let before = self.songs.len();
        self.songs.extend(songs);
        let new: Vec<usize> = (before..self.songs.len()).collect();
        let at = self.position.map_or(0, |p| p + 1).min(self.order.len());
        for (offset, index) in new.into_iter().enumerate() {
            self.order.insert(at + offset, index);
        }
    }

    /// Move the song at `song_index` one slot up or down in the play order.
    ///
    /// Returns the index it ended up at, so the caller can follow it with the
    /// selection.
    pub fn move_song(&mut self, song_index: usize, delta: isize) -> usize {
        let len = self.songs.len();
        if len < 2 || song_index >= len {
            return song_index;
        }
        let target = (song_index as isize + delta).clamp(0, len as isize - 1) as usize;
        if target == song_index {
            return song_index;
        }
        let current = self.current_index();
        let song = self.songs.remove(song_index);
        self.songs.insert(target, song);

        // Where every song ended up, so both the play order and the playhead
        // can be rewritten without guessing.
        let (lo, hi) = (song_index.min(target), song_index.max(target));
        let moved_to = |i: usize| -> usize {
            if i == song_index {
                target
            } else if i >= lo && i <= hi {
                if song_index < target { i - 1 } else { i + 1 }
            } else {
                i
            }
        };

        if self.shuffle {
            // Shuffle already decouples the visible list from the play order,
            // so preserve the shuffled sequence and just follow the indices.
            for i in self.order.iter_mut() {
                *i = moved_to(*i);
            }
        } else {
            // Unshuffled, the list *is* the play order — which is the whole
            // point of dragging a track up: it should now play sooner.
            self.order = (0..len).collect();
        }

        // Playback follows the song, not the slot it used to sit in.
        if let Some(current) = current {
            let now_at = moved_to(current);
            self.position = self.order.iter().position(|&i| i == now_at);
        }
        target
    }

    pub fn remove(&mut self, song_index: usize) {
        if song_index >= self.songs.len() {
            return;
        }
        let current = self.current_index();
        self.songs.remove(song_index);
        self.order.retain(|&i| i != song_index);
        // Indices above the removed one all shift down by one.
        for i in self.order.iter_mut() {
            if *i > song_index {
                *i -= 1;
            }
        }
        // Keep playing whatever was playing, unless it was the removed track.
        self.position = match current {
            _ if self.order.is_empty() => None,
            Some(c) if c == song_index => self.position.map(|p| p.min(self.order.len() - 1)),
            Some(c) => {
                let adjusted = if c > song_index { c - 1 } else { c };
                self.order.iter().position(|&i| i == adjusted)
            }
            None => None,
        };
    }

    /// Start playing the song at `song_index` (an index into `songs`).
    pub fn play_at(&mut self, song_index: usize) {
        if let Some(p) = self.order.iter().position(|&i| i == song_index) {
            self.position = Some(p);
        }
    }

    /// Advance to the next track, honouring repeat mode.
    ///
    /// `Repeat::One` is deliberately *not* handled here: it applies when a
    /// track ends on its own, not when the user presses next. Use
    /// [`Queue::advance_after_track_end`] for the end-of-track case.
    pub fn next(&mut self) -> Option<&Song> {
        if self.order.is_empty() {
            return None;
        }
        let p = match self.position {
            None => 0,
            Some(p) if p + 1 < self.order.len() => p + 1,
            Some(p) if self.repeat == Repeat::All => {
                let _ = p;
                if self.shuffle {
                    self.reshuffle_preserving_nothing();
                }
                0
            }
            Some(_) => {
                self.position = None;
                return None;
            }
        };
        self.position = Some(p);
        self.current()
    }

    pub fn prev(&mut self) -> Option<&Song> {
        if self.order.is_empty() {
            return None;
        }
        let p = match self.position {
            None => 0,
            Some(0) if self.repeat == Repeat::All => self.order.len() - 1,
            Some(0) => 0,
            Some(p) => p - 1,
        };
        self.position = Some(p);
        self.current()
    }

    /// What to play when the current track ends naturally.
    pub fn advance_after_track_end(&mut self) -> Option<&Song> {
        if self.repeat == Repeat::One {
            return self.current();
        }
        self.next()
    }

    pub fn toggle_shuffle(&mut self) {
        self.shuffle = !self.shuffle;
        let current = self.current_index();
        if self.shuffle {
            self.reshuffle_preserving_current(current);
        } else {
            self.order = (0..self.songs.len()).collect();
            self.position = current;
        }
    }

    pub fn toggle_repeat(&mut self) {
        self.repeat = self.repeat.cycle();
    }

    pub fn set_repeat(&mut self, repeat: Repeat) {
        self.repeat = repeat;
    }

    /// Shuffle the order, keeping the current track at the front so playback
    /// continues uninterrupted.
    fn reshuffle_preserving_current(&mut self, current: Option<usize>) {
        let mut rest: Vec<usize> = (0..self.songs.len())
            .filter(|i| Some(*i) != current)
            .collect();
        fisher_yates(&mut rest);
        self.order = match current {
            Some(c) => {
                let mut order = Vec::with_capacity(self.songs.len());
                order.push(c);
                order.extend(rest);
                self.position = Some(0);
                order
            }
            None => {
                self.position = None;
                rest
            }
        };
    }

    fn reshuffle_preserving_nothing(&mut self) {
        self.order = (0..self.songs.len()).collect();
        fisher_yates(&mut self.order);
    }
}

fn fisher_yates<T>(items: &mut [T]) {
    for i in (1..items.len()).rev() {
        items.swap(i, rand::random_range(0..=i));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str) -> Song {
        Song {
            id: id.to_string(),
            title: id.to_string(),
            album: None,
            album_id: None,
            artist: None,
            artist_id: None,
            cover_art: None,
            duration: 100,
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

    fn queue_of(n: usize) -> Queue {
        let mut q = Queue::default();
        q.extend((0..n).map(|i| song(&i.to_string())));
        q
    }

    #[test]
    fn insert_next_puts_tracks_immediately_after_the_current_one() {
        let mut q = queue_of(3);
        q.play_at(0);
        q.insert_next([song("x"), song("y")]);

        assert_eq!(q.current().unwrap().id, "0", "playback is undisturbed");
        assert_eq!(q.next().unwrap().id, "x");
        assert_eq!(q.next().unwrap().id, "y");
        assert_eq!(q.next().unwrap().id, "1", "the old tail follows");
    }

    #[test]
    fn insert_next_into_an_idle_queue_puts_tracks_at_the_front() {
        let mut q = queue_of(2);
        q.insert_next([song("x")]);
        assert_eq!(q.next().unwrap().id, "x");
    }

    #[test]
    fn moving_a_track_reorders_it_and_playback_follows_the_song() {
        let mut q = queue_of(4);
        q.play_at(2);
        assert_eq!(q.current().unwrap().id, "2");

        assert_eq!(q.move_song(2, -1), 1);
        assert_eq!(
            q.songs().iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["0", "2", "1", "3"]
        );
        assert_eq!(q.current().unwrap().id, "2", "still playing the same song");
        assert_eq!(q.next().unwrap().id, "1");
    }

    #[test]
    fn moving_a_track_while_shuffling_keeps_the_shuffled_sequence() {
        let mut q = queue_of(4);
        q.shuffle = true;
        q.order = vec![3, 1, 0, 2];
        q.position = Some(0);

        q.move_song(0, 1);
        assert_eq!(q.current().unwrap().id, "3", "playback is undisturbed");
        // The shuffled play order still visits the same songs in the same
        // sequence, only the visible list changed.
        let sequence: Vec<String> = q.order.iter().map(|&i| q.songs[i].id.clone()).collect();
        assert_eq!(sequence, vec!["3", "1", "0", "2"]);
    }

    #[test]
    fn moving_a_track_past_the_ends_is_clamped() {
        let mut q = queue_of(3);
        assert_eq!(q.move_song(0, -1), 0, "already at the top");
        assert_eq!(q.move_song(2, 1), 2, "already at the bottom");
        assert_eq!(
            q.songs().iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["0", "1", "2"]
        );
    }

    #[test]
    fn moving_a_track_the_player_is_not_on_leaves_playback_alone() {
        let mut q = queue_of(4);
        q.play_at(0);
        q.move_song(2, 1);
        assert_eq!(q.current().unwrap().id, "0");
    }

    #[test]
    fn recent_returns_the_tail_up_to_the_current_track() {
        let mut q = queue_of(5);
        q.play_at(3);
        let recent: Vec<String> = q.recent(3).into_iter().map(|s| s.id).collect();
        assert_eq!(recent, vec!["1", "2", "3"], "oldest first, current last");
    }

    #[test]
    fn recent_on_an_unstarted_queue_does_not_panic() {
        let q = queue_of(2);
        assert_eq!(q.recent(10).len(), 2);
        assert!(Queue::default().recent(5).is_empty());
    }

    #[test]
    fn advances_through_tracks_in_order() {
        let mut q = queue_of(3);
        assert_eq!(q.next().unwrap().id, "0");
        assert_eq!(q.next().unwrap().id, "1");
        assert_eq!(q.next().unwrap().id, "2");
    }

    #[test]
    fn stops_at_the_end_when_repeat_is_off() {
        let mut q = queue_of(2);
        q.next();
        q.next();
        assert!(q.next().is_none(), "should stop past the last track");
        assert!(q.current().is_none());
    }

    #[test]
    fn wraps_to_start_when_repeat_all() {
        let mut q = queue_of(2);
        q.repeat = Repeat::All;
        q.next();
        q.next();
        assert_eq!(q.next().unwrap().id, "0", "should wrap around");
    }

    #[test]
    fn repeat_one_replays_current_track_only_on_natural_end() {
        let mut q = queue_of(3);
        q.repeat = Repeat::One;
        q.next();
        assert_eq!(q.advance_after_track_end().unwrap().id, "0");
        // An explicit skip must still move on, even under repeat-one.
        assert_eq!(q.next().unwrap().id, "1");
    }

    #[test]
    fn prev_wraps_only_under_repeat_all() {
        let mut q = queue_of(3);
        q.next();
        assert_eq!(q.prev().unwrap().id, "0", "clamps at the first track");
        q.repeat = Repeat::All;
        assert_eq!(q.prev().unwrap().id, "2", "wraps to the last track");
    }

    #[test]
    fn toggling_shuffle_keeps_the_current_track_playing() {
        let mut q = queue_of(10);
        q.play_at(4);
        q.toggle_shuffle();
        assert_eq!(q.current().unwrap().id, "4");
        q.toggle_shuffle();
        assert_eq!(q.current().unwrap().id, "4", "and restores original order");
        assert_eq!(q.current_index(), Some(4));
    }

    #[test]
    fn shuffle_order_covers_every_track_exactly_once() {
        let mut q = queue_of(20);
        q.toggle_shuffle();
        let mut seen = q.order.clone();
        seen.sort_unstable();
        assert_eq!(seen, (0..20).collect::<Vec<_>>());
    }

    #[test]
    fn removing_a_track_keeps_the_current_one_playing() {
        let mut q = queue_of(4);
        q.play_at(2);
        q.remove(0);
        assert_eq!(q.current().unwrap().id, "2", "still the same song");
        assert_eq!(q.len(), 3);
    }

    #[test]
    fn removing_the_last_remaining_track_clears_playback() {
        let mut q = queue_of(1);
        q.play_at(0);
        q.remove(0);
        assert!(q.is_empty());
        assert!(q.current().is_none());
    }

    #[test]
    fn extend_appends_without_disturbing_playback() {
        let mut q = queue_of(2);
        q.play_at(1);
        q.extend([song("new")]);
        assert_eq!(q.current().unwrap().id, "1");
        assert_eq!(q.next().unwrap().id, "new");
    }
}
