use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders, Cell, Row, Table, TableState};

use super::widgets::truncate;
use super::{Hits, Region};
use crate::app::{App, Pane, format_duration};
use crate::config::ColumnKind;
use crate::theme::Theme;

/// The play queue.
///
/// Used both as the full-width Queue tab (`full = true`, honouring the user's
/// configured columns) and as the narrow side pane (`full = false`, which drops
/// to title + duration so it stays readable when squeezed).
pub fn draw(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    theme: &Theme,
    hits: &mut Hits,
    pane: Pane,
    full: bool,
) {
    let focused = full || app.focus == pane;

    let columns: Vec<(ColumnKind, u16)> = if full {
        app.config
            .queue_columns
            .iter()
            .map(|c| (c.kind, c.width))
            .collect()
    } else {
        // The side pane has no room for a column of its own, so the source
        // badge rides along in front of the title.
        vec![
            (ColumnKind::Source, 8),
            (ColumnKind::Title, 70),
            (ColumnKind::Length, 22),
        ]
    };

    let header = Row::new(
        columns
            .iter()
            .map(|(kind, _)| Cell::from(kind.header()))
            .collect::<Vec<_>>(),
    )
    .style(theme.dim());

    let queue = app.player.queue.lock().unwrap();
    let playing = queue.current_index();
    let count = queue.len();

    let rows: Vec<Row> = queue
        .songs()
        .iter()
        .enumerate()
        .map(|(index, song)| {
            let cells: Vec<Cell> = columns
                .iter()
                .map(|(kind, _)| {
                    // The badge carries its own colour, so it is built as a
                    // span rather than falling through to the row's style.
                    if *kind == ColumnKind::Source {
                        return Cell::from(Line::from(super::widgets::source_span(
                            &song.id,
                            app.config.glyphs,
                            theme,
                        )));
                    }
                    let text = match kind {
                        ColumnKind::Artist => song.artist_or_unknown().to_string(),
                        ColumnKind::Title => song.title.clone(),
                        ColumnKind::Album => song.album_or_unknown().to_string(),
                        ColumnKind::Length => {
                            format_duration(std::time::Duration::from_secs(song.duration as u64))
                        }
                        ColumnKind::Track => song.track.map(|t| t.to_string()).unwrap_or_default(),
                        ColumnKind::Year => song.year.map(|y| y.to_string()).unwrap_or_default(),
                        ColumnKind::Source => unreachable!("handled above"),
                    };
                    Cell::from(text)
                })
                .collect();

            // The playing track stays visually distinct even when not selected.
            let style = if Some(index) == playing {
                theme.playing()
            } else {
                theme.base()
            };
            Row::new(cells).style(style)
        })
        .collect();

    drop(queue);
    app.queue_sel.clamp(count);

    let widths: Vec<Constraint> = columns
        .iter()
        .map(|(_, width)| Constraint::Percentage(*width))
        .collect();

    let title = if full {
        format!(" Queue ({count}) ")
    } else {
        format!(" Up Next ({count}) ")
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border(focused))
        .style(theme.base())
        .title(truncate(&title, area.width.saturating_sub(2) as usize))
        .title_style(theme.title());
    let inner = block.inner(area);

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(theme.selected())
        .block(block);

    let mut state = TableState::default().with_selected(Some(app.queue_sel.index));
    frame.render_stateful_widget(table, area, &mut state);

    register_rows(hits, inner, count, app.queue_sel.index, pane, 1);
}

/// Register one click target per visible row.
///
/// `header_rows` accounts for a table header line; lists pass 0. The first
/// visible row is derived from the selection the same way ratatui scrolls, so
/// clicks line up with what is on screen.
pub fn register_rows(
    hits: &mut Hits,
    inner: Rect,
    count: usize,
    selected: usize,
    pane: Pane,
    header_rows: u16,
) {
    let visible = inner.height.saturating_sub(header_rows) as usize;
    if visible == 0 || count == 0 {
        return;
    }
    let first = selected
        .saturating_sub(visible.saturating_sub(1))
        .min(count.saturating_sub(visible.min(count)));

    for slot in 0..visible.min(count.saturating_sub(first)) {
        let y = inner.y + header_rows + slot as u16;
        if y >= inner.bottom() {
            break;
        }
        hits.push(
            Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 1,
            },
            Region::Row {
                pane,
                index: first + slot,
            },
        );
    }
}
