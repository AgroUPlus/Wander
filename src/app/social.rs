//! Friends, the activity feed, the inbox and the recap, as the TUI drives them.
//!
//! Every one of these is fire-and-forget in the same way the jam actions are: the request goes out
//! on a task, and the answer arrives as a [`LoadEvent::Social`] that replaces the whole local copy.
//! One event for all four surfaces because the tab is drawn as a whole — four separate events would
//! repaint it four times, each with three of its sections a moment out of date.
//!
//! Nothing here is cached to disk. Three of the four are derived on the server from other people's
//! plays, and the switch that permits them can be withdrawn; a copy on this machine would outlive
//! that. The inbox *could* be stored — a drop is durable — but a terminal that is only running
//! while you are looking at it gains little from it, so it is re-read on open like the rest.

use base64::Engine;
use crate::app::App;
use crate::app::types::LoadEvent;
use crate::integrations::agro_social::Recap;

impl App {
    /// The client, or nothing when Agro is not configured.
    fn social_client(&self) -> Option<std::sync::Arc<crate::integrations::agro::AgroClient>> {
        crate::integrations::agro::ACTIVE_CLIENT.get().cloned()
    }

    /// Re-reads everything the Friends tab shows.
    ///
    /// Each surface is allowed to fail on its own and comes back empty when it does. They are
    /// gated by different switches on different accounts, so a friend who has closed their
    /// statistics must not take the feed down with them.
    pub fn refresh_social(&mut self) {
        let Some(client) = self.social_client() else {
            return;
        };
        let (secret, pubkey) = self.config.agro.get_or_create_identity_keys();
        let loads = self.loads.clone();
        let period = self.recap_period.clone();
        let client_clone = client.clone();
        tokio::spawn(async move {
            // Ensure our public key is published on Agro
            let _ = client_clone.set_public_key(&base64::prelude::BASE64_STANDARD.encode(pubkey.as_bytes())).await;
            let friends = client.friends().await.unwrap_or_default();
            let feed = client.friend_activity().await.unwrap_or_default();
            let inbox = client.inbox(Some(&secret)).await.unwrap_or_default();
            let recap = client
                .circle_recap(&period)
                .await
                .unwrap_or_else(|_| Recap::default());
            let _ = loads.send(LoadEvent::Social {
                friends,
                feed,
                inbox,
                recap,
            });
        });
    }

    /// Marks the selected drop read, then re-reads.
    ///
    /// Reading is what opening it means, so there is no separate confirmation. A drop that is not
    /// this account's to mark answers `false` and simply changes nothing.
    pub fn read_selected_drop(&mut self) {
        let Some(client) = self.social_client() else {
            return;
        };
        let Some(drop) = self.inbox.get(self.inbox_sel).cloned() else {
            return;
        };
        let loads = self.loads.clone();
        let period = self.recap_period.clone();
        let (secret, _) = self.config.agro.get_or_create_identity_keys();
        tokio::spawn(async move {
            if client.mark_drop_read(&drop.id).await.unwrap_or(false) {
                refresh_into(client, loads, period, Some(secret)).await;
            }
        });
    }

    /// Archives the selected drop and removes it from the list.
    pub fn archive_selected_drop(&mut self) {
        let Some(client) = self.social_client() else {
            return;
        };
        let Some(drop) = self.inbox.get(self.inbox_sel).cloned() else {
            return;
        };
        let loads = self.loads.clone();
        let period = self.recap_period.clone();
        let (secret, _) = self.config.agro.get_or_create_identity_keys();
        tokio::spawn(async move {
            if client.archive_drop(&drop.id).await.unwrap_or(false) {
                refresh_into(client, loads, period, Some(secret)).await;
            }
        });
    }

