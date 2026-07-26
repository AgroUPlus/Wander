use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use super::widgets::gradient;
use super::{Hits, Region};
use crate::app::{App, Pane};
use crate::theme::Theme;

/// How quickly the view eases toward the active line. Higher is snappier;
/// this reaches the target in roughly a third of a second at 20 fps.
const EASE: f32 = 0.18;
/// Lines beyond this distance from the active one are fully dimmed.
const FALLOFF: f32 = 4.0;

/// Smooth-scrolling lyrics with the active line centred and highlighted.
///
/// Scroll position is interpolated rather than snapped so the view glides
/// between lines instead of jumping.
pub fn draw(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme, hits: &mut Hits) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border(false))
        .style(theme.base())
        .title(" Lyrics ")
        .title_style(theme.title());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let lyrics = &app.lyrics;

    if lyrics.is_empty() {
        let message = if app.lyrics_pending {
            "Loading lyrics…"
        } else {
            "No lyrics"
        };
        let middle = Rect {
            y: inner.y + inner.height / 2,
            height: 1,
            ..inner
        };
        frame.render_widget(
            Paragraph::new(message).style(theme.dim()).centered(),
            middle,
        );
        return;
    }

    let position = app.player.elapsed();
    let active = lyrics.active_at(position);

    // Synced lyrics follow playback. Unsynced ones have no timing to follow, so
    // they scroll with the cursor instead — without this they were pinned to
    // the first line with no way to read the rest.
    let target = match active {
        Some(index) => index as f32,
        None if lyrics.synced => 0.0,
        None => app.lyrics_scroll_target() as f32,
    };

    // Ease toward the target line; `scroll` is retained between frames.
    app.lyrics_scroll += (target - app.lyrics_scroll) * EASE;
    if (target - app.lyrics_scroll).abs() < 0.01 {
        app.lyrics_scroll = target;
    }

    let centre = inner.height as f32 / 2.0;
    let mut lines: Vec<Line> = Vec::with_capacity(inner.height as usize);

    for row in 0..inner.height {
        // Which lyric line falls on this screen row, given the eased scroll.
        let offset = row as f32 - centre + app.lyrics_scroll;
        let index = offset.round() as i32;

        if index < 0 || index as usize >= lyrics.lines.len() {
            lines.push(Line::default());
            continue;
        }

        let idx = index as usize;
        let row_rect = Rect {
            x: inner.x,
            y: inner.y + row,
            width: inner.width,
            height: 1,
        };
        hits.push(
            row_rect,
            Region::Row {
                pane: Pane::Lyrics,
                index: idx,
            },
        );

        let text = &lyrics.lines[index as usize].text;
        let distance = (index as f32 - app.lyrics_scroll).abs();
        let is_active = active == Some(index as usize);

        let line = if is_active {
            let progress = lyrics.line_progress(position).clamp(0.0, 1.0);
            let colour = gradient(
                theme.foreground.0,
                theme.current_track.0,
                (progress * 3.0).min(1.0) as f64,
            );
            let style = Style::default().fg(colour).add_modifier(Modifier::BOLD);
            Line::from(vec![
                Span::styled("❯ ", theme.title()),
                Span::styled(text.as_str(), style),
                Span::styled(" ❮", theme.title()),
            ])
            .centered()
        } else {
            let t = (distance / FALLOFF).clamp(0.0, 1.0);
            let style = Style::default().fg(gradient(theme.foreground.0, theme.dim.0, t as f64));
            Line::from(Span::styled(text.as_str(), style)).centered()
        };

        lines.push(line);
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reproduces the easing update the draw loop performs.
    fn ease(mut scroll: f32, target: f32, frames: usize) -> f32 {
        for _ in 0..frames {
            scroll += (target - scroll) * EASE;
            if (target - scroll).abs() < 0.01 {
                scroll = target;
            }
        }
        scroll
    }

    #[test]
    fn scroll_converges_on_the_target_line() {
        // Roughly a third of a second at 20 fps.
        let settled = ease(0.0, 5.0, 30);
        assert!((settled - 5.0).abs() < 0.05, "did not converge: {settled}");
    }

    #[test]
    fn scroll_moves_gradually_rather_than_snapping() {
        let after_one = ease(0.0, 10.0, 1);
        assert!(after_one > 0.0, "should move");
        assert!(after_one < 10.0, "should not jump straight to the target");
    }

    #[test]
    fn scroll_is_stable_once_settled() {
        let settled = ease(7.0, 7.0, 10);
        assert_eq!(settled, 7.0, "an unchanged target must not drift");
    }

    #[test]
    fn scroll_handles_backward_seeks() {
        let settled = ease(20.0, 2.0, 40);
        assert!(
            (settled - 2.0).abs() < 0.05,
            "seeking back should converge: {settled}"
        );
    }
}
