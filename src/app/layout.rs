use super::types::*;
use super::*;
use crate::ui::{Hits, Region};
use std::time::Duration;

impl App {
    /// Which lyric line the view should centre on when there is no timing to
    /// follow.
    pub fn lyrics_scroll_target(&self) -> usize {
        self.lyrics_sel
            .index
            .min(self.lyrics.lines.len().saturating_sub(1))
    }

    /// Statistics including the track playing right now, so the Home counters
    /// tick during a track rather than only jumping when it ends.
    pub fn live_stats(&self) -> crate::history::Stats {
        if self.player.status().current.is_none() {
            return self.stats.clone();
        }
        let secs = self.player.elapsed().as_secs();
        // The play started `secs` ago, so that is the hour bucket it belongs to.
        let hour = ((crate::history::now() - secs as i64).rem_euclid(86_400) / 3600) as usize;
        self.stats.with_in_progress(secs, hour)
    }

    /// Enter or leave the full-screen now-playing view.
    ///
    /// Focus moves to the lyrics while it is open. That is the only place they
    /// can hold the cursor now that they have no tab of their own, and it makes
    /// up/down do the obvious thing: scroll the words you are reading.
    pub fn set_focus_mode(&mut self, on: bool) {
        self.focus_mode = on;
        self.focus = if on { Pane::Lyrics } else { self.panes()[0] };
        self.status_message = Some(if on {
            "Focus mode — F or Esc to leave".to_string()
        } else {
            "Focus mode off".to_string()
        });
    }

    /// Widen (`+1`) or narrow (`-1`) the side panes by one step.
    /// Move the dividers one step.
    ///
    /// Both side panes move, so the content area between them changes by twice
    /// `PANE_STEP` per press — which is why that constant is half of what one
    /// press should feel like.
    pub(crate) fn nudge_side_panes(&mut self, direction: i8) {
        self.cover_percent = step_percent(self.cover_percent, direction);
        self.queue_percent = step_percent(self.queue_percent, direction);
    }

    pub(crate) fn nudge_visualiser_height(&mut self, direction: i8) {
        let step = 2;
        let new_height =
            (self.visualiser_height as i16 + direction as i16 * step).clamp(3, 40) as u16;
        self.visualiser_height = new_height;
    }

    pub(crate) fn update_visualiser_drag(&mut self, row: u16, hits: &Hits) {
        if let Some(rect) = hits.rect_of(Region::Visualiser) {
            let bottom = rect.y.saturating_add(rect.height);
            let new_height = bottom.saturating_sub(row).clamp(3, 40);
            self.visualiser_height = new_height;
        }
    }

    // ---- pane size tweening ---------------------------------------------

    /// Advance the pane-size animations one frame, and report the sizes to draw
    /// with. Called once per frame from the renderer.
    pub fn tween_panes(&mut self) -> (u16, u16, u16) {
        fn ease(current: &mut f32, target: u16) -> u16 {
            let target = target as f32;
            if (target - *current).abs() < PANE_EPSILON {
                *current = target;
            } else {
                *current += (target - *current) * PANE_EASE;
            }
            current.round().max(0.0) as u16
        }
        (
            ease(&mut self.eased_cover, self.cover_percent),
            ease(&mut self.eased_queue, self.queue_percent),
            ease(&mut self.eased_visualiser_height, self.visualiser_height),
        )
    }

    pub(crate) fn panes_are_tweening(&self) -> bool {
        (self.eased_cover - self.cover_percent as f32).abs() >= PANE_EPSILON
            || (self.eased_queue - self.queue_percent as f32).abs() >= PANE_EPSILON
            || (self.eased_visualiser_height - self.visualiser_height as f32).abs() >= PANE_EPSILON
    }

