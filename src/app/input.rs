use super::layout::{copy_to_clipboard, open_url};
use super::types::*;
use super::*;
use crate::keymap::Action;
use crate::ui::{Hits, Region};
use crossterm::event::{KeyEvent, MouseEvent};

impl App {
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        if self.show_help {
            // Any key dismisses help, so it never traps the user.
            self.show_help = false;
            return;
        }

        // A popup owns the keyboard while it is open, so its text fields are
        // not also firing global single-letter bindings.
        if self.handle_overlay_key(key) {
            return;
        }

        // An open settings field owns the keyboard for the same reason: typing
        // a server URL must not trigger the single-letter playback bindings.
        if self.settings_edit.is_some() && self.handle_settings_edit_key(key) {
            return;
        }

        #[cfg(feature = "nyaa")]
        if self.nyaa_plugin.editing_query && self.handle_nyaa_query_key(key) {
            return;
        }
        if self.archive_plugin.editing_query && self.handle_archive_query_key(key) {
            return;
        }
        if self.jamendo_plugin.editing_query && self.handle_jamendo_query_key(key) {
            return;
        }

        if self.tab == Tab::Online {
            use crossterm::event::{KeyCode, KeyModifiers};
            let no_mods = key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT;
            // Switching sources is shared; everything else belongs to whichever
            // plugin currently owns the tab.
            if key.code == KeyCode::Char('o') && no_mods {
                self.cycle_online_source();
                return;
            }
            match self.online_source {
                #[cfg(feature = "nyaa")]
                OnlineSource::Nyaa => {
                    if key.code == KeyCode::Char('/') && no_mods {
                        self.nyaa_plugin.editing_query = true;
                        return;
                    }
                    if key.code == KeyCode::Char('c') && no_mods {
                        self.cycle_nyaa_category();
                        return;
                    }
                    if key.code == KeyCode::Char('d') && key.modifiers.is_empty() {
                        self.download_selected_nyaa_item();
                        return;
                    }
                    if (key.code == KeyCode::Char('s') || key.code == KeyCode::Char('p'))
                        && key.modifiers.is_empty()
                    {
                        self.stream_selected_nyaa_item();
                        return;
                    }
                }
                OnlineSource::Jamendo => {
                    if key.code == KeyCode::Char('/') && no_mods {
                        self.jamendo_plugin.editing_query = true;
                        return;
                    }
                    if key.code == KeyCode::Char('c') && no_mods {
                        self.cycle_jamendo_format();
                        return;
                    }
                    if key.code == KeyCode::Char('d') && key.modifiers.is_empty() {
                        self.download_selected_jamendo_track();
                        return;
                    }
                    if (key.code == KeyCode::Char('s') || key.code == KeyCode::Char('p'))
                        && key.modifiers.is_empty()
                    {
                        self.stream_selected_jamendo_track();
                        return;
                    }
                }
                OnlineSource::Archive => {
                    if key.code == KeyCode::Char('/') && no_mods {
                        self.archive_plugin.editing_query = true;
                        return;
                    }
                    if key.code == KeyCode::Char('c') && no_mods {
                        self.cycle_archive_collection();
                        return;
                    }
                    if key.code == KeyCode::Char('d') && key.modifiers.is_empty() {
                        self.download_selected_archive_item();
                        return;
                    }
                    if (key.code == KeyCode::Char('s') || key.code == KeyCode::Char('p'))
                        && key.modifiers.is_empty()
                    {
                        self.stream_selected_archive_item();
                        return;
                    }
                }
            }
        }

