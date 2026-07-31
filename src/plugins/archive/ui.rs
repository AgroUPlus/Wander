use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table, TableState};

use super::api::ArchiveCollection;
use crate::app::{App, Pane};
use crate::theme::Theme;
use crate::ui::{Hits, Region};

pub fn draw(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme, hits: &mut Hits) {
    let focused = app.focus == Pane::Online;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border(focused))
        .style(theme.base())
        .title(" Online (Internet Archive) — [o] switch source ")
        .title_style(theme.title());

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let chunks = Layout::vertical([
        Constraint::Length(3), // Search bar & collection filter
        Constraint::Min(5),    // Results table
        Constraint::Length(1), // Help / status bar
    ])
    .split(inner);

    draw_search_bar(frame, chunks[0], app, theme, hits);
    draw_results_table(frame, chunks[1], app, theme, hits);
    draw_help_bar(frame, chunks[2], app, theme);
}

fn draw_search_bar(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme, hits: &mut Hits) {
    let splits =
        Layout::horizontal([Constraint::Min(30), Constraint::Length(28)]).split(area);

    let search_focused = app.archive_plugin.editing_query;
    let query_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border(search_focused))
        .title(" Search Query [/] ");

    let query_inner = query_block.inner(splits[0]);
    frame.render_widget(query_block, splits[0]);

    let input_width = query_inner.width as usize;
    let (query_text, query_style) = if search_focused {
        let shown = app.archive_plugin.query_input.display(input_width);
        let caret = app.archive_plugin.query_input.display_cursor(input_width);
        let mut chars: Vec<char> = shown.chars().collect();
        while chars.len() <= caret {
            chars.push(' ');
        }
        let mut text: String = chars[..caret].iter().collect();
        text.push('▏');
        text.extend(chars[caret..].iter());
        (text, theme.playing())
    } else {
        let text = if app.archive_plugin.query.is_empty() {
            "Type query & press Enter to search archive.org...".to_string()
        } else {
            app.archive_plugin.query.clone()
        };
        (crate::ui::widgets::truncate(&text, input_width), theme.base())
    };

    frame.render_widget(Paragraph::new(query_text).style(query_style), query_inner);
    hits.push(splits[0], Region::OnlineSearch);

    let current = ArchiveCollection::from_code(&app.config.plugins.archive.collection);
    let cat_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border(false))
        .title(" Collection [c] ");

    let cat_inner = cat_block.inner(splits[1]);
    frame.render_widget(cat_block, splits[1]);
    hits.push(splits[1], Region::OnlineFilter);
    frame.render_widget(
        Paragraph::new(crate::ui::widgets::truncate(
            current.label(),
            cat_inner.width as usize,
        ))
        .style(theme.selected())
        .centered(),
        cat_inner,
    );
}

