use super::layout::*;
use super::types::*;
use super::*;
use anyhow::Result;

impl App {
    /// Whether the app has nothing to play from yet.
    pub fn is_unconfigured(&self) -> bool {
        let no_server = !self.config.server.enabled
            || self.config.server.url.trim().is_empty()
            || self.config.server.username.trim().is_empty();
        no_server && self.config.local.paths.is_empty()
    }

    /// Open the first-run chooser, if there is nothing set up yet.
    pub fn maybe_start_setup(&mut self) {
        if self.is_unconfigured() && self.overlay.is_none() {
            self.overlay = Some(Overlay::Setup(Default::default()));
        }
    }

    /// Act on the first-run choice: land on the settings row that starts the
    /// chosen path, so the next keypress is already the useful one.
    pub(crate) fn begin_setup(&mut self, choice: usize) {
        use crate::ui::settings::SettingItem;
        // 1 is "local folder"; 0 and 2 both begin with the server.
        let target = if choice == 1 {
            SettingItem::AddLocalPath
        } else {
            SettingItem::ServerUrl
        };

        self.go_to_tab(Tab::Settings);
        self.focus = Pane::Settings;
        if let Some(index) = crate::ui::settings::rows(&self.config)
            .iter()
            .position(|item| *item == target)
        {
            self.settings_sel.index = index;
        }
        self.status_message = Some(match choice {
            1 => "Press Enter to type the path to your music folder".to_string(),
            _ => "Press Enter to type your server URL, then fill in the rows below".to_string(),
        });
        // Open the field straight away: the user already said what they want.
        self.activate_setting();
    }

    /// The settings row currently selected.
    pub fn selected_setting(&self) -> Option<crate::ui::settings::SettingItem> {
        let rows = crate::ui::settings::rows(&self.config);
        rows.get(self.settings_sel.index.min(rows.len().saturating_sub(1)))
            .copied()
    }