        let Some(action) = self.keymap.resolve(key) else {
            return;
        };
        self.handle_action(action);
    }

    #[cfg(feature = "nyaa")]
    pub(crate) fn handle_nyaa_query_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Enter => {
                self.nyaa_plugin.editing_query = false;
                let query = self.nyaa_plugin.query_input.value().to_string();
                self.search_nyaa(query);
                return true;
            }
            KeyCode::Esc => {
                self.nyaa_plugin.editing_query = false;
                return true;
            }
            _ => {}
        }

        match key.code {
            KeyCode::Char('w') if ctrl => self.nyaa_plugin.query_input.delete_word(),
            KeyCode::Char('u') if ctrl => self.nyaa_plugin.query_input.clear(),
            KeyCode::Char(ch) => self.nyaa_plugin.query_input.insert(ch),
            KeyCode::Backspace => self.nyaa_plugin.query_input.backspace(),
            KeyCode::Delete => self.nyaa_plugin.query_input.delete(),
            KeyCode::Left => self.nyaa_plugin.query_input.left(),
            KeyCode::Right => self.nyaa_plugin.query_input.right(),
            KeyCode::Home => self.nyaa_plugin.query_input.home(),
            KeyCode::End => self.nyaa_plugin.query_input.end(),
            _ => {}
        }
        true
    }

    pub(crate) fn handle_jamendo_query_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Enter => {
                self.jamendo_plugin.editing_query = false;
                let query = self.jamendo_plugin.query_input.value().to_string();
                self.search_jamendo(query);
                return true;
            }
            KeyCode::Esc => {
                self.jamendo_plugin.editing_query = false;
                return true;
            }
            _ => {}
        }

        match key.code {
            KeyCode::Char('w') if ctrl => self.jamendo_plugin.query_input.delete_word(),
            KeyCode::Char('u') if ctrl => self.jamendo_plugin.query_input.clear(),
            KeyCode::Char(ch) => self.jamendo_plugin.query_input.insert(ch),
            KeyCode::Backspace => self.jamendo_plugin.query_input.backspace(),
            KeyCode::Delete => self.jamendo_plugin.query_input.delete(),
            KeyCode::Left => self.jamendo_plugin.query_input.left(),
            KeyCode::Right => self.jamendo_plugin.query_input.right(),
            KeyCode::Home => self.jamendo_plugin.query_input.home(),
            KeyCode::End => self.jamendo_plugin.query_input.end(),
            _ => {}
        }
        true
    }

    pub(crate) fn handle_archive_query_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Enter => {
                self.archive_plugin.editing_query = false;
                let query = self.archive_plugin.query_input.value().to_string();
                self.search_archive(query);
                return true;
            }
            KeyCode::Esc => {
                self.archive_plugin.editing_query = false;
                return true;
            }
            _ => {}
        }

        match key.code {
            KeyCode::Char('w') if ctrl => self.archive_plugin.query_input.delete_word(),
            KeyCode::Char('u') if ctrl => self.archive_plugin.query_input.clear(),
            KeyCode::Char(ch) => self.archive_plugin.query_input.insert(ch),
            KeyCode::Backspace => self.archive_plugin.query_input.backspace(),
            KeyCode::Delete => self.archive_plugin.query_input.delete(),
            KeyCode::Left => self.archive_plugin.query_input.left(),
            KeyCode::Right => self.archive_plugin.query_input.right(),
            KeyCode::Home => self.archive_plugin.query_input.home(),
            KeyCode::End => self.archive_plugin.query_input.end(),
            _ => {}
        }
        true
    }

    /// Route a keystroke into the open settings text field.
    ///
    /// Returns whether the key was consumed; everything reaches the field
    /// except the keys that close it, so no global binding can fire mid-edit.
    pub(crate) fn handle_settings_edit_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Enter => {
                self.commit_setting_edit();
                return true;
            }
            KeyCode::Esc => {
                self.settings_edit = None;
                return true;
            }
            _ => {}
        }

        let Some(input) = self.settings_edit.as_mut() else {
            return true;
        };
        match key.code {
            KeyCode::Char('w') if ctrl => input.delete_word(),
            KeyCode::Char('u') if ctrl => input.clear(),
            KeyCode::Char(ch) => input.insert(ch),
            KeyCode::Backspace => input.backspace(),
            KeyCode::Delete => input.delete(),
            KeyCode::Left => input.left(),
            KeyCode::Right => input.right(),
            KeyCode::Home => input.home(),
            KeyCode::End => input.end(),
            _ => {}
        }
        true
    }

    pub fn handle_action(&mut self, action: Action) {
        match action {
            Action::Quit => {
                self.save_queue_state();
                self.player.shared.set_paused(true);
                self.player.shared.request_flush();
                self.player.send(PlayerCommand::Stop);
                self.should_quit = true;
            }
            Action::NextTab => self.cycle_tab(1),
            Action::PrevTab => self.cycle_tab(-1),
            Action::TabBack => {
                if let Some(previous) = self.tab_history.pop() {
                    self.tab = previous;
                    self.focus = self.panes()[0];
                }
            }
            Action::Tab(index) => {
                let available = Tab::available(&self.config);
                if let Some(tab) = available.get(index) {
                    self.go_to_tab(*tab);
                }
            }

            // Home is a horizontal row; up/down would have nowhere to go.
            Action::Up if self.focus == Pane::Home => {}
            Action::Down if self.focus == Pane::Home => {}
            Action::Up => self.move_selection(-1),
            Action::Down => self.move_selection(1),
            Action::PageUp => self.move_selection(-10),
            Action::PageDown => self.move_selection(10),
            Action::Top => self.move_selection(isize::MIN / 2),
            Action::Bottom => self.move_selection(isize::MAX / 2),
            // Home's mixes are drawn as a row, so they are navigated as one.
            // Home's mixes are drawn as a horizontal row, so Left/Right walk
            // them rather than cycling panes — until the cursor reaches the
            // end, where the next step continues into whatever is drawn there.
            Action::Left => match self.focus {
                Pane::Settings => self.adjust_setting(-1),
                Pane::Home if self.home_sel.index > 0 => self.move_selection(-1),
                _ => self.focus_by(-1),
            },
            Action::Right => match self.focus {
                Pane::Settings => self.adjust_setting(1),
                Pane::Home if self.home_sel.index + 1 < crate::ui::home::mix_count(self) => {
                    self.move_selection(1)
                }
                _ => self.focus_by(1),
            },
            // Unconditional pane cycling. Left/Right are spoken for on Home
            // (the mix row) and Settings (value editing), so these are the only
            // way to reach the side panes from the keyboard on those tabs.
            Action::FocusNext => self.cycle_focus(1),
            Action::FocusPrev => self.cycle_focus(-1),
            Action::Confirm => self.activate(),
            Action::Cancel => {
                self.show_help = false;
                if self.focus_mode {
                    // Goes through the setter so focus is handed back to the
                    // tab underneath rather than left on the lyrics.
                    self.set_focus_mode(false);
                }
                self.status_message = None;
            }

            Action::JumpToArtist => self.jump_to_current_artist(),
            Action::JumpToAlbum => self.jump_to_current_album(),

            Action::TogglePause => self.player.send(PlayerCommand::TogglePause),
            Action::Stop => self.player.send(PlayerCommand::Stop),
            Action::NextTrack => self.player.send(PlayerCommand::Next),
            Action::PrevTrack => self.player.send(PlayerCommand::Prev),
            Action::SeekForward => self.player.send(PlayerCommand::SeekForward),
            Action::SeekBackward => self.player.send(PlayerCommand::SeekBackward),
            Action::VolumeUp => self.player.send(PlayerCommand::AdjustVolume(VOLUME_STEP)),
            Action::VolumeDown => self.player.send(PlayerCommand::AdjustVolume(-VOLUME_STEP)),

            Action::AddToQueue => {
                let songs = self.selected_songs();
                if !songs.is_empty() {
                    self.status_message = Some(format!("Queued {} track(s)", songs.len()));
                    self.player.send(PlayerCommand::Enqueue(songs));
                    // Advance so repeated presses queue consecutive tracks.
                    self.move_selection(1);
                }
            }
            Action::RemoveFromQueue => {
                if self.focus == Pane::Settings {
                    // The same "remove the selected thing" gesture, applied to
                    // a music folder or a queue column.
                    self.delete_setting();
                } else if self.focus == Pane::PlaylistSongs {
                    self.remove_from_playlist();
                } else if self.tab == Tab::Queue || self.focus == Pane::Queue {
                    self.snapshot_queue();
                    self.player
                        .send(PlayerCommand::Remove(self.queue_sel.index));
                }
            }
            Action::ClearQueue => {
                self.snapshot_queue();
                // One command rather than clearing the queue here and stopping
                // separately: the player owns both halves of that transition.
                self.player.send(PlayerCommand::Clear);
                self.save_queue_state();
                self.status_message = Some("Queue cleared — C-z to undo".to_string());
            }
            Action::ToggleRepeat => self.player.send(PlayerCommand::ToggleRepeat),
            Action::ToggleShuffle => self.player.send(PlayerCommand::ToggleShuffle),

            Action::Refresh => {
                self.status_message = Some("Refreshing library…".to_string());
                self.bootstrap();
            }
            Action::ToggleHelp => self.show_help = !self.show_help,
            Action::ToggleQueuePane => {
                // Whichever Up Next the user is actually looking at.
                if self.focus_mode {
                    self.show_focus_queue = !self.show_focus_queue;
                } else {
                    self.show_queue_pane = !self.show_queue_pane;
                }
                self.ensure_focus_visible();
            }
            Action::ToggleCoverPane => {
                // Whichever cover the user is actually looking at.
                if self.focus_mode {
                    self.show_focus_cover = !self.show_focus_cover;
                } else {
                    self.show_cover_pane = !self.show_cover_pane;
                }
            }
            Action::ToggleLyricsPane => {
                // Whichever lyrics the user is actually looking at.
                if self.focus_mode {
                    self.show_focus_lyrics = !self.show_focus_lyrics;
                } else {
                    self.show_lyrics_pane = !self.show_lyrics_pane;
                }
                self.save_queue_state();
            }
            Action::ToggleVisualiser => self.show_visualiser = !self.show_visualiser,
            Action::CycleVisualiser => {
                self.viz_mode = self.viz_mode.next();
                // Turning the pane on saves the user wondering why `V` did
                // nothing when the visualiser is hidden.
                self.show_visualiser = true;
                self.status_message = Some(format!("Visualiser: {}", self.viz_mode.label()));
                self.save_queue_state();
            }
            Action::CycleLyricVariant => self.cycle_lyric_variant(),
            Action::TranslateLyrics => self.translate_lyrics(),
            Action::FetchOnlineLyrics => self.fetch_online_lyrics_action(),
            Action::ToggleRadio => {
                let enabled = !self.player.queue.lock().unwrap().radio;
                self.status_message = Some(if enabled {
                    "Radio mode on — queue extends automatically".to_string()
                } else {
                    "Radio mode off".to_string()
                });
                self.player.send(PlayerCommand::ToggleRadio);
            }
            Action::ToggleStar => {
                if let Some(song) = self.player.status().current {
                    let starred = !song.is_starred();
                    self.status_message = Some(if starred {
                        "Starred".into()
                    } else {
                        "Unstarred".to_string()
                    });
                    self.player.send(PlayerCommand::SetStarred {
                        song_id: song.id,
                        starred,
                    });
                }
            }

            // The side panes sit on the right, so the arrow moves the *divider*
            // rather than the panes: left widens them, right narrows them.
            // Doing it the other way round made the boundary travel opposite to
            // the key that was pressed.
            Action::ResizePaneLeft => self.nudge_side_panes(1),
            Action::ResizePaneRight => self.nudge_side_panes(-1),
            Action::ResizePaneUp => self.nudge_visualiser_height(1),
            Action::ResizePaneDown => self.nudge_visualiser_height(-1),

            Action::LibraryModeNext => {
                if self.tab == Tab::Library {
                    self.cycle_library_mode(1);
                } else {
                    self.go_to_tab(Tab::Library);
                }
            }
            Action::LibraryModePrev => {
                if self.tab == Tab::Library {
                    self.cycle_library_mode(-1);
                } else {
                    self.go_to_tab(Tab::Library);
                }
            }
            Action::PlayNext => {
                let songs = self.selected_songs();
                if !songs.is_empty() {
                    let count = songs.len();
                    self.player.queue.lock().unwrap().insert_next(songs);
                    self.status_message = Some(format!("Playing {count} track(s) next"));
                    self.save_queue_state();
                }
            }
            Action::MoveTrackUp => self.move_queued_track(-1),
            Action::MoveTrackDown => self.move_queued_track(1),
            Action::AddToPlaylist => self.open_playlist_picker(),
            Action::Share => self.open_share(),
            Action::RatingUp => self.adjust_rating(1),
            Action::RatingDown => self.adjust_rating(-1),
            Action::OpenPalette => self.open_palette(),
            Action::UndoQueue => self.undo_queue(),
            Action::ToggleFocusMode => self.set_focus_mode(!self.focus_mode),
        }

        // Pane sizes are part of the saved layout; persist them as they change
        // rather than relying on a later save happening to pick them up.
        if matches!(
            action,
            Action::ResizePaneLeft
                | Action::ResizePaneRight
                | Action::ResizePaneUp
                | Action::ResizePaneDown
        ) {
            self.save_queue_state();
        }
    }

    /// Route a mouse event using the hit map built during the last draw.
    pub fn handle_mouse(&mut self, event: crossterm::event::MouseEvent, hits: &Hits) {
        use crossterm::event::{MouseButton, MouseEventKind};

        match event.kind {
            MouseEventKind::ScrollDown => self.move_selection(3),
            MouseEventKind::ScrollUp => self.move_selection(-3),

            MouseEventKind::Down(MouseButton::Left) => {
                let Some(region) = hits.at(event.column, event.row) else {
                    return;
                };

                match region {
                    Region::Tab(index) => {
                        let available = Tab::available(&self.config);
                        if let Some(tab) = available.get(index) {
                            self.go_to_tab(*tab);
                        }
                    }
                    Region::LibraryMode(index) => {
                        if let Some(mode) = LibraryMode::ALL.get(index) {
                            self.set_library_mode(*mode);
                        }
                    }
                    Region::OnlineSearch => self.focus_online_search(),
                    Region::OnlineFilter => self.cycle_online_filter(),
                    Region::OnlineSource(index) => {
                        if let Some(source) = OnlineSource::available(&self.config).get(index) {
                            self.set_online_source(*source);
                        }
                    }
                    Region::PlayPause => self.player.send(PlayerCommand::TogglePause),
                    Region::Repeat => self.player.send(PlayerCommand::ToggleRepeat),
                    Region::Shuffle => self.player.send(PlayerCommand::ToggleShuffle),
                    Region::Visualiser => {
                        self.active_drag = Some(Region::Visualiser);
                        self.drag_start_viz_height = Some(self.visualiser_height);
                        self.update_visualiser_drag(event.row, hits);
                    }
                    Region::CurrentArtist => self.jump_to_current_artist(),
                    Region::CurrentAlbum => self.jump_to_current_album(),
                    Region::Seek => {
                        self.active_drag = Some(Region::Seek);
                        self.update_drag_ratio(event.column, hits, Region::Seek);
                    }
                    Region::Volume => {
                        self.active_drag = Some(Region::Volume);
                        self.update_drag_ratio(event.column, hits, Region::Volume);
                    }
                    Region::Cover => {
                        let url = "https://github.com/Kolbxyz/Wander";
                        open_url(url);
                        copy_to_clipboard(url);
                        self.status_message = Some(format!("Opened {url}"));
                    }
                    Region::Row { pane, index } => {
                        self.focus = pane;
                        // Only synced lyrics carry timestamps; clicking an
                        // unsynced line would otherwise seek to zero.
                        if pane == Pane::Lyrics {
                            if self.lyrics.synced {
                                if let Some(line) = self.lyrics.lines.get(index) {
                                    self.player.send(PlayerCommand::SeekTo(line.at));
                                }
                            } else {
                                self.select_in(Pane::Lyrics, index);
                            }
                        } else {
                            self.select_in(pane, index);

                            let now = Instant::now();
                            let is_double = matches!(
                                self.last_click,
                                Some((last, at)) if last == region && now.duration_since(at) < DOUBLE_CLICK
                            );
                            self.last_click = Some((region, now));
                            if is_double || pane == Pane::Online {
                                self.activate();
                            }
                        }
                    }
                }
            }

            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(drag_region) = self.active_drag {
                    if drag_region == Region::Visualiser {
                        self.update_visualiser_drag(event.row, hits);
                    } else {
                        self.update_drag_ratio(event.column, hits, drag_region);
                    }
                }
            }

            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(drag_region) = self.active_drag.take() {
                    match drag_region {
                        Region::Visualiser => {
                            let start = self.drag_start_viz_height.take();
                            if start == Some(self.visualiser_height) {
                                self.handle_action(Action::CycleVisualiser);
                            } else {
                                self.save_queue_state();
                            }
                        }
                        Region::Seek => {
                            let Some(song) = self.player.status().current else {
                                return;
                            };
                            let target =
                                Duration::from_secs_f64(song.duration as f64 * self.drag_ratio);
                            self.player.send(PlayerCommand::SeekTo(target));
                        }
                        Region::Volume => {
                            self.player
                                .send(PlayerCommand::SetVolume(self.drag_ratio as f32));
                        }
                        _ => {}
                    }
                }
            }

            _ => {}
        }
    }

    pub(crate) fn update_drag_ratio(&mut self, column: u16, hits: &Hits, region: Region) {
        let Some(rect) = hits.rect_of(region) else {
            return;
        };
        // Both sliders fill their registered rect exactly, so the click maps
        // straight through with no padding to compensate for.
        if rect.width == 0 {
            return;
        }
        let offset = column.saturating_sub(rect.x);
        self.drag_ratio = crate::ui::widgets::Slider::ratio_at(offset, rect.width);
    }
}
