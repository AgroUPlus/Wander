//! The social surfaces the TUI reads: friends, the activity feed, the recap, and the drop inbox.
//!
//! Wander had none of these — the only social feature it carried was the jam, which is a room you
//! are in rather than a graph you belong to. Everything here is a read of somebody else's data,
//! which means every one of them can legitimately come back empty: each surface is gated on a
//! switch on the *subject's* account, and those default closed. An empty feed is the normal state
//! of a server whose users have not opted in, not a failure worth reporting.
//!
//! Sending a drop is the exception — it writes — and it is refused for a stranger with exactly the
//! same message the server uses for an account that does not exist. There is nothing more specific
//! to tell the user than that it did not go.

use anyhow::Result;
use serde_json::json;

use super::agro::AgroClient;

const DROP_FIELDS: &str = "id fromUser toUser trackTitle artistName albumName note createdAt readAt";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Friend {
    pub username: String,
    pub display_name: Option<String>,
    /// What they are playing, when they let that be seen.
    pub now_playing: Option<String>,
}

impl Friend {
    /// What to put on screen. Falls back to the username, which always exists.
    pub fn label(&self) -> &str {
        self.display_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&self.username)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drop {
    pub id: String,
    pub from_user: String,
    pub to_user: String,
    pub track_title: String,
    pub artist_name: String,
    pub note: Option<String>,
    pub created_at: String,
    /// Always `None` on a drop this account sent — the server blanks it, because whether somebody
    /// opened what you gave them is information about them. Do not draw a "seen" marker from this.
    pub read_at: Option<String>,
}

impl Drop {
    pub fn is_unread(&self) -> bool {
        self.read_at.is_none()
    }
}

/// One line of the activity feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedItem {
    pub username: String,
    /// The sentence to show, composed by the server so every client says the same thing.
    pub summary: String,
    pub at: String,
}

/// The circle's shared recap, reduced to the parts a terminal can show usefully.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recap {
    pub members: Vec<String>,
    pub anthem: Option<String>,
    pub trendsetter: Option<String>,
    /// `alice & bob — 72%`, already formatted.
    pub matrix: Vec<String>,
}

fn string(value: &serde_json::Value, key: &str) -> Option<String> {
    value[key]
        .as_str()
        .map(str::to_string)
        .filter(|text| !text.is_empty())
}

fn parse_drop(value: &serde_json::Value) -> Option<Drop> {
    Some(Drop {
        id: string(value, "id")?,
        from_user: string(value, "fromUser").unwrap_or_default(),
        to_user: string(value, "toUser").unwrap_or_default(),
        track_title: string(value, "trackTitle").unwrap_or_default(),
        artist_name: string(value, "artistName").unwrap_or_default(),
        note: string(value, "note"),
        created_at: string(value, "createdAt").unwrap_or_default(),
        read_at: string(value, "readAt"),
    })
}

