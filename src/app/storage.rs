use super::types::*;
use super::*;
use crate::player::PlayerCommand;
use anyhow::Result;
use std::time::Instant;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct SavedAppState {
    songs: Vec<Song>,
    index: usize,
    volume: f32,
    cover_percent: u16,
    queue_percent: u16,
    #[serde(default = "default_visualiser_height")]
    visualiser_height: u16,
    show_queue_pane: bool,
    #[serde(default = "yes")]
    show_focus_queue: bool,
    show_cover_pane: bool,
    #[serde(default = "yes")]
    show_focus_cover: bool,
    show_lyrics_pane: bool,
    #[serde(default = "yes")]
    show_focus_lyrics: bool,
    show_visualiser: bool,
    #[serde(default, deserialize_with = "crate::ui::visualiser::lenient_mode")]
    viz_mode: crate::ui::visualiser::VizMode,
    #[serde(default)]
    radio: bool,
    /// Position within the current track, so a restart resumes there.
    #[serde(default)]
    elapsed_secs: f64,
    #[serde(default = "default_library_mode")]
    library_mode: LibraryMode,
}

/// Saved states written before focus mode existed should still show lyrics.
pub(crate) fn yes() -> bool {
    true
}

pub(crate) fn default_library_mode() -> LibraryMode {
    LibraryMode::Artists
}

pub(crate) fn default_visualiser_height() -> u16 {
    8
}

pub(crate) fn queue_cache_path() -> Option<std::path::PathBuf> {
    Some(crate::paths::cache_dir()?.join("saved_state.json"))
}

impl App {
    /// repeat.
    pub fn save_queue_state(&mut self) {
        self.state_dirty = true;
    }

    /// Write the session state if it is dirty and the cooldown has elapsed.
    /// `force` skips the cooldown, for quitting.
    pub fn flush_state(&mut self, force: bool) {
        if !self.state_dirty {
            return;
        }
        if !force && self.last_state_save.elapsed() < STATE_SAVE_INTERVAL {
            return;
        }
        self.state_dirty = false;
        self.last_state_save = Instant::now();
        self.write_queue_state();
    }

    pub(crate) fn write_queue_state(&self) {
        let Some(path) = queue_cache_path() else {
            return;
        };
        let (songs, index, radio) = {
            let queue = self.player.queue.lock().unwrap();
            (
                queue.songs().to_vec(),
                queue.current_index().unwrap_or(0),
                queue.radio,
            )
        };
        let state = SavedAppState {
            songs,
            index,
            volume: self.player.shared.volume(),
            cover_percent: self.cover_percent,
            queue_percent: self.queue_percent,
            visualiser_height: self.visualiser_height,
            show_queue_pane: self.show_queue_pane,
            show_focus_queue: self.show_focus_queue,
            show_cover_pane: self.show_cover_pane,
            show_focus_cover: self.show_focus_cover,
            show_lyrics_pane: self.show_lyrics_pane,
            show_focus_lyrics: self.show_focus_lyrics,
            show_visualiser: self.show_visualiser,
            viz_mode: self.viz_mode,
            radio,
            elapsed_secs: self.player.elapsed().as_secs_f64(),
            library_mode: self.library_mode,
        };
        if let Ok(raw) = serde_json::to_string(&state) {
            let _ = std::fs::write(path, raw);
        }
    }

    pub fn load_queue_state(&mut self) {
        let Some(path) = queue_cache_path() else {
            return;
        };
        let Ok(raw) = std::fs::read_to_string(path) else {
            return;
        };
        let Ok(state): Result<SavedAppState, _> = serde_json::from_str(&raw) else {
            return;
        };
        self.cover_percent = state.cover_percent.clamp(10, 80);
        self.queue_percent = state.queue_percent.clamp(10, 80);
        self.visualiser_height = state.visualiser_height.clamp(3, 40);
        self.show_queue_pane = state.show_queue_pane;
        self.show_focus_queue = state.show_focus_queue;
        self.show_cover_pane = state.show_cover_pane;
        self.show_focus_cover = state.show_focus_cover;
        self.show_lyrics_pane = state.show_lyrics_pane;
        self.show_focus_lyrics = state.show_focus_lyrics;
        self.show_visualiser = state.show_visualiser;
        self.viz_mode = state.viz_mode;
        self.library_mode = state.library_mode;
        self.player.send(PlayerCommand::SetVolume(state.volume));
        let restored = {
            let mut queue = self.player.queue.lock().unwrap();
            let restored = !state.songs.is_empty();
            if restored {
                queue.restore(state.songs, state.index);
            }
            queue.radio = state.radio;
            restored
        };
        // Arm the track where it stopped, paused, so play resumes rather than
        // requiring the user to find and select it again.
        if restored {
            self.player.send(PlayerCommand::Resume {
                offset: Duration::from_secs_f64(state.elapsed_secs.max(0.0)),
            });
        }
    }
}