    /// What the frame looks like, coarsely: enough to notice when the album art
    /// needs a clean repaint instead of a diff.
    ///
    /// Album art is drawn by a terminal graphics protocol, which means its cells
    /// are marked `skip` and the backend deliberately writes nothing to them. A
    /// popup drawn on top puts real text in those cells; when it closes, the
    /// artwork re-skips them and the popup's characters are never overwritten —
    /// which is the fragment left behind on screen. Comparing this value between
    /// frames catches every such transition in one place, rather than asking a
    /// dozen `overlay = …` and pane-toggle sites to remember to invalidate.
    pub fn frame_shape(&self) -> FrameShape {
        FrameShape {
            overlay: self.overlay.as_ref().map(|overlay| overlay.kind()),
            show_help: self.show_help,
            focus_mode: self.focus_mode,
            panes: [
                self.show_queue_pane,
                self.show_focus_queue,
                self.show_cover_pane,
                self.show_focus_cover,
                self.show_lyrics_pane,
                self.show_focus_lyrics,
                self.show_visualiser,
            ],
            // Mid-tween the panes move a column at a time, so the artwork's rect
            // moves with them and would otherwise smear. Tracking the eased
            // sizes rather than "is tweening" catches every step of the glide,
            // not just its start and end.
            panes_sizes: [
                self.eased_cover.round() as u16,
                self.eased_queue.round() as u16,
                self.eased_visualiser_height.round() as u16,
            ],
            cover: self.cover_generation,
            tab: self.tab as u8,
            viz_mode: self.viz_mode,
        }
    }

    /// Whether anything on screen is animating and therefore needs redrawing.
    ///
    /// The main loop only wakes on a timer while this is true, so an animation
    /// that is not listed here simply will not run.
    pub fn is_animating(&self) -> bool {
        // Home's live counters only move while a track is playing, which the
        // first condition already covers.
        (self.player.status().playing && !self.player.is_paused())
            || self.panes_are_tweening()
            || self.lyrics_are_scrolling()
    }

    /// Hand-scrolled lyrics ease toward the cursor, which needs frames to
    /// finish; without this the glide stops wherever the last keypress left it.
    pub(crate) fn lyrics_are_scrolling(&self) -> bool {
        !self.lyrics.synced
            && !self.lyrics.lines.is_empty()
            && (self.lyrics_scroll - self.lyrics_scroll_target() as f32).abs() >= 0.01
    }
}

/// Put text on the system clipboard via OSC 52.
///
/// The terminal itself does the copying, so this works over SSH and needs no
/// clipboard library. Terminals that ignore the sequence simply do nothing —
/// which is why the share popup keeps showing the URL either way.
pub(crate) fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    let encoded = base64(text.as_bytes());
    let mut stdout = std::io::stdout();
    let _ = write!(stdout, "\x1b]52;c;{encoded}\x07");
    let _ = stdout.flush();
}

/// Open a URL in the user's default browser.
pub(crate) fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

pub(crate) fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(ALPHABET[(n >> 18 & 63) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// A song's effective rating: what this session set, else what the server
/// reported, else unrated.
///

/// Bounds a side pane can be resized between, as a percentage of the width.
pub(crate) const PANE_MIN_PERCENT: u16 = 10;
pub(crate) const PANE_MAX_PERCENT: u16 = 45;
/// Per-pane step. Both side panes move together on a resize, so the content
/// area between them changes by twice this — one press, one visible step.
pub(crate) const PANE_STEP: u16 = 1;

/// One resize step, clamped so a pane can never vanish or swallow the screen.
pub(crate) fn step_percent(current: u16, direction: i8) -> u16 {
    if direction >= 0 {
        (current + PANE_STEP).min(PANE_MAX_PERCENT)
    } else {
        current.saturating_sub(PANE_STEP).max(PANE_MIN_PERCENT)
    }
}

/// Users type `~/Music`; nothing in `std` expands it, and a literal `~`
/// directory is not what they meant.
pub(crate) fn expand_home(input: &str) -> std::path::PathBuf {
    let trimmed = input.trim();
    if let Some(rest) = trimmed.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return std::path::PathBuf::from(home).join(rest);
    }
    if trimmed == "~"
        && let Some(home) = std::env::var_os("HOME")
    {
        return std::path::PathBuf::from(home);
    }
    std::path::PathBuf::from(trimmed)
}

/// Format a duration as `m:ss`, or `h:mm:ss` past an hour.
pub fn format_duration(duration: Duration) -> String {
    let total = duration.as_secs();
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}
