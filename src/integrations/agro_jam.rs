//! Jam sessions, as the TUI needs them.
//!
//! A jam is one queue several people build together — distinct from listen-along, which mirrors a
//! single person's playback. Every mutation answers with the whole jam, so there is never a
//! fragment to reconcile against a stale local copy: the answer replaces what was there.

use anyhow::Result;
use serde_json::json;

use super::agro::AgroClient;

/// How the next track is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JamMode {
    /// Plays in the order added.
    Open,
    /// Most votes plays next.
    Democracy,
}

impl JamMode {
    pub fn as_str(self) -> &'static str {
        match self {
            JamMode::Open => "open",
            JamMode::Democracy => "democracy",
        }
    }

    fn parse(raw: &str) -> Self {
        if raw.eq_ignore_ascii_case("open") {
            JamMode::Open
        } else {
            JamMode::Democracy
        }
    }

    /// What the host switches to when they toggle.
    pub fn toggled(self) -> Self {
        match self {
            JamMode::Open => JamMode::Democracy,
            JamMode::Democracy => JamMode::Open,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            JamMode::Open => "free for all",
            JamMode::Democracy => "democracy",
        }
    }
}

#[derive(Debug, Clone)]
pub struct JamTrack {
    pub id: String,
    pub added_by: String,
    pub title: String,
    pub artist: String,
    /// Approvals so far, not counting whoever suggested it.
    pub approvals: i64,
    /// Whether this account has approved it, so the row states it rather than guessing.
    pub approved: bool,
    /// How many more approvals it needs before it joins the queue.
    pub still_needed: i64,
}

/// What the whole room is hearing, decided by the server.
#[derive(Debug, Clone)]
pub struct JamNowPlaying {
    pub track_id: String,
    pub title: String,
    pub artist: String,
    pub duration_ms: i64,
    /// Where the room is now, so a device joining late lands in the right place.
    pub position_ms: i64,
    pub skip_votes: i64,
    pub skips_needed: i64,
    pub you_skipped: bool,
}

#[derive(Debug, Clone)]
pub struct Jam {
    pub code: String,
    pub host: String,
    pub mode: JamMode,
    pub is_host: bool,
    pub members: Vec<String>,
    /// Accepted tracks, in the order added. Excludes whatever is playing.
    pub queue: Vec<JamTrack>,
    /// Suggestions the room has not accepted. Always empty in `open` mode.
    pub proposals: Vec<JamTrack>,
    pub now_playing: Option<JamNowPlaying>,
    pub approvals_needed: i64,
    /// Whether friends can find this jam without being handed the code.
    pub open_to_friends: bool,
}

const TRACK_FIELDS: &str = "id addedBy title artist approvals approved stillNeeded";

const JAM_FIELDS: &str = "id code host mode isHost members approvalsNeeded \
     queue { id addedBy title artist approvals approved stillNeeded } \
     proposals { id addedBy title artist approvals approved stillNeeded } \
     visibility \
     nowPlaying { trackId title artist durationMs positionMs skipVotes skipsNeeded youSkipped }";

