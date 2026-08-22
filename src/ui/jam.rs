//! The Jam tab: one queue, several people, and a rule for whose turn is next.
//!
//! Distinct from listen-along, which mirrors one person's playback. Here everybody adds to the
//! same queue, and in `democracy` the order is the room's decision rather than the host's — so the
//! vote count is the most important column on screen, not a decoration beside the title.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use super::widgets::truncate;
use super::Hits;
use crate::app::App;
use crate::integrations::agro_jam::JamMode;
use crate::theme::Theme;

pub fn draw(frame: &mut Frame, area: Rect, app: &App, theme: &Theme, _hits: &mut Hits) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border.0))
        .title(" Jam ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(jam) = app.jam.as_ref() else {
        let help = vec![
            Line::from(Span::styled(
                "You are not in a jam.",
                Style::default().fg(theme.foreground.0).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "c  start one          j  join with a code",
                Style::default().fg(theme.foreground.0),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "One queue, everyone in it. The room plays the same track at the same time, and \
                 while you are in a jam anything you play goes to the room instead.",
                Style::default().fg(theme.border.0),
            )),
        ];
        frame.render_widget(Paragraph::new(help), inner);
        return;
    };

    let rows = Layout::vertical([Constraint::Length(3), Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    // The code first: a jam nobody else can reach is just a playlist.
    let header = vec![
        Line::from(vec![
            Span::styled("code ", Style::default().fg(theme.border.0)),
            Span::styled(
                jam.code.clone(),
                Style::default().fg(theme.accent.0).add_modifier(Modifier::BOLD),
            ),
            Span::styled("   mode ", Style::default().fg(theme.border.0)),
            Span::styled(jam.mode.label(), Style::default().fg(theme.foreground.0)),
            Span::styled(
                if jam.is_host { "   (you host)" } else { "" },
                Style::default().fg(theme.border.0),
            ),
            Span::styled(
                if jam.open_to_friends { "   open to friends" } else { "   code only" },
                Style::default().fg(theme.border.0),
            ),
        ]),
        Line::from(Span::styled(
            format!("with {}", jam.members.join(", ")),
            Style::default().fg(theme.border.0),
        )),
    ];
    frame.render_widget(Paragraph::new(header), rows[0]);

    // Three things, in the order they matter: what the room is on, what it will play, and what
    // has been suggested but not agreed. The last is kept separate because it is *not* in the
    // queue — showing it as though it were is what made voting look like it did nothing.
    let mut lines: Vec<Line> = Vec::new();
    let width = rows[1].width as usize;

    match jam.now_playing.as_ref() {
        Some(now) => {
            let elapsed = now.position_ms / 1000;
            let total = now.duration_ms / 1000;
            lines.push(Line::from(vec![
                Span::styled("▶ ", Style::default().fg(theme.accent.0)),
                Span::styled(
                    truncate(&now.title, width.saturating_sub(28)),
                    Style::default().fg(theme.foreground.0).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {}  ", truncate(&now.artist, 16)),
                    Style::default().fg(theme.border.0),
                ),
                Span::styled(
                    format!("{}:{:02}/{}:{:02}", elapsed / 60, elapsed % 60, total / 60, total % 60),
                    Style::default().fg(theme.border.0),
                ),
                Span::styled(
                    if now.you_skipped {
                        format!("  skip {}/{} ✓", now.skip_votes, now.skips_needed)
                    } else if now.skip_votes > 0 {
                        format!("  skip {}/{}", now.skip_votes, now.skips_needed)
                    } else {
                        String::new()
                    },
                    Style::default().fg(theme.border.0),
                ),
            ]));
        }
        None => lines.push(Line::from(Span::styled(
            "nothing playing",
            Style::default().fg(theme.border.0),
        ))),
    }

    if !jam.proposals.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("waiting on the room ({} to agree)", jam.approvals_needed),
            Style::default().fg(theme.border.0),
        )));
        for (index, track) in jam.proposals.iter().enumerate() {
            let selected = index == app.jam_sel;
            let style = if selected {
                Style::default().fg(theme.highlight_fg.0).bg(theme.highlight_bg.0)
            } else {
                Style::default().fg(theme.foreground.0)
            };
            // A tick where yours already is: approving is one-way, so there is nothing to undo.
            let mark = if track.approved { "✓" } else { " " };
            lines.push(Line::from(Span::styled(
                format!(
                    "{mark} {:>2} more  {}  —  {}   ·{}",
                    track.still_needed,
                    truncate(&track.title, width.saturating_sub(36)),
                    truncate(&track.artist, 16),
                    track.added_by
                ),
                style,
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "up next",
        Style::default().fg(theme.border.0),
    )));
    if jam.queue.is_empty() {
        lines.push(Line::from(Span::styled(
            "  nothing queued — press a on a track to send it to the room",
            Style::default().fg(theme.border.0),
        )));
    }
    for track in &jam.queue {
        lines.push(Line::from(Span::styled(
            format!(
                "   {}  —  {}   ·{}",
                truncate(&track.title, width.saturating_sub(30)),
                truncate(&track.artist, 16),
                track.added_by
            ),
            Style::default().fg(theme.foreground.0),
        )));
    }
    frame.render_widget(Paragraph::new(lines), rows[1]);

    let keys = if jam.is_host {
        "Enter/v accept   s skip   x remove   m mode   o open to friends   l end jam"
    } else {
        "Enter/v accept   s skip   x remove your own   l leave"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(keys, Style::default().fg(theme.border.0)))),
        rows[2],
    );
}

/// What the host's toggle switches to. Kept here so the key handler and the label agree.
pub fn next_mode(current: JamMode) -> JamMode {
    current.toggled()
}