    /// Opens the prompt for a note to send with the selected track.
    ///
    /// The track comes from [`Self::selected_songs`], the same set every other "act on this track"
    /// key uses, so a drop can be sent from the library, a playlist or the queue — not only from
    /// whatever happens to be playing.
    ///
    /// The friend is chosen first, from the list on screen, so by the time this runs the only
    /// question left is what to say — which is why it is a single prompt rather than a dialog.
    pub fn prompt_drop_to_selected_friend(&mut self) {
        if self.selected_songs().is_empty() {
            self.status_message = Some("No track selected to send".into());
            return;
        }
        let Some(friend) = self.friends.get(self.social_sel) else {
            self.status_message = Some("No friend selected".into());
            return;
        };
        self.drop_target = Some(friend.username.clone());
        self.drop_note_input = Some(String::new());
        self.status_message = Some(format!(
            "Send to {}: type a note, then Enter (Esc cancels)",
            friend.label()
        ));
    }

    /// Sends the pending drop, with whatever note was typed.
    pub fn confirm_drop(&mut self) {
        self.submit_drop();
    }

    /// Submits the drop being typed in the modal.
    pub fn submit_drop(&mut self) {
        let (Some(to), Some(note)) = (self.drop_target.take(), self.drop_note_input.take()) else {
            return;
        };
        let Some(client) = self.social_client() else {
            return;
        };
        let Some(song) = self.selected_songs().into_iter().next() else {
            self.status_message = Some("No track selected to send".into());
            return;
        };

        let recipient_pubkey = self
            .friends
            .iter()
            .find(|f| f.username.eq_ignore_ascii_case(&to))
            .and_then(|f| f.public_key.clone());

        let (secret, _) = self.config.agro.get_or_create_identity_keys();
        let loads = self.loads.clone();
        let period = self.recap_period.clone();
        self.status_message = Some(format!("Sending E2EE drop to {to}…"));
        tokio::spawn(async move {
            let outcome = client
                .drop_track(
                    &to,
                    &song.title,
                    song.artist.as_deref().unwrap_or_default(),
                    song.album.as_deref(),
                    Some(&crate::integrations::agro::namespaced_id(&song.id)),
                    Some(note.as_str()),
                    recipient_pubkey.as_deref(),
                )
                .await;
            // Sent drops show in the inbox tab's own list, so the answer is worth re-reading
            // whether it succeeded or not.
            if outcome.is_ok() {
                refresh_into(client, loads, period, Some(secret)).await;
            }
        });
    }

    /// Cancels a pending drop without sending it.
    pub fn cancel_drop(&mut self) {
        self.drop_note_input = None;
        self.drop_target = None;
        self.status_message = None;
    }

    /// Purges scrobbles for a specific year or all time from Agro.
    pub fn purge_scrobbles(&mut self, year: Option<i32>) {
        let Some(client) = self.social_client() else {
            return;
        };
        let status = if let Some(y) = year {
            format!("Purging scrobbles for {y}…")
        } else {
            "Purging all scrobbles…".to_string()
        };
        self.status_message = Some(status);
        tokio::spawn(async move {
            let _ = client.purge_scrobbles(year, None).await;
        });
    }
}

/// The refresh, as a free function, so the actions above can chain one after a mutation without
/// borrowing `self` across an await.
async fn refresh_into(
    client: std::sync::Arc<crate::integrations::agro::AgroClient>,
    loads: tokio::sync::mpsc::UnboundedSender<LoadEvent>,
    period: String,
    secret: Option<x25519_dalek::StaticSecret>,
) {
    let friends = client.friends().await.unwrap_or_default();
    let feed = client.friend_activity().await.unwrap_or_default();
    let inbox = client.inbox(secret.as_ref()).await.unwrap_or_default();
    let recap = client.circle_recap(&period).await.unwrap_or_default();
    let _ = loads.send(LoadEvent::Social {
        friends,
        feed,
        inbox,
        recap,
    });
}

impl App {
    /// Steps the recap through the periods the server offers, then re-reads it.
    ///
    /// A cycle rather than four keys: the recap is one panel among four on this tab, and three more
    /// bindings for something you change occasionally is not worth the keyboard.
    pub fn cycle_recap_period(&mut self) {
        self.recap_period = match self.recap_period.as_str() {
            "WEEK" => "MONTH",
            "MONTH" => "YEAR",
            "YEAR" => "ALL",
            _ => "WEEK",
        }
        .to_string();
        self.refresh_social();
    }
}
