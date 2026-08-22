//! The Friends tab: who you know, what they have been into, and what they have handed you.
//!
//! Four surfaces in one tab rather than four tabs, because they are all the same question at
//! different ranges — and because each of them is often empty. Every one is gated on a switch on
//! somebody *else's* account, and those default closed, so a tab per surface would mostly be a tab
//! per empty screen. Together they at least add up to a page.
//!
//! Only the friend list takes the cursor. It is the half that answers keys, because sending a drop
//! needs somebody chosen; the feed, the inbox and the recap are read.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use super::widgets::truncate;
use super::{Hits, Region};
use crate::app::types::Pane;
use crate::app::App;
use crate::theme::Theme;

pub fn draw(frame: &mut Frame, area: Rect, app: &App, theme: &Theme, hits: &mut Hits) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border.0))
        .title(" Friends ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.friends.is_empty() && app.inbox.is_empty() && app.social_feed.is_empty() {
        let help = vec![
            Line::from(Span::styled(
                "Nothing here yet.",
                Style::default().fg(theme.foreground.0).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Add friends from the dashboard or the phone app. Once they are there, this shows \
                 what they have been listening to and the songs they send you.",
                Style::default().fg(theme.border.0),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Each of those needs a switch on their account, and every switch starts off — an \
                 empty page here usually means nobody has opted in rather than that anything failed.",
                Style::default().fg(theme.border.0),
            )),
        ];
        frame.render_widget(Paragraph::new(help), inner);
        return;
    }

    // Friends on the left because that is the pane the cursor lives in; everything else reads to
    // the right of it.
    let columns = Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(inner);
    draw_friends(frame, columns[0], app, theme, hits);

    let rows = Layout::vertical([
        Constraint::Percentage(40),
        Constraint::Percentage(40),
        Constraint::Percentage(20),
    ])
    .split(columns[1]);
    draw_inbox(frame, rows[0], app, theme);
    draw_feed(frame, rows[1], app, theme);
    draw_recap(frame, rows[2], app, theme);
}

fn draw_friends(frame: &mut Frame, area: Rect, app: &App, theme: &Theme, hits: &mut Hits) {
    let focused = app.focus == Pane::Social;
    let width = area.width.saturating_sub(2) as usize;
    let mut lines = Vec::new();

    for (index, friend) in app.friends.iter().enumerate() {
        let selected = focused && index == app.social_sel;
        let style = if selected {
            Style::default().fg(theme.accent.0).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground.0)
        };
        lines.push(Line::from(Span::styled(
            format!("{} {}", if selected { "▸" } else { " " }, truncate(friend.label(), width)),
            style,
        )));
        // Presence sits under the name rather than beside it: a track and an artist do not fit on
        // one line beside a username in a pane this narrow, and truncating the name to fit them
        // loses the more important half.
        if let Some(now) = friend.now_playing.as_deref() {
            lines.push(Line::from(Span::styled(
                format!("   ♪ {}", truncate(now, width.saturating_sub(3))),
                Style::default().fg(theme.border.0),
            )));
        }
        if area.height > 0 {
            hits.push(
                Rect { x: area.x, y: area.y + index as u16, width: area.width, height: 1 },
                Region::Row { pane: Pane::Social, index },
            );
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No friends yet.",
            Style::default().fg(theme.border.0),
        )));
    }

    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(theme.border.0))
        .title(" People ");
    let body = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines), body);
}

fn draw_inbox(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let unread = app.inbox.iter().filter(|drop| drop.is_unread()).count();
    let title = if unread > 0 {
        format!(" Inbox ({unread} new) ")
    } else {
        " Inbox ".to_string()
    };
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(theme.border.0))
        .title(title);
    let body = block.inner(area);
    frame.render_widget(block, area);

    let width = body.width as usize;
    let mut lines = Vec::new();
    for (index, drop) in app.inbox.iter().enumerate() {
        let selected = index == app.inbox_sel;
        // Unread is the only thing here that gets the accent. A list where every row is emphasised
        // emphasises nothing, and "there is something you have not seen" is exactly a status.
        let style = if drop.is_unread() {
            Style::default().fg(theme.accent.0).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground.0)
        };
        lines.push(Line::from(Span::styled(
            format!(
                "{} {} — {} · from {}",
                if selected { "▸" } else { " " },
                drop.track_title,
                drop.artist_name,
                drop.from_user
            ),
            style,
        )));
        if let Some(note) = drop.note.as_deref() {
            lines.push(Line::from(Span::styled(
                format!("   “{}”", truncate(note, width.saturating_sub(5))),
                Style::default().fg(theme.border.0),
            )));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Nothing sent to you yet.",
            Style::default().fg(theme.border.0),
        )));
    }
    frame.render_widget(Paragraph::new(lines), body);
}

fn draw_feed(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(theme.border.0))
        .title(" Lately ");
    let body = block.inner(area);
    frame.render_widget(block, area);

    let width = body.width as usize;
    let mut lines: Vec<Line> = app
        .social_feed
        .iter()
        // The summary is composed by the server so that every client says the same thing. Building
        // our own sentence here would drift from the phone's the first time a rule changed.
        .map(|item| {
            Line::from(Span::styled(
                truncate(&item.summary, width),
                Style::default().fg(theme.foreground.0),
            ))
        })
        .collect();
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No activity shared. Friends have to turn it on.",
            Style::default().fg(theme.border.0),
        )));
    }
    frame.render_widget(Paragraph::new(lines), body);
}

fn draw_recap(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = Block::default().title(format!(" Recap · {} ", app.recap_period));
    let body = block.inner(area);
    frame.render_widget(block, area);

    let width = body.width as usize;
    let mut lines = Vec::new();
    if let Some(anthem) = app.recap.anthem.as_deref() {
        lines.push(Line::from(vec![
            Span::styled("Anthem  ", Style::default().fg(theme.border.0)),
            Span::styled(truncate(anthem, width.saturating_sub(8)), Style::default().fg(theme.foreground.0)),
        ]));
    }
    if let Some(setter) = app.recap.trendsetter.as_deref() {
        lines.push(Line::from(vec![
            Span::styled("First   ", Style::default().fg(theme.border.0)),
            Span::styled(truncate(setter, width.saturating_sub(8)), Style::default().fg(theme.foreground.0)),
        ]));
    }
    for pair in app.recap.matrix.iter().take(3) {
        lines.push(Line::from(vec![
            Span::styled("Match   ", Style::default().fg(theme.border.0)),
            Span::styled(truncate(pair, width.saturating_sub(8)), Style::default().fg(theme.foreground.0)),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No recap: nobody in the circle has opened their statistics.",
            Style::default().fg(theme.border.0),
        )));
    }
    frame.render_widget(Paragraph::new(lines), body);
}
