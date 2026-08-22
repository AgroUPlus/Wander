//! Jam actions, as the TUI drives them.
//!
//! Every one of these is fire-and-forget: the request goes out on a task and the answer arrives as
//! a [`LoadEvent::Jam`], which replaces the whole local copy. That is the same shape the server
//! uses — each mutation returns the entire jam — so there is never a partial update to merge, and
//! somebody else's vote landing between your keypress and the answer is simply part of the result.

use crate::app::types::LoadEvent;
use crate::app::App;
use crate::integrations::agro_jam::JamMode;

impl App {
    /// The client, or nothing when Agro is not configured. Every action starts here.
    fn jam_client(&self) -> Option<std::sync::Arc<crate::integrations::agro::AgroClient>> {
        crate::integrations::agro::ACTIVE_CLIENT.get().cloned()
    }

    /// Re-reads the jam. Called on opening the tab and on a `JAM_UPDATED` frame.
    pub fn refresh_jam(&mut self) {
        let Some(client) = self.jam_client() else { return };
        let loads = self.loads.clone();
        tokio::spawn(async move {
            if let Ok(jam) = client.jam().await {
                let _ = loads.send(LoadEvent::Jam(jam));
            }
        });
    }

    pub fn create_jam(&mut self) {
        let Some(client) = self.jam_client() else { return };
        self.borrow_queue_for_jam();
        let loads = self.loads.clone();
        self.status_message = Some("Starting a jam…".into());
        tokio::spawn(async move {
            if let Ok(jam) = client.create_jam(JamMode::Democracy).await {
                let _ = loads.send(LoadEvent::Jam(jam));
            }
        });
    }

    /// Opens the prompt for a join code. The code is the whole credential, so it is typed rather
    /// than discovered.
    pub fn prompt_join_jam(&mut self) {
        self.status_message =
            Some("Join code: type it, then press Enter (Esc cancels)".into());
        self.jam_join_input = Some(String::new());
    }

    /// Keeps what was playing, so leaving the jam can put it back. A jam borrows the queue.
    pub(crate) fn borrow_queue_for_jam(&mut self) {
        if self.jam_queue_borrowed {
            return;
        }
        self.snapshot_queue();
        self.jam_queue_borrowed = true;
    }

    /// Hands the borrowed queue back, if there was one.
    pub(crate) fn return_queue_after_jam(&mut self) {
        if !self.jam_queue_borrowed {
            return;
        }
        self.jam_queue_borrowed = false;
        self.jam_playing_track = None;
        self.undo_queue();
    }

    pub fn submit_jam_join(&mut self) {
        let Some(code) = self.jam_join_input.take().filter(|c| !c.trim().is_empty()) else {
            return;
        };
        let Some(client) = self.jam_client() else { return };
        self.borrow_queue_for_jam();
        let loads = self.loads.clone();
        tokio::spawn(async move {
            match client.join_jam(&code).await {
                Ok(jam) => {
                    let _ = loads.send(LoadEvent::Jam(jam));
                }
                // A wrong code and an ended jam are the same refusal from the server, and saying
                // more here than it does would undo that.
                Err(_) => {
                    let _ = loads.send(LoadEvent::Jam(None));
                }
            }
        });
    }

    /// Accepts the selected suggestion. Only proposals can be approved; the queue is already in.
    pub fn approve_selected_jam_track(&mut self) {
        let Some(track) = self.selected_proposal_id() else {
            self.status_message = Some("Nothing to accept — that one is already in".into());
            return;
        };
        let Some(client) = self.jam_client() else { return };
        let loads = self.loads.clone();
        tokio::spawn(async move {
            if let Ok(jam) = client.approve_jam_track(&track).await {
                let _ = loads.send(LoadEvent::Jam(jam));
            }
        });
    }

    pub fn remove_selected_jam_track(&mut self) {
        let Some(track) = self.selected_jam_track_id() else { return };
        let Some(client) = self.jam_client() else { return };
        let loads = self.loads.clone();
        tokio::spawn(async move {
            if let Ok(jam) = client.remove_jam_track(&track).await {
                let _ = loads.send(LoadEvent::Jam(jam));
            }
        });
    }

