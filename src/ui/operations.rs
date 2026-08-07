//! The Operations tab screen.
//!
//! Shows current and recent background operations (downloads, library scans,
//! searches, lyric fetches, etc.), progress gauges, cancellation options, and
//! notification log history.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph};

use super::{Hits, Region};
use crate::app::{App, OperationKind, OperationStatus, Pane};
use crate::theme::Theme;

pub fn draw(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme, hits: &mut Hits) {
    let rows = Layout::vertical([
        Constraint::Length(3), // Summary bar
        Constraint::Min(6),    // Operations list
        Constraint::Length(8), // Notification log
    ])
    .split(area);

    draw_summary(frame, rows[0], app, theme);
    draw_operations_list(frame, rows[1], app, theme, hits);
    draw_notification_log(frame, rows[2], app, theme);
}

fn draw_summary(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border(app.focus == Pane::Operations));

    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 1 {
        return;
    }

    let active = app.active_operations_count();
    let completed = app.operations.iter().filter(|o| o.status == OperationStatus::Completed).count();
    let failed = app.operations.iter().filter(|o| matches!(o.status, OperationStatus::Failed(_))).count();

    let summary_line = Line::from(vec![
        Span::styled("⚡ Active Operations: ", theme.title()),
        Span::styled(format!("{active} running  •  "), theme.selected()),
        Span::styled(format!("{completed} completed  •  "), theme.dim()),
        Span::styled(format!("{failed} failed  │  "), if failed > 0 { theme.error() } else { theme.dim() }),
        Span::styled("[c] Cancel Selected   ", theme.playing()),
        Span::styled("[x] Clear Finished", theme.dim()),
    ]);

    frame.render_widget(Paragraph::new(summary_line), inner);
}

fn draw_operations_list(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme, hits: &mut Hits) {
    let block = Block::default()
        .title(" Operations & Downloads ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border(app.focus == Pane::Operations));

    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 2 {
        return;
    }

    if app.operations.is_empty() {
        let empty_msg = Paragraph::new(Line::from(Span::styled(
            " No active or recent background operations.",
            theme.dim(),
        )));
        frame.render_widget(empty_msg, inner);
        return;
    }

    let items: Vec<ListItem> = app
        .operations
        .iter()
        .enumerate()
        .map(|(index, op)| {
            let is_selected = app.focus == Pane::Operations && index == app.operations_sel.index;
            let (status_icon, status_style) = match &op.status {
                OperationStatus::Running => ("⚡", theme.playing()),
                OperationStatus::Completed => ("✓", theme.selected()),
                OperationStatus::Failed(_) => ("✕", theme.error()),
                OperationStatus::Cancelled => ("⊘", theme.dim()),
            };

            let badge = format!(" [{}] ", op.kind.badge());
            let elapsed = op.started_at.elapsed().as_secs();
            let duration_str = if elapsed < 60 {
                format!("{elapsed}s")
            } else {
                format!("{}m {}s", elapsed / 60, elapsed % 60)
            };

            let progress_str = match &op.status {
                OperationStatus::Running => {
                    if let Some(p) = op.progress {
                        let pct = (p * 100.0).clamp(0.0, 100.0) as usize;
                        let filled = pct / 10;
                        let empty = 10 - filled;
                        format!("[{}{}] {:>3}%", "█".repeat(filled), "░".repeat(empty), pct)
                    } else {
                        "Running…".to_string()
                    }
                }
                OperationStatus::Completed => "Completed".to_string(),
                OperationStatus::Failed(err) => format!("Failed: {err}"),
                OperationStatus::Cancelled => "Cancelled".to_string(),
            };

            let title_padded = format!("{:<30}", crate::ui::widgets::truncate(&op.title, 30));
            let detail_text = op.details.as_deref().unwrap_or("");
            let detail_padded = format!("{:<25}", crate::ui::widgets::truncate(detail_text, 25));

            let item_line = Line::from(vec![
                Span::styled(if is_selected { "❯ " } else { "  " }, theme.title()),
                Span::styled(status_icon, status_style),
                Span::styled(badge, theme.dim()),
                Span::styled(title_padded, if is_selected { theme.selected() } else { theme.base() }),
                Span::styled(detail_padded, theme.dim()),
                Span::styled(format!(" {:<16} ", progress_str), status_style),
                Span::styled(duration_str, theme.dim()),
            ]);

            ListItem::new(item_line)
        })
        .collect();

    let mut state = ListState::default().with_selected(Some(app.operations_sel.index));
    frame.render_stateful_widget(List::new(items), inner, &mut state);

    // Record hit areas
    for (i, _) in app.operations.iter().enumerate() {
        if (i as u16) < inner.height {
            hits.push(
                Rect {
                    x: inner.x,
                    y: inner.y + i as u16,
                    width: inner.width,
                    height: 1,
                },
                Region::Row {
                    pane: Pane::Operations,
                    index: i,
                },
            );
        }
    }
}

fn draw_notification_log(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = Block::default()
        .title(" Notification Log ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border(false));

    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 1 {
        return;
    }

    if app.notifications.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(" No notifications recorded yet.", theme.dim()))),
            inner,
        );
        return;
    }

    let visible_count = inner.height as usize;
    let start_idx = app.notifications.len().saturating_sub(visible_count);
    let items: Vec<ListItem> = app.notifications[start_idx..]
        .iter()
        .map(|n| {
            let style = match n.level {
                crate::app::NotificationLevel::Info => theme.base(),
                crate::app::NotificationLevel::Success => theme.selected(),
                crate::app::NotificationLevel::Warning => theme.playing(),
                crate::app::NotificationLevel::Error => theme.error(),
            };
            let icon = n.level.icon();
            let elapsed = n.created_at.elapsed().as_secs();
            let time_str = format!("{elapsed}s ago");

            ListItem::new(Line::from(vec![
                Span::styled(format!(" {} ", icon), style),
                Span::styled(format!("{:<60} ", crate::ui::widgets::truncate(&n.message, 60)), style),
                Span::styled(time_str, theme.dim()),
            ]))
        })
        .collect();

    frame.render_widget(List::new(items), inner);
}
