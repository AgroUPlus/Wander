//! A status-bar icon, for when the terminal is not the thing you are looking at.
//!
//! Wander is a TUI, which is exactly why this earns its place: the window it lives in is often
//! buried behind whatever you are actually working on. MPRIS already exposes playback to the
//! desktop, but only to things that speak it — a bar applet, `playerctl`, media keys. This puts
//! the player itself one click away, with the current track readable without switching windows.
//!
//! Implemented over the StatusNotifierItem D-Bus protocol via `ksni`, not a GUI toolkit. A
//! terminal program should not pull in GTK to draw a 22-pixel icon, and `ksni` reuses the same
//! `zbus` that `mpris` is already talking over, so the whole feature costs two crates.
//!
//! Like MPRIS, it is advisory. A machine with no session bus — a server, an SSH session, a bare
//! tty — must play music exactly as well as a desktop does, so every failure here is reported and
//! then forgotten.

use anyhow::{Context, Result};

use crate::player::{PlayerCommand, PlayerHandle};

/// How often the icon re-reads the player.
///
/// Slower than the MPRIS poll: this only feeds a tooltip and two checkmarks, and a tray that
/// repaints on a timer the user cannot perceive is battery spent on nothing.
const POLL: std::time::Duration = std::time::Duration::from_millis(1000);

/// Matches `Icon=wander` in `packaging/wander.desktop`, so the tray and the launcher show the same
/// artwork. A theme without it falls back to a generic icon rather than showing nothing.
const ICON: &str = "wander";

/// What the icon says, in one place.
///
/// Defined as a free function because both the tray and the poll that decides whether the tray is
/// worth repainting need it, and two copies of a formatting rule are two chances for the bar to
/// disagree with itself about what is playing.
fn now_playing(player: &PlayerHandle) -> String {
    match player.status().current {
        Some(song) => match song.artist.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
            Some(artist) => format!("{artist} — {}", song.title),
            // A local file with no tags still has a filename-derived title worth showing; padding
            // it with "Unknown artist" adds nothing the user did not already know.
            None => song.title,
        },
        None => "Nothing playing".to_string(),
    }
}

struct WanderTray {
    player: PlayerHandle,
    /// Cached so the menu and tooltip do not each hit the player independently and risk
    /// disagreeing with one another within a single repaint.
    title: String,
    paused: bool,
}

impl WanderTray {
    fn new(player: PlayerHandle) -> Self {
        let mut tray = Self {
            player,
            title: String::new(),
            paused: true,
        };
        tray.refresh();
        tray
    }

    /// Re-reads the player into the fields the menu and tooltip are built from.
    fn refresh(&mut self) {
        self.title = now_playing(&self.player);
        self.paused = self.player.is_paused();
    }
}

impl ksni::Tray for WanderTray {
    fn id(&self) -> String {
        "wander".into()
    }

    fn icon_name(&self) -> String {
        ICON.into()
    }

    /// The hover text. `title` rather than `tool_tip` because several bars render only this one,
    /// and a tray icon that cannot say what is playing is not worth the pixels.
    fn title(&self) -> String {
        self.title.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            icon_name: ICON.into(),
            title: "Wander".into(),
            description: self.title.clone(),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem, StandardItem};

        vec![
            // Not clickable: the label *is* the information. Disabled so it does not look like a
            // control that does nothing when pressed.
            StandardItem {
                label: self.title.clone(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: if self.paused { "Play".into() } else { "Pause".into() },
                icon_name: if self.paused {
                    "media-playback-start".into()
                } else {
                    "media-playback-pause".into()
                },
                activate: Box::new(|this: &mut Self| {
                    this.player.send(PlayerCommand::TogglePause);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Previous".into(),
                icon_name: "media-skip-backward".into(),
                activate: Box::new(|this: &mut Self| this.player.send(PlayerCommand::Prev)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Next".into(),
                icon_name: "media-skip-forward".into(),
                activate: Box::new(|this: &mut Self| this.player.send(PlayerCommand::Next)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            // Stops playback only. Quitting Wander from here would mean killing the terminal
            // session the user is sitting in, from a menu they may have opened by accident.
            StandardItem {
                label: "Stop".into(),
                icon_name: "media-playback-stop".into(),
                activate: Box::new(|this: &mut Self| this.player.send(PlayerCommand::Stop)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Publishes the tray icon and keeps it in step with playback.
///
/// Returns `Err` when there is no status-bar host to talk to, which the caller surfaces in the
/// status line rather than treating as fatal.
pub async fn spawn(player: PlayerHandle) -> Result<()> {
    use ksni::TrayMethods;

    let watcher = player.clone();
    let handle = WanderTray::new(player)
        .spawn()
        .await
        .context("publishing the tray icon")?;

    tokio::spawn(async move {
        // Mirrored outside the tray so the poll can tell whether anything changed *without*
        // touching it. `update` repaints and notifies the bar unconditionally, so calling it every
        // second would have a paused player waking the status bar forever for no new information.
        let mut last: Option<(String, bool)> = None;

        loop {
            tokio::time::sleep(POLL).await;

            let now = (now_playing(&watcher), watcher.is_paused());
            if last.as_ref() == Some(&now) {
                continue;
            }
            last = Some(now);

            // Awaited. `Handle::update` is async, and a future that is built and dropped does
            // nothing at all: the icon kept whatever it read at startup, so the title never
            // followed the track and the button never left "Play".
            handle
                .update(|tray: &mut WanderTray| {
                    tray.refresh();
                })
                .await;
        }
    });

    Ok(())
}