    /// Votes to skip whatever the room is playing.
    pub fn vote_skip_jam_track(&mut self) {
        let Some(client) = self.jam_client() else { return };
        if self.jam.as_ref().and_then(|j| j.now_playing.as_ref()).is_none() {
            self.status_message = Some("Nothing is playing to skip".into());
            return;
        }
        let loads = self.loads.clone();
        tokio::spawn(async move {
            if let Ok(jam) = client.vote_skip_jam_track().await {
                let _ = loads.send(LoadEvent::Jam(jam));
            }
        });
    }

    /// Opens the jam so friends can find it, or shuts it back to code-only. Creator only.
    pub fn toggle_jam_visibility(&mut self) {
        let Some(jam) = self.jam.as_ref() else { return };
        if !jam.is_host {
            self.status_message = Some("Only the creator can open the jam up".into());
            return;
        }
        let next = !jam.open_to_friends;
        let Some(client) = self.jam_client() else { return };
        let loads = self.loads.clone();
        self.status_message = Some(if next {
            "Open to your friends".into()
        } else {
            "Code only".into()
        });
        tokio::spawn(async move {
            if let Ok(jam) = client.set_jam_visibility(next).await {
                let _ = loads.send(LoadEvent::Jam(jam));
            }
        });
    }

    /// Host only. The server refuses everyone else, and the footer only offers it to the host.
    pub fn toggle_jam_mode(&mut self) {
        let Some(jam) = self.jam.as_ref() else { return };
        if !jam.is_host {
            self.status_message = Some("Only the host can change the mode".into());
            return;
        }
        let next = jam.mode.toggled();
        let Some(client) = self.jam_client() else { return };
        let loads = self.loads.clone();
        tokio::spawn(async move {
            if let Ok(jam) = client.set_jam_mode(next).await {
                let _ = loads.send(LoadEvent::Jam(jam));
            }
        });
    }

    /// Leaving, or ending it if you host it — the server decides which, and says so by answering
    /// with no jam.
    pub fn leave_jam(&mut self) {
        let Some(client) = self.jam_client() else { return };
        self.return_queue_after_jam();
        let loads = self.loads.clone();
        tokio::spawn(async move {
            let _ = client.leave_jam().await;
            let _ = loads.send(LoadEvent::Jam(None));
        });
    }

    /// Adds the selected track to the jam, from wherever it is selected.
    pub fn add_selected_to_jam(&mut self) {
        // Whatever the focused pane considers selected — the same set every other "act on this
        // track" key uses, so adding to a jam works from the library, a playlist or the queue.
        let Some(song) = self.selected_songs().into_iter().next() else { return };
        let Some(client) = self.jam_client() else { return };
        if self.jam.is_none() {
            self.status_message = Some("You are not in a jam".into());
            return;
        }
        let loads = self.loads.clone();
        let (uri, title, artist) = (
            crate::integrations::agro::namespaced_id(&song.id),
            song.title.clone(),
            song.artist.clone().unwrap_or_default(),
        );
        // The server advances the room on this, so a track without one would be skipped the moment
        // it started.
        let duration_ms = song.duration as i64 * 1000;
        let democracy = self
            .jam
            .as_ref()
            .is_some_and(|jam| jam.mode == JamMode::Democracy);
        self.status_message = Some(if democracy {
            format!("Suggested “{title}” — waiting on the room")
        } else {
            format!("Added “{title}” to the jam")
        });
        tokio::spawn(async move {
            if let Ok(jam) = client.add_jam_track(&uri, &title, &artist, duration_ms).await {
                let _ = loads.send(LoadEvent::Jam(jam));
            }
        });
    }

