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

use crate::app::types::LoadEvent;
use crate::app::App;
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
        let Some(client) = self.social_client() else { return };
        let loads = self.loads.clone();
        let period = self.recap_period.clone();
        tokio::spawn(async move {
            let friends = client.friends().await.unwrap_or_default();
            let feed = client.friend_activity().await.unwrap_or_default();
            let inbox = client.inbox().await.unwrap_or_default();
            let recap = client.circle_recap(&period).await.unwrap_or_else(|_| Recap::default());
            let _ = loads.send(LoadEvent::Social { friends, feed, inbox, recap });
        });
    }

    /// Marks the selected drop read, then re-reads.
    ///
    /// Reading is what opening it means, so there is no separate confirmation. A drop that is not
    /// this account's to mark answers `false` and simply changes nothing.
    pub fn read_selected_drop(&mut self) {
        let Some(client) = self.social_client() else { return };
        let Some(drop) = self.inbox.get(self.inbox_sel).cloned() else { return };
        if !drop.is_unread() {
            return;
        }
        let loads = self.loads.clone();
        let period = self.recap_period.clone();
        tokio::spawn(async move {
            let _ = client.mark_drop_read(&drop.id).await;
            refresh_into(client, loads, period).await;
        });
    }

    /// Takes the selected drop out of the inbox.
    pub fn archive_selected_drop(&mut self) {
        let Some(client) = self.social_client() else { return };
        let Some(drop) = self.inbox.get(self.inbox_sel).cloned() else { return };
        let loads = self.loads.clone();
        let period = self.recap_period.clone();
        self.status_message = Some(format!("Archived “{}”", drop.track_title));
        tokio::spawn(async move {
            let _ = client.archive_drop(&drop.id).await;
            refresh_into(client, loads, period).await;
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
        let note = self.drop_note_input.take().unwrap_or_default();
        let Some(to) = self.drop_target.take() else { return };
        let Some(client) = self.social_client() else { return };
        let Some(song) = self.selected_songs().into_iter().next() else {
            self.status_message = Some("No track selected to send".into());
            return;
        };

        let loads = self.loads.clone();
        let period = self.recap_period.clone();
        self.status_message = Some(format!("Sending to {to}…"));
        tokio::spawn(async move {
            let outcome = client
                .drop_track(
                    &to,
                    &song.title,
                    song.artist.as_deref().unwrap_or_default(),
                    song.album.as_deref(),
                    Some(&crate::integrations::agro::namespaced_id(&song.id)),
                    Some(note.as_str()),
                )
                .await;
            // Sent drops show in the inbox tab's own list, so the answer is worth re-reading
            // whether it succeeded or not.
            if outcome.is_ok() {
                refresh_into(client, loads, period).await;
            }
        });
    }

    /// Cancels a pending drop without sending it.
    pub fn cancel_drop(&mut self) {
        self.drop_note_input = None;
        self.drop_target = None;
        self.status_message = None;
    }
}

/// The refresh, as a free function, so the actions above can chain one after a mutation without
/// borrowing `self` across an await.
async fn refresh_into(
    client: std::sync::Arc<crate::integrations::agro::AgroClient>,
    loads: tokio::sync::mpsc::UnboundedSender<LoadEvent>,
    period: String,
) {
    let friends = client.friends().await.unwrap_or_default();
    let feed = client.friend_activity().await.unwrap_or_default();
    let inbox = client.inbox().await.unwrap_or_default();
    let recap = client.circle_recap(&period).await.unwrap_or_default();
    let _ = loads.send(LoadEvent::Social { friends, feed, inbox, recap });
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
