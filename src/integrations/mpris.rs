//! MPRIS D-Bus integration.
//!
//! Exposing the standard `org.mpris.MediaPlayer2` interface is what makes
//! wander show up in the caelestia bar and lock screen — that shell reads
//! plain MPRIS (`Quickshell.Services.Mpris`), so no shell-specific code is
//! needed. It also brings `playerctl` and media keys along for free.

use anyhow::{Context, Result};
use mpris_server::zbus::fdo;
use mpris_server::{
    LoopStatus, Metadata, PlaybackRate, PlaybackStatus, PlayerInterface, Property, RootInterface,
    Server, Signal, Time, TrackId, Volume,
};
use std::sync::Arc;
use std::time::Duration;

use crate::app::COVER_SIZE;
use crate::player::queue::Repeat;
use crate::player::{PlayerCommand, PlayerHandle};
use crate::subsonic::cache::CoverCache;

/// How often playback state is polled to publish D-Bus property changes.
const POLL: Duration = Duration::from_millis(500);
/// Position changes smaller than this are treated as normal playback rather
/// than a seek, so we don't spam `Seeked` every tick.
const SEEK_EPSILON: Duration = Duration::from_secs(2);

pub struct MprisPlayer {
    player: PlayerHandle,
    covers: Arc<CoverCache>,
}

impl MprisPlayer {
    fn metadata(&self) -> Metadata {
        let status = self.player.status();
        let Some(song) = status.current else {
            return Metadata::new();
        };

        let mut metadata = Metadata::builder()
            .title(song.title.clone())
            .artist([song.artist_or_unknown().to_string()])
            .album(song.album_or_unknown().to_string())
            .length(Time::from_secs(song.duration as i64))
            .build();

        if let Ok(track_id) = TrackId::try_from(track_path(&song.id)) {
            metadata.set_trackid(Some(track_id));
        }

        // Point at the on-disk cache rather than the Navidrome URL: the latter
        // carries our auth token, and D-Bus metadata is readable by any
        // process on the session bus.
        if let Some(cover_id) = song.cover_art.as_deref()
            && let Some(path) = self.covers.cached_path(cover_id, COVER_SIZE)
            && let Some(path) = path.to_str()
        {
            metadata.set_art_url(Some(format!("file://{path}")));
        }

        metadata
    }
}

impl RootInterface for MprisPlayer {
    async fn raise(&self) -> fdo::Result<()> {
        Ok(())
    }

    async fn quit(&self) -> fdo::Result<()> {
        Ok(())
    }

    async fn can_quit(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn set_fullscreen(&self, _: bool) -> mpris_server::zbus::Result<()> {
        Ok(())
    }

    async fn can_set_fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn can_raise(&self) -> fdo::Result<bool> {
        // We are a terminal app with no window to focus.
        Ok(false)
    }

    async fn has_track_list(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn identity(&self) -> fdo::Result<String> {
        Ok("wander".to_string())
    }

    async fn desktop_entry(&self) -> fdo::Result<String> {
        Ok("wander".to_string())
    }

    async fn supported_uri_schemes(&self) -> fdo::Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn supported_mime_types(&self) -> fdo::Result<Vec<String>> {
        Ok(Vec::new())
    }
}

impl PlayerInterface for MprisPlayer {
    async fn next(&self) -> fdo::Result<()> {
        self.player.send(PlayerCommand::Next);
        Ok(())
    }

    async fn previous(&self) -> fdo::Result<()> {
        self.player.send(PlayerCommand::Prev);
        Ok(())
    }

    async fn pause(&self) -> fdo::Result<()> {
        if !self.player.is_paused() {
            self.player.send(PlayerCommand::TogglePause);
        }
        Ok(())
    }

    async fn play_pause(&self) -> fdo::Result<()> {
        self.player.send(PlayerCommand::TogglePause);
        Ok(())
    }

    async fn stop(&self) -> fdo::Result<()> {
        self.player.send(PlayerCommand::Stop);
        Ok(())
    }

    async fn play(&self) -> fdo::Result<()> {
        if self.player.is_paused() {
            self.player.send(PlayerCommand::TogglePause);
        }
        Ok(())
    }

    async fn seek(&self, offset: Time) -> fdo::Result<()> {
        let current = self.player.elapsed().as_secs_f64();
        let target = (current + offset.as_micros() as f64 / 1e6).max(0.0);
        self.player
            .send(PlayerCommand::SeekTo(Duration::from_secs_f64(target)));
        Ok(())
    }

    async fn set_position(&self, _track_id: TrackId, position: Time) -> fdo::Result<()> {
        let secs = (position.as_micros() as f64 / 1e6).max(0.0);
        self.player
            .send(PlayerCommand::SeekTo(Duration::from_secs_f64(secs)));
        Ok(())
    }

    async fn open_uri(&self, _uri: String) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported("wander cannot open URIs".into()))
    }

    async fn playback_status(&self) -> fdo::Result<PlaybackStatus> {
        let status = self.player.status();
        Ok(if status.current.is_none() {
            PlaybackStatus::Stopped
        } else if self.player.is_paused() {
            PlaybackStatus::Paused
        } else {
            PlaybackStatus::Playing
        })
    }

    async fn loop_status(&self) -> fdo::Result<LoopStatus> {
        Ok(match self.player.queue.lock().unwrap().repeat {
            Repeat::Off => LoopStatus::None,
            Repeat::All => LoopStatus::Playlist,
            Repeat::One => LoopStatus::Track,
        })
    }