    /// Plays what the server says the room is on, at the position it says.
    ///
    /// Nothing here decides anything. Picking a track out of the queue by hand would put this
    /// device out of step with everyone else, which is the whole thing the server clock exists to
    /// prevent.
    pub fn follow_jam_now_playing(&mut self) {
        let Some(now) = self.jam.as_ref().and_then(|jam| jam.now_playing.clone()) else {
            return;
        };
        if self.jam_playing_track.as_deref() == Some(now.track_id.as_str()) {
            return;
        }

        let wanted_title = clean_track_name(&now.title);
        let wanted_artist = clean_track_name(&now.artist);

        // 1. Match against currently loaded tracks list
        let found = self.tracks.iter().position(|song| {
            let s_title = clean_track_name(&song.title);
            let title_match = s_title == wanted_title
                || s_title.contains(&wanted_title)
                || wanted_title.contains(&s_title);
            let artist_match = wanted_artist.is_empty()
                || song
                    .artist
                    .as_deref()
                    .map(|a| {
                        let sa = clean_track_name(a);
                        sa.is_empty() || sa == wanted_artist || sa.contains(&wanted_artist) || wanted_artist.contains(&sa)
                    })
                    .unwrap_or(true);
            title_match && artist_match
        });

        if let Some(index) = found {
            self.jam_playing_track = Some(now.track_id.clone());
            let songs = self.tracks.clone();
            self.player
                .send(crate::player::PlayerCommand::PlayNow { songs, index });
            if now.position_ms > 0 {
                self.player
                    .send(crate::player::PlayerCommand::SeekTo(std::time::Duration::from_millis(
                        now.position_ms as u64,
                    )));
            }
            return;
        }

        // 2. Match against local library index (all local internal sounds and files)
        if let Some(local_song) = self
            .library_root
            .as_ref()
            .and_then(|root| root.local())
            .map(|local| local.index())
            .and_then(|idx| {
                idx.tracks.iter().find(|t| {
                    let t_title = clean_track_name(&t.title);
                    let title_match = t_title == wanted_title
                        || t_title.contains(&wanted_title)
                        || wanted_title.contains(&t_title);
                    let artist_match = wanted_artist.is_empty()
                        || t.artist
                            .as_deref()
                            .map(|a| {
                                let sa = clean_track_name(a);
                                sa.is_empty() || sa == wanted_artist || sa.contains(&wanted_artist) || wanted_artist.contains(&sa)
                            })
                            .unwrap_or(true);
                    title_match && artist_match
                }).map(|t| t.to_song())
            })
        {
            self.jam_playing_track = Some(now.track_id.clone());
            self.player
                .send(crate::player::PlayerCommand::PlayNow {
                    songs: vec![local_song],
                    index: 0,
                });
            if now.position_ms > 0 {
                self.player
                    .send(crate::player::PlayerCommand::SeekTo(std::time::Duration::from_millis(
                        now.position_ms as u64,
                    )));
            }
            return;
        }

        // 3. Match against album, artist, playlist, favorites or queue songs
        let queue_songs = self.player.queue.lock().unwrap().songs().to_vec();
        let other_found = self
            .album_songs
            .iter()
            .chain(self.artist_songs.iter())
            .chain(self.playlist_songs.iter())
            .chain(self.favorites.iter())
            .chain(queue_songs.iter())
            .find(|song| {
                let s_title = clean_track_name(&song.title);
                let title_match = s_title == wanted_title
                    || s_title.contains(&wanted_title)
                    || wanted_title.contains(&s_title);
                let artist_match = wanted_artist.is_empty()
                    || song
                        .artist
                        .as_deref()
                        .map(|a| {
                            let sa = clean_track_name(a);
                            sa.is_empty() || sa == wanted_artist || sa.contains(&wanted_artist) || wanted_artist.contains(&sa)
                        })
                        .unwrap_or(true);
                title_match && artist_match
            })
            .cloned();

        if let Some(song) = other_found {
            self.jam_playing_track = Some(now.track_id.clone());
            self.player
                .send(crate::player::PlayerCommand::PlayNow {
                    songs: vec![song],
                    index: 0,
                });
            if now.position_ms > 0 {
                self.player
                    .send(crate::player::PlayerCommand::SeekTo(std::time::Duration::from_millis(
                        now.position_ms as u64,
                    )));
            }
            return;
        }

        // Not skipped: the room is still playing it, and skipping ahead is what puts a device
        // out of step. This one simply sits the track out.
        self.status_message = Some(format!("You don't have “{}”", now.title));
        self.jam_playing_track = None;
    }

    /// The suggestion under the cursor, if the cursor is on one.
    fn selected_proposal_id(&self) -> Option<String> {
        self.jam
            .as_ref()?
            .proposals
            .get(self.jam_sel)
            .map(|t| t.id.clone())
    }

    fn selected_jam_track_id(&self) -> Option<String> {
        self.jam
            .as_ref()?
            .queue
            .get(self.jam_sel)
            .map(|t| t.id.clone())
    }
}

fn clean_track_name(s: &str) -> String {
    let lower = s.to_lowercase();
    let without_ext = if let Some(idx) = lower.rfind('.') {
        let ext = &lower[idx + 1..];
        if matches!(ext, "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac" | "opus" | "wma" | "alac") {
            &lower[..idx]
        } else {
            &lower
        }
    } else {
        &lower
    };

    without_ext
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