    /// Left/Right on a settings row: cycle or nudge the value in place.
    pub fn adjust_setting(&mut self, delta: isize) {
        use crate::ui::settings::SettingItem;
        let Some(item) = self.selected_setting() else {
            return;
        };

        match item {
            SettingItem::ServerEnabled => {
                self.config.server.enabled = !self.config.server.enabled;
                let _ = self.config.save();
                self.apply_server_config();
            }
            SettingItem::StreamFormat => {
                let formats = [None, Some("mp3"), Some("opus"), Some("flac")];
                let current = formats
                    .iter()
                    .position(|f| f.map(String::from) == self.config.server.format)
                    .unwrap_or(0) as isize;
                let next = (current + delta).rem_euclid(formats.len() as isize) as usize;
                self.config.server.format = formats[next].map(String::from);
                let _ = self.config.save();
                // The format is baked into the stream URL, so the client has to
                // be rebuilt for the change to take effect.
                self.apply_server_config();
                self.status_message = Some(format!(
                    "Stream format: {}",
                    self.config.server.format.as_deref().unwrap_or("raw")
                ));
            }

            SettingItem::ScanOnStart => {
                self.config.local.scan_on_start = !self.config.local.scan_on_start;
                let _ = self.config.save();
            }

            SettingItem::ThemePreset => self.cycle_theme_preset(delta),
            SettingItem::Glyphs => {
                use crate::ui::glyphs::GlyphSet;
                let sets = [GlyphSet::Nerd, GlyphSet::Unicode, GlyphSet::Ascii];
                let current = sets
                    .iter()
                    .position(|s| *s == self.config.glyphs)
                    .unwrap_or(0) as isize;
                let next = (current + delta).rem_euclid(sets.len() as isize) as usize;
                self.config.glyphs = sets[next];
                let _ = self.config.save();
                self.status_message = Some(format!("Icons: {:?}", self.config.glyphs));
            }
            SettingItem::CoverWidth => {
                self.cover_percent = if delta > 0 {
                    (self.cover_percent + 2).min(60)
                } else {
                    self.cover_percent.saturating_sub(2).max(15)
                };
                self.save_queue_state();
            }
            SettingItem::QueueWidth => {
                self.queue_percent = if delta > 0 {
                    (self.queue_percent + 2).min(50)
                } else {
                    self.queue_percent.saturating_sub(2).max(10)
                };
                self.save_queue_state();
            }
            SettingItem::ShowCover => {
                self.show_cover_pane = !self.show_cover_pane;
                self.save_queue_state();
            }
            SettingItem::ShowQueue => {
                self.show_queue_pane = !self.show_queue_pane;
                self.ensure_focus_visible();
                self.save_queue_state();
            }
            SettingItem::ShowLyrics => {
                self.show_lyrics_pane = !self.show_lyrics_pane;
                self.save_queue_state();
            }

            SettingItem::VolumeScale => {
                self.config.volume_log = !self.config.volume_log;
                let _ = self.config.save();
                self.status_message = Some(format!(
                    "Volume scaling: {}",
                    if self.config.volume_log {
                        "Logarithmic (perceptual)"
                    } else {
                        "Linear"
                    }
                ));
            }
            SettingItem::BufferSeconds => {
                // Bounded well away from zero: too small a buffer underruns on
                // any network hiccup, too large makes seeking feel unresponsive.
                let next = self.config.buffer_seconds + if delta > 0 { 0.5 } else { -0.5 };
                self.config.buffer_seconds = next.clamp(1.0, 30.0);
                let _ = self.config.save();
                self.status_message = Some("Audio buffer applies on the next start".to_string());
            }
            SettingItem::AutoMix => {
                // Through the player, so switching it on fills the queue right
                // away rather than waiting for the next track to end.
                let active = !self.player.queue.lock().unwrap().radio;
                self.player.send(PlayerCommand::ToggleRadio);
                self.save_queue_state();
                self.status_message =
                    Some(format!("Auto-Mix: {}", if active { "ON" } else { "OFF" }));
            }

            SettingItem::DiscordEnabled => {
                self.config.discord.enabled = !self.config.discord.enabled;
                let _ = self.config.save();
                self.status_message = Some(format!(
                    "Discord Rich Presence: {} (restart to apply)",
                    if self.config.discord.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    }
                ));
            }
            SettingItem::DiscordCoverArt => {
                self.config.discord.cover_art = !self.config.discord.cover_art;
                let _ = self.config.save();
            }
            SettingItem::FetchOnlineLyrics => {
                self.config.lyrics.fetch_online = !self.config.lyrics.fetch_online;
                let _ = self.config.save();
                self.status_message = Some(format!(
                    "Online lyrics (LRCLIB): {}",
                    if self.config.lyrics.fetch_online {
                        "enabled"
                    } else {
                        "disabled"
                    }
                ));
            }

            SettingItem::QueueColumn(index) => {
                if let Some(column) = self.config.queue_columns.get_mut(index) {
                    let width = column.width as isize + delta * 5;
                    column.width = width.clamp(5, 90) as u16;
                    let _ = self.config.save();
                }
            }

            // Rows whose only interaction is Enter.
            SettingItem::ServerUrl
            | SettingItem::ServerUsername
            | SettingItem::ServerPassword
            | SettingItem::TestConnection
            | SettingItem::LocalPath(_)
            | SettingItem::AddLocalPath
            | SettingItem::LocalPlaylistDir
            | SettingItem::Rescan
            | SettingItem::ClearQueue
            | SettingItem::DiscordClientId
            | SettingItem::LrclibUrl
            | SettingItem::AddQueueColumn
            | SettingItem::ShowKeybindings => {}
        }
    }

    /// Enter on a settings row: open a text field, or run the row's action.
    pub fn activate_setting(&mut self) {
        use crate::ui::settings::SettingItem;
        let Some(item) = self.selected_setting() else {
            return;
        };

        if item.is_text() {
            let current = match item {
                // Never pre-fill the password field from the keyring: the panel
                // shows that one is stored without ever handling the secret.
                SettingItem::ServerPassword => String::new(),
                SettingItem::ServerUrl => self.config.server.url.clone(),
                SettingItem::ServerUsername => self.config.server.username.clone(),
                SettingItem::LocalPath(index) => self
                    .config
                    .local
                    .paths
                    .get(index)
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                SettingItem::AddLocalPath => String::new(),
                SettingItem::LocalPlaylistDir => self
                    .config
                    .local
                    .playlist_dir
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                SettingItem::DiscordClientId => self.config.discord.client_id.clone(),
                SettingItem::LrclibUrl => self.config.lyrics.lrclib_url.clone(),
                _ => String::new(),
            };
            self.settings_edit =
                Some(crate::ui::widgets::TextInput::new(current).masked(item.is_secret()));
            return;
        }

        match item {
            SettingItem::TestConnection => self.test_connection(),
            SettingItem::Rescan => self.rescan_local_library(),
            SettingItem::ClearQueue => {
                self.snapshot_queue();
                // One command rather than clearing the queue here and stopping
                // separately: the player owns both halves of that transition.
                self.player.send(PlayerCommand::Clear);
                self.save_queue_state();
                self.status_message = Some("Queue cleared — C-z to undo".to_string());
            }
            SettingItem::ShowKeybindings => self.show_help = true,
            SettingItem::QueueColumn(index) => {
                use crate::config::ColumnKind;
                const KINDS: &[ColumnKind] = &[
                    ColumnKind::Artist,
                    ColumnKind::Title,
                    ColumnKind::Album,
                    ColumnKind::Length,
                    ColumnKind::Track,
                    ColumnKind::Year,
                ];
                if let Some(column) = self.config.queue_columns.get_mut(index) {
                    let current = KINDS.iter().position(|k| *k == column.kind).unwrap_or(0);
                    column.kind = KINDS[(current + 1) % KINDS.len()];
                    let _ = self.config.save();
                }
            }
            SettingItem::AddQueueColumn => {
                use crate::config::{Column, ColumnKind};
                self.config.queue_columns.push(Column {
                    kind: ColumnKind::Year,
                    width: 10,
                });
                let _ = self.config.save();
            }
            // Toggles are reachable with Enter as well as Left/Right, which is
            // what the on-screen hint promises.
            _ => self.adjust_setting(1),
        }
    }

    /// Delete on a settings row: remove the per-item entry it stands for.
    pub fn delete_setting(&mut self) {
        use crate::ui::settings::SettingItem;
        let Some(item) = self.selected_setting() else {
            return;
        };
        match item {
            SettingItem::LocalPath(index) if index < self.config.local.paths.len() => {
                let removed = self.config.local.paths.remove(index);
                let _ = self.config.save();
                self.apply_local_config();
                self.status_message = Some(format!("Removed {}", removed.display()));
            }
            SettingItem::QueueColumn(index) if index < self.config.queue_columns.len() => {
                // Never leave the queue with no columns at all; there would be
                // nothing to click on and no way to add one back.
                if self.config.queue_columns.len() > 1 {
                    self.config.queue_columns.remove(index);
                    let _ = self.config.save();
                } else {
                    self.status_message = Some("The queue needs at least one column".to_string());
                }
            }
            SettingItem::ServerPassword => {
                let _ = crate::paths::delete_keyring_password(&self.config.server.username);
                self.has_stored_password = false;
                self.status_message = Some("Removed the stored password".to_string());
            }
            _ => {}
        }
        let len = crate::ui::settings::rows(&self.config).len();
        self.settings_sel.clamp(len);
    }

    /// Enter in an open settings text field: store what was typed.
    pub fn commit_setting_edit(&mut self) {
        use crate::ui::settings::SettingItem;
        let Some(input) = self.settings_edit.take() else {
            return;
        };
        let Some(item) = self.selected_setting() else {
            return;
        };
        let value = input.value().trim().to_string();

        match item {
            SettingItem::ServerUrl => {
                // Stored without a trailing slash so the client's `{base}/rest/…`
                // never produces a double slash.
                self.config.server.url = value.trim_end_matches('/').to_string();
                let _ = self.config.save();
                self.apply_server_config();
            }
            SettingItem::ServerUsername => {
                self.config.server.username = value;
                let _ = self.config.save();
                self.refresh_password_state();
                self.apply_server_config();
            }
            SettingItem::ServerPassword => {
                if value.is_empty() {
                    self.status_message = Some("Password unchanged".to_string());
                    return;
                }
                if self.config.server.username.trim().is_empty() {
                    self.status_message = Some("Set a username first".to_string());
                    return;
                }
                // Into the OS keyring, never into config.toml.
                match crate::paths::store_keyring_password(&self.config.server.username, &value) {
                    Ok(()) => {
                        // Any plaintext password left in the config is now
                        // redundant, and keeping it would silently win over the
                        // keyring entry the user just set.
                        self.config.server.password.clear();
                        let _ = self.config.save();
                        self.has_stored_password = true;
                        self.status_message = Some("Password stored in the keyring".to_string());
                        self.apply_server_config();
                    }
                    Err(err) => {
                        self.status_message = Some(format!("Could not store password: {err:#}"))
                    }
                }
            }
            SettingItem::LocalPath(index) => {
                if value.is_empty() {
                    return;
                }
                if let Some(slot) = self.config.local.paths.get_mut(index) {
                    *slot = expand_home(&value);
                    let _ = self.config.save();
                    self.apply_local_config();
                }
            }
            SettingItem::AddLocalPath => {
                if value.is_empty() {
                    return;
                }
                let path = expand_home(&value);
                if !path.is_dir() {
                    self.status_message = Some(format!("{} is not a folder", path.display()));
                    return;
                }
                self.config.local.paths.push(path);
                let _ = self.config.save();
                self.apply_local_config();
                self.rescan_local_library();
            }
            SettingItem::LocalPlaylistDir => {
                self.config.local.playlist_dir = (!value.is_empty()).then(|| expand_home(&value));
                let _ = self.config.save();
                self.apply_local_config();
            }
            SettingItem::DiscordClientId => {
                self.config.discord.client_id = value;
                let _ = self.config.save();
                self.status_message =
                    Some("Discord application ID saved (restart to apply)".into());
            }
            SettingItem::LrclibUrl => {
                self.config.lyrics.lrclib_url = value;
                let _ = self.config.save();
                self.status_message = Some("LRCLIB server URL saved".into());
            }
            _ => {}
        }
    }

    /// Note whether a password is available, without reading the secret.
    pub fn refresh_password_state(&mut self) {
        self.has_stored_password = !self.config.server.password.is_empty()
            || self
                .config
                .password()
                .map(|p| !p.is_empty())
                .unwrap_or(false);
    }

    /// Rebuild the Subsonic backend from the current config and swap it in.
    ///
    /// Nothing downstream is touched: `App`, the player task and Discord all
    /// hold the same `MergedLibrary`, so this takes effect immediately and
    /// without a restart.
    pub fn apply_server_config(&mut self) {
        let Some(root) = self.library_root.clone() else {
            return;
        };
        match crate::build_remote(&self.config) {
            Ok(remote) => {
                root.set_remote(remote);
                self.connection_status = None;
                self.invalidate_library();
            }
            Err(err) => self.status_message = Some(format!("Server settings: {err:#}")),
        }
    }

    /// Rebuild the local backend from the current config and swap it in.
    pub fn apply_local_config(&mut self) {
        let Some(root) = self.library_root.clone() else {
            return;
        };
        if self.config.local.paths.is_empty() {
            root.set_local(None);
        } else if let Some(local) = root.local() {
            local.set_playlist_dir(self.config.local.playlist_dir.clone());
        } else {
            root.set_local(Some(Arc::new(crate::library::LocalLibrary::from_cache(
                self.config.local.playlist_dir.clone(),
            ))));
        }
        self.invalidate_library();
    }

    /// Drop the cached library views so the next visit to a tab refetches.
    ///
    /// Called after a backend changes, because every list on screen came from
    /// the old one and would otherwise keep showing a library that is no longer
    /// configured.
    pub fn invalidate_library(&mut self) {
        // Artists, albums and playlists are reloaded whenever their list is
        // empty, so clearing them is what marks them stale; tracks and
        // favourites carry an explicit flag.
        self.artists.clear();
        self.albums.clear();
        self.playlists.clear();
        self.tracks.clear();
        self.tracks_loaded = false;
        self.favorites.clear();
        self.favorites_loaded = false;
        self.bootstrap();
    }

    pub(crate) fn test_connection(&mut self) {
        self.connection_status = Some("checking…".to_string());
        match crate::build_remote(&self.config) {
            Ok(None) => {
                self.connection_status =
                    Some("no server configured (set a URL and username)".to_string())
            }
            Err(err) => self.connection_status = Some(format!("failed: {err:#}")),
            Ok(Some(remote)) => {
                let loads = self.loads.clone();
                tokio::spawn(async move {
                    let result = match remote.client().ping().await {
                        Ok(version) => format!("OK — server version {version}"),
                        Err(err) => format!("failed: {err:#}"),
                    };
                    let _ = loads.send(LoadEvent::ConnectionTested(result));
                });
            }
        }
    }

    pub fn rescan_local_library(&mut self) {
        if self.config.local.paths.is_empty() {
            self.scan_status = Some("add a music folder first".to_string());
            return;
        }
        let Some(root) = self.library_root.clone() else {
            return;
        };
        // Make sure a backend exists to receive the scan.
        self.apply_local_config();
        let Some(local) = root.local() else { return };

        let roots = self.config.local.paths.clone();
        let loads = self.loads.clone();
        self.scan_status = Some("scanning…".to_string());

        tokio::task::spawn_blocking(move || {
            let previous = local.index();
            let index = crate::library::local::scan::scan(&roots, &previous, |_| {});
            let songs = index.tracks.len();
            let albums = index.albums().len();
            let _ = index.save();
            local.set_index(index);
            let _ = loads.send(LoadEvent::LocalScanned { songs, albums });
        });
    }

    pub fn cycle_theme_preset(&mut self, delta: isize) {
        let names = Theme::PRESET_NAMES;
        let current = self
            .config
            .theme_preset
            .as_deref()
            .and_then(|active| names.iter().position(|&n| n == active))
            .unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(names.len() as isize) as usize;
        let preset_name = names[next];
        self.config.theme = Theme::preset(preset_name);
        self.config.theme_preset = Some(preset_name.to_string());
        // Re-fold the current artwork's accents over the new preset. Saving
        // first keeps the pristine preset on disk, not the tinted result.
        let _ = self.config.save();
        self.refresh_theme();
        self.status_message = Some(format!("Theme set to {preset_name}"));
    }

    pub fn jump_to_current_artist(&mut self) {
        let status = self.player.status();
        let Some(song) = status.current.as_ref() else {
            return;
        };
        let artist_name = song.artist_or_unknown();
        if let Some(pos) = self.artists.iter().position(|a| {
            a.id == song.artist_id.clone().unwrap_or_default()
                || a.name.eq_ignore_ascii_case(artist_name)
        }) {
            self.go_to_library(LibraryMode::Artists);
            self.artist_sel.index = pos;
            let artist_id = self.artists[pos].id.clone();
            self.load_artist_albums(artist_id);
            self.status_message = Some(format!("Jumped to artist: {artist_name}"));
        } else {
            self.status_message = Some(format!("Artist '{artist_name}' not found in library"));
        }
    }

    pub fn jump_to_current_album(&mut self) {
        let status = self.player.status();
        let Some(song) = status.current.as_ref() else {
            return;
        };
        let album_title = song.album_or_unknown();
        if let Some(pos) = self.albums.iter().position(|a| {
            a.id == song.album_id.clone().unwrap_or_default()
                || a.name.eq_ignore_ascii_case(album_title)
        }) {
            self.go_to_library(LibraryMode::Albums);
            self.album_sel.index = pos;
            let album_id = self.albums[pos].id.clone();
            self.load_album_songs(album_id);
            self.status_message = Some(format!("Jumped to album: {album_title}"));
        } else {
            self.status_message = Some(format!("Album '{album_title}' not found in library"));
        }
    }
}