fn parse_tracks(value: &serde_json::Value) -> Vec<JamTrack> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|t| JamTrack {
                    id: t["id"].as_str().unwrap_or_default().to_string(),
                    added_by: t["addedBy"].as_str().unwrap_or_default().to_string(),
                    title: t["title"].as_str().unwrap_or_default().to_string(),
                    artist: t["artist"].as_str().unwrap_or_default().to_string(),
                    approvals: t["approvals"].as_i64().unwrap_or(0),
                    approved: t["approved"].as_bool().unwrap_or(false),
                    still_needed: t["stillNeeded"].as_i64().unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_jam(value: &serde_json::Value) -> Option<Jam> {
    if value.is_null() {
        return None;
    }
    Some(Jam {
        code: value["code"].as_str().unwrap_or_default().to_string(),
        host: value["host"].as_str().unwrap_or_default().to_string(),
        mode: JamMode::parse(value["mode"].as_str().unwrap_or("democracy")),
        is_host: value["isHost"].as_bool().unwrap_or(false),
        members: value["members"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| m.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        queue: parse_tracks(&value["queue"]),
        proposals: parse_tracks(&value["proposals"]),
        now_playing: value["nowPlaying"].as_object().map(|now| JamNowPlaying {
            track_id: now["trackId"].as_str().unwrap_or_default().to_string(),
            title: now["title"].as_str().unwrap_or_default().to_string(),
            artist: now["artist"].as_str().unwrap_or_default().to_string(),
            duration_ms: now["durationMs"].as_i64().unwrap_or(0),
            position_ms: now["positionMs"].as_i64().unwrap_or(0),
            skip_votes: now["skipVotes"].as_i64().unwrap_or(0),
            skips_needed: now["skipsNeeded"].as_i64().unwrap_or(1),
            you_skipped: now["youSkipped"].as_bool().unwrap_or(false),
        }),
        approvals_needed: value["approvalsNeeded"].as_i64().unwrap_or(1),
        open_to_friends: value["visibility"].as_str().unwrap_or("code") == "friends",
    })
}

impl AgroClient {
    /// The jam this account is in, or `None`. There is at most one.
    pub async fn jam(&self) -> Result<Option<Jam>> {
        let answer = self
            .graphql(&json!({ "query": format!("query {{ jam {{ {JAM_FIELDS} }} }}") }))
            .await?;
        Ok(parse_jam(&answer["data"]["jam"]))
    }

    async fn jam_mutation(&self, body: serde_json::Value, field: &str) -> Result<Option<Jam>> {
        let answer = self.graphql(&body).await?;
        if let Some(message) = answer["errors"][0]["message"].as_str() {
            anyhow::bail!("{message}");
        }
        Ok(parse_jam(&answer["data"][field]))
    }

    pub async fn create_jam(&self, mode: JamMode) -> Result<Option<Jam>> {
        self.jam_mutation(
            json!({
                "query": format!("mutation C($m: String) {{ createJam(mode: $m) {{ {JAM_FIELDS} }} }}"),
                "variables": { "m": mode.as_str() }
            }),
            "createJam",
        )
        .await
    }

    pub async fn join_jam(&self, code: &str) -> Result<Option<Jam>> {
        self.jam_mutation(
            json!({
                "query": format!("mutation J($c: String!) {{ joinJam(code: $c) {{ {JAM_FIELDS} }} }}"),
                "variables": { "c": code.trim().to_uppercase() }
            }),
            "joinJam",
        )
        .await
    }

    pub async fn leave_jam(&self) -> Result<()> {
        self.graphql(&json!({ "query": "mutation { leaveJam }" })).await?;
        Ok(())
    }

    /// Accepts somebody's suggestion. One-way: there is no un-approving.
    pub async fn approve_jam_track(&self, track_id: &str) -> Result<Option<Jam>> {
        self.jam_mutation(
            json!({
                "query": format!("mutation A($id: String!) {{ approveJamTrack(trackId: $id) {{ {JAM_FIELDS} }} }}"),
                "variables": { "id": track_id }
            }),
            "approveJamTrack",
        )
        .await
    }

    pub async fn remove_jam_track(&self, track_id: &str) -> Result<Option<Jam>> {
        self.jam_mutation(
            json!({
                "query": format!("mutation R($id: String!) {{ removeJamTrack(trackId: $id) {{ {JAM_FIELDS} }} }}"),
                "variables": { "id": track_id }
            }),
            "removeJamTrack",
        )
        .await
    }

    pub async fn set_jam_mode(&self, mode: JamMode) -> Result<Option<Jam>> {
        self.jam_mutation(
            json!({
                "query": format!("mutation M($m: String!) {{ setJamMode(mode: $m) {{ {JAM_FIELDS} }} }}"),
                "variables": { "m": mode.as_str() }
            }),
            "setJamMode",
        )
        .await
    }

    /// Adds a track to the shared queue.
    /// Votes to skip whatever is playing. A majority of the room retires it at once.
    pub async fn vote_skip_jam_track(&self) -> Result<Option<Jam>> {
        self.jam_mutation(
            json!({ "query": format!("mutation {{ voteSkipJamTrack {{ {JAM_FIELDS} }} }}") }),
            "voteSkipJamTrack",
        )
        .await
    }

    /// Opens the jam to friends, or shuts it back to code-only. Creator only.
    pub async fn set_jam_visibility(&self, open_to_friends: bool) -> Result<Option<Jam>> {
        self.jam_mutation(
            json!({
                "query": format!("mutation V($v: String!) {{ setJamVisibility(visibility: $v) {{ {JAM_FIELDS} }} }}"),
                "variables": { "v": if open_to_friends { "friends" } else { "code" } }
            }),
            "setJamVisibility",
        )
        .await
    }

    /// Suggests a track. The duration matters: the server advances the room on it.
    pub async fn add_jam_track(
        &self,
        track_uri: &str,
        title: &str,
        artist: &str,
        duration_ms: i64,
    ) -> Result<Option<Jam>> {
        self.jam_mutation(
            json!({
                "query": format!(
                    "mutation A($u: String!, $t: String!, $a: String!, $d: Int) \
                     {{ addJamTrack(trackUri: $u, title: $t, artist: $a, durationMs: $d) \
                        {{ {JAM_FIELDS} }} }}"
                ),
                "variables": { "u": track_uri, "t": title, "a": artist, "d": duration_ms }
            }),
            "addJamTrack",
        )
        .await
    }
}