    async fn set_loop_status(&self, status: LoopStatus) -> mpris_server::zbus::Result<()> {
        let wanted = match status {
            LoopStatus::None => Repeat::Off,
            LoopStatus::Playlist => Repeat::All,
            LoopStatus::Track => Repeat::One,
        };
        self.player.send(PlayerCommand::SetRepeat(wanted));
        Ok(())
    }

    async fn rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn set_rate(&self, _: PlaybackRate) -> mpris_server::zbus::Result<()> {
        Ok(())
    }

    async fn shuffle(&self) -> fdo::Result<bool> {
        Ok(self.player.queue.lock().unwrap().shuffle)
    }

    async fn set_shuffle(&self, shuffle: bool) -> mpris_server::zbus::Result<()> {
        let current = { self.player.queue.lock().unwrap().shuffle };
        if current != shuffle {
            self.player.send(PlayerCommand::ToggleShuffle);
        }
        Ok(())
    }

    async fn metadata(&self) -> fdo::Result<Metadata> {
        Ok(self.metadata())
    }

    async fn volume(&self) -> fdo::Result<Volume> {
        Ok(self.player.volume() as f64)
    }

    async fn set_volume(&self, volume: Volume) -> mpris_server::zbus::Result<()> {
        self.player
            .send(PlayerCommand::SetVolume(volume.clamp(0.0, 1.0) as f32));
        Ok(())
    }

    async fn position(&self) -> fdo::Result<Time> {
        Ok(Time::from_micros(self.player.elapsed().as_micros() as i64))
    }

    async fn minimum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn maximum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn can_go_next(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_go_previous(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_play(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_pause(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_seek(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_control(&self) -> fdo::Result<bool> {
        Ok(true)
    }
}

/// Claim the MPRIS bus name and keep its properties in sync with playback.
///
/// Failing to reach D-Bus is not fatal: the player must keep working on a
/// system with no session bus.
pub async fn spawn(player: PlayerHandle, covers: Arc<CoverCache>) -> Result<()> {
    let server = Server::new("wander", MprisPlayer { player, covers })
        .await
        .context("claiming the MPRIS bus name")?;

    tokio::spawn(async move {
        let mut last: Option<Snapshot> = None;

        loop {
            tokio::time::sleep(POLL).await;

            let now = Snapshot::capture(server.imp()).await;
            let Some(previous) = last.replace(now.clone()) else {
                continue;
            };

            let mut changed = Vec::new();
            if previous.status != now.status {
                changed.push(Property::PlaybackStatus(now.status));
            }
            if previous.track != now.track {
                changed.push(Property::Metadata(server.imp().metadata()));
            }
            if (previous.volume - now.volume).abs() > f64::EPSILON {
                changed.push(Property::Volume(now.volume));
            }
            if previous.shuffle != now.shuffle {
                changed.push(Property::Shuffle(now.shuffle));
            }
            if previous.repeat != now.repeat {
                changed.push(Property::LoopStatus(now.repeat));
            }

            if !changed.is_empty() {
                let _ = server.properties_changed(changed).await;
            }

            // A position jump larger than the poll interval means a seek, which
            // clients need told explicitly since Position is not a notifying
            // property.
            let drift = now.position.abs_diff(previous.position);
            if drift > SEEK_EPSILON.as_micros() as u64 {
                let _ = server
                    .emit(Signal::Seeked {
                        position: Time::from_micros(now.position as i64),
                    })
                    .await;
            }
        }
    });

    Ok(())
}

/// The subset of state worth comparing between polls.
#[derive(Clone, PartialEq)]
struct Snapshot {
    status: PlaybackStatus,
    track: Option<String>,
    volume: f64,
    shuffle: bool,
    repeat: LoopStatus,
    position: u64,
}

impl Snapshot {
    async fn capture(imp: &MprisPlayer) -> Self {
        // Read every lock-guarded value into a local *before* any await. A
        // MutexGuard held across an await point would make this future non-Send
        // and could deadlock the player task.
        let (shuffle, repeat) = {
            let queue = imp.player.queue.lock().unwrap();
            (queue.shuffle, queue.repeat)
        };
        let track = imp.player.status().current.map(|song| song.id);
        let volume = imp.player.volume() as f64;
        let position = imp.player.elapsed().as_micros() as u64;

        let status = imp
            .playback_status()
            .await
            .unwrap_or(PlaybackStatus::Stopped);

        Self {
            status,
            track,
            volume,
            shuffle,
            repeat: match repeat {
                Repeat::Off => LoopStatus::None,
                Repeat::All => LoopStatus::Playlist,
                Repeat::One => LoopStatus::Track,
            },
            position,
        }
    }
}

/// Build a valid D-Bus object path for a song.
///
/// Subsonic IDs are opaque strings that may contain characters a D-Bus object
/// path forbids, so anything outside `[A-Za-z0-9_]` becomes an underscore.
fn track_path(song_id: &str) -> String {
    let safe: String = song_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("/xyz/wander/track/{safe}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_paths_are_valid_dbus_object_paths() {
        let path = track_path("al-123/../weird id");
        assert!(
            TrackId::try_from(path.clone()).is_ok(),
            "{path} was rejected"
        );
        assert!(path.starts_with('/'));
        assert!(!path.contains(' '));
    }

    #[test]
    fn track_paths_are_stable_and_distinct() {
        assert_eq!(track_path("abc"), track_path("abc"));
        assert_ne!(track_path("abc"), track_path("abd"));
    }

    #[test]
    fn typical_navidrome_ids_survive_unchanged() {
        // Navidrome IDs are alphanumeric, so they should pass through intact.
        assert_eq!(
            track_path("tH0B3eAdbPRjI24do8WXHf"),
            "/xyz/wander/track/tH0B3eAdbPRjI24do8WXHf"
        );
    }
}