fn draw_results_table(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme, hits: &mut Hits) {
    if app.archive_plugin.searching {
        let loading = vec![
            Line::from(""),
            Line::from(Span::styled(" Searching archive.org... ", theme.playing())).centered(),
        ];
        frame.render_widget(Paragraph::new(loading), area);
        return;
    }

    if app.archive_plugin.results.is_empty() {
        let empty_msg = if app.archive_plugin.query.is_empty() {
            "Press '/' to search archive.org's audio collections (public domain, netlabels, live recordings)."
        } else {
            "No items found for this query on archive.org."
        };
        let text = vec![
            Line::from(""),
            Line::from(Span::styled(empty_msg, theme.dim())).centered(),
        ];
        frame.render_widget(Paragraph::new(text), area);
        return;
    }

    let header = Row::new(
        ["", " Title", "Creator", "Year", "Length", "Downloads"]
            .iter()
            .map(|h| Cell::from(*h).style(theme.title()))
            .collect::<Vec<_>>(),
    )
    .height(1);

    let selected_index = app
        .archive_plugin
        .selection
        .index
        .min(app.archive_plugin.results.len().saturating_sub(1));

    let rows: Vec<Row> = app
        .archive_plugin
        .results
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            let is_selected = idx == selected_index;
            let style = if is_selected {
                theme.selected()
            } else {
                theme.base()
            };
            let prefix = if is_selected { "❯ " } else { "  " };

            Row::new(vec![
                Cell::from(Line::from(online_badge(app, theme))),
                Cell::from(crate::ui::widgets::truncate(
                    &format!("{}{}", prefix, item.title),
                    (area.width as usize).saturating_sub(54),
                )),
                Cell::from(crate::ui::widgets::truncate(&item.creator, 24)),
                Cell::from(item.year.clone()),
                Cell::from(length_cell(app, &item.identifier)),
                Cell::from(item.downloads.to_string()),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(2),
        Constraint::Min(30),
        Constraint::Length(26),
        Constraint::Length(6),
        Constraint::Length(9),
        Constraint::Length(10),
    ];

    let mut state = TableState::default().with_selected(Some(selected_index));
    *state.offset_mut() = app.archive_plugin.selection.offset;

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(theme.selected());

    frame.render_stateful_widget(table, area, &mut state);
    app.archive_plugin.selection.offset = state.offset();

    // Only the rows actually on screen, starting from the table's scroll
    // offset. Registering one rect per result mapped clicks to the wrong item
    // as soon as the list scrolled — and since the Online pane activates on a
    // single click, that downloaded the wrong thing.
    let first = app.archive_plugin.selection.offset;
    let visible = area.height.saturating_sub(1) as usize;
    for slot in 0..visible.min(app.archive_plugin.results.len().saturating_sub(first)) {
        let y = area.y + 1 + slot as u16;
        if y >= area.bottom() {
            break;
        }
        hits.push(
            Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            },
            Region::Row {
                pane: Pane::Online,
                index: first + slot,
            },
        );
    }
}

/// The Online badge, identical to the one these tracks will carry once they
/// are in the queue — the tab is where that connection is worth making.
fn online_badge(app: &App, theme: &Theme) -> Span<'static> {
    use crate::library::SongSource;
    Span::styled(
        crate::ui::widgets::source_glyph(SongSource::Online, app.config.glyphs),
        crate::ui::widgets::source_style(SongSource::Online, theme),
    )
}

/// The Length column for one row.
///
/// Only the item metadata knows a runtime, and that is fetched a row at a time
/// as the cursor visits them, so most rows legitimately have nothing to show
/// yet. The three states are kept visually distinct: still coming, genuinely
/// unknown, and known.
fn length_cell(app: &App, identifier: &str) -> String {
    if let Some(total) = app.archive_plugin.total_duration(identifier) {
        return crate::app::format_duration(std::time::Duration::from_secs(total));
    }
    if app.archive_plugin.pending.contains(identifier) {
        return "···".to_string();
    }
    match app.archive_plugin.files.get(identifier) {
        // Metadata arrived but carried no usable length.
        Some(_) => "—".to_string(),
        None => String::new(),
    }
}

fn draw_help_bar(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let msg = if app.archive_plugin.editing_query {
        "Editing query: [Enter] Search  •  [Esc] Cancel  •  [Ctrl-U] Clear".to_string()
    } else {
        match app.config.plugins.archive.primary_action {
            crate::config::OnlinePrimaryAction::Stream => {
                "[Enter] Stream (FLAC preferred)  •  [d] Download album  •  [/] Search  •  [c] Collection  •  [o] Source"
                    .to_string()
            }
            crate::config::OnlinePrimaryAction::Download => {
                "[Enter / d] Download album  •  [s] Stream (FLAC preferred)  •  [/] Search  •  [c] Collection  •  [o] Source"
                    .to_string()
            }
        }
    };

    frame.render_widget(Paragraph::new(msg).style(theme.dim()).centered(), area);
}