impl AgroClient {
    /// Accepted friends, with whatever each is playing that they allow to be seen.
    pub async fn friends(&self) -> Result<Vec<Friend>> {
        let answer = self
            .graphql(&json!({
                "query": "{ friends { profile { username displayName } nowPlaying { trackTitle artistName } } }"
            }))
            .await?;
        Ok(answer["data"]["friends"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| {
                        let profile = &row["profile"];
                        Some(Friend {
                            username: string(profile, "username")?,
                            display_name: string(profile, "displayName"),
                            now_playing: string(&row["nowPlaying"], "trackTitle").map(|title| {
                                match string(&row["nowPlaying"], "artistName") {
                                    Some(artist) => format!("{title} — {artist}"),
                                    None => title,
                                }
                            }),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// What friends have been into lately. Only those who opened `showActivity` appear.
    pub async fn friend_activity(&self) -> Result<Vec<FeedItem>> {
        let answer = self
            .graphql(&json!({
                "query": "{ friendActivity { username summary at } }"
            }))
            .await?;
        Ok(answer["data"]["friendActivity"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| {
                        Some(FeedItem {
                            username: string(row, "username")?,
                            summary: string(row, "summary")?,
                            at: string(row, "at").unwrap_or_default(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The circle's recap for a period, flattened into lines this client can print.
    pub async fn circle_recap(&self, period: &str) -> Result<Recap> {
        let answer = self
            .graphql(&json!({
                "query": "query R($p: String) { circleRecap(period: $p) { \
                          members \
                          anthem { title artist plays } \
                          trendsetter { username firsts } \
                          matrix { a b score } } }",
                "variables": { "p": period }
            }))
            .await?;
        let recap = &answer["data"]["circleRecap"];
        Ok(Recap {
            members: recap["members"]
                .as_array()
                .map(|names| {
                    names
                        .iter()
                        .filter_map(|name| name.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            anthem: string(&recap["anthem"], "title").map(|title| {
                let artist = string(&recap["anthem"], "artist").unwrap_or_default();
                let plays = recap["anthem"]["plays"].as_i64().unwrap_or(0);
                format!("{title} — {artist} ({plays} plays)")
            }),
            trendsetter: string(&recap["trendsetter"], "username").map(|who| {
                let firsts = recap["trendsetter"]["firsts"].as_i64().unwrap_or(0);
                format!("{who}, first to {firsts}")
            }),
            matrix: recap["matrix"]
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .filter_map(|row| {
                            let a = row["a"].as_str()?;
                            let b = row["b"].as_str()?;
                            let score = row["score"].as_i64().unwrap_or(0);
                            Some(format!("{a} & {b} — {score}%"))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// Drops sent to this account and not yet archived.
    pub async fn inbox(&self) -> Result<Vec<Drop>> {
        let answer = self
            .graphql(&json!({
                "query": format!("{{ inbox {{ {DROP_FIELDS} }} }}")
            }))
            .await?;
        Ok(answer["data"]["inbox"]
            .as_array()
            .map(|rows| rows.iter().filter_map(parse_drop).collect())
            .unwrap_or_default())
    }

    /// Hands a track to a friend.
    ///
    /// `track_uri` is this device's namespaced identifier when it has one. It is inert on a client
    /// that does not share the same backend, which is why the title and artist are what actually
    /// carry the message.
    pub async fn drop_track(
        &self,
        to: &str,
        title: &str,
        artist: &str,
        album: Option<&str>,
        track_uri: Option<&str>,
        note: Option<&str>,
    ) -> Result<()> {
        let answer = self
            .graphql(&json!({
                "query": format!(
                    "mutation D($to: String!, $t: String!, $a: String!, $al: String, \
                     $u: String, $n: String) {{ \
                     dropTrack(to: $to, trackTitle: $t, artistName: $a, albumName: $al, \
                     trackUri: $u, note: $n) {{ {DROP_FIELDS} }} }}"
                ),
                "variables": {
                    "to": to.trim().to_lowercase(),
                    "t": title,
                    "a": artist,
                    "al": album,
                    "u": track_uri,
                    "n": note.filter(|text| !text.trim().is_empty()),
                }
            }))
            .await?;
        if let Some(message) = answer["errors"][0]["message"].as_str() {
            anyhow::bail!("{message}");
        }
        Ok(())
    }

    /// Marks a drop read. Answers `false` for one that is not this account's to mark.
    pub async fn mark_drop_read(&self, id: &str) -> Result<bool> {
        let answer = self
            .graphql(&json!({
                "query": "mutation M($id: String!) { markDropRead(id: $id) }",
                "variables": { "id": id }
            }))
            .await?;
        Ok(answer["data"]["markDropRead"].as_bool().unwrap_or(false))
    }

    /// Takes a drop out of the inbox. The sender's record of having sent it survives.
    pub async fn archive_drop(&self, id: &str) -> Result<bool> {
        let answer = self
            .graphql(&json!({
                "query": "mutation A($id: String!) { archiveDrop(id: $id) }",
                "variables": { "id": id }
            }))
            .await?;
        Ok(answer["data"]["archiveDrop"].as_bool().unwrap_or(false))
    }
}
