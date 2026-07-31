use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use super::api::JamendoFormat;
use crate::app::{App, Pane, format_duration};
use crate::theme::Theme;
use crate::ui::{Hits, Region};

pub fn draw(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme, hits: &mut Hits) {
    let focused = app.focus == Pane::Online;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border(focused))
        .style(theme.base())
        .title(" Online (Jamendo — free & Creative Commons music) ")
        .title_style(theme.title());

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .split(inner);

    draw_search_bar(frame, chunks[0], app, theme, hits);
    draw_results_table(frame, chunks[1], app, theme, hits);
    draw_help_bar(frame, chunks[2], app, theme);
}

fn draw_search_bar(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme, hits: &mut Hits) {
    let splits = Layout::horizontal([Constraint::Min(30), Constraint::Length(24)]).split(area);

    let search_focused = app.jamendo_plugin.editing_query;
    let query_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border(search_focused))
        .title(" Search Query [/] ");

    let query_inner = query_block.inner(splits[0]);
    frame.render_widget(query_block, splits[0]);

    let input_width = query_inner.width as usize;
    let (query_text, query_style) = if search_focused {
        let shown = app.jamendo_plugin.query_input.display(input_width);
        let caret = app.jamendo_plugin.query_input.display_cursor(input_width);
        let mut chars: Vec<char> = shown.chars().collect();
        while chars.len() <= caret {
            chars.push(' ');
        }
        let mut text: String = chars[..caret].iter().collect();
        text.push('▏');
        text.extend(chars[caret..].iter());
        (text, theme.playing())
    } else {
        let text = if app.jamendo_plugin.query.is_empty() {
            "Search artists, tracks and genres...".to_string()
        } else {
            app.jamendo_plugin.query.clone()
        };
        (crate::ui::widgets::truncate(&text, input_width), theme.base())
    };

    frame.render_widget(Paragraph::new(query_text).style(query_style), query_inner);
    hits.push(splits[0], Region::OnlineSearch);

    let format = JamendoFormat::from_code(&app.config.plugins.jamendo.format);
    let format_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border(false))
        .title(" Format [c] ");
    let format_inner = format_block.inner(splits[1]);
    frame.render_widget(format_block, splits[1]);
    hits.push(splits[1], Region::OnlineFilter);
    frame.render_widget(
        Paragraph::new(crate::ui::widgets::truncate(
            format.label(),
            format_inner.width as usize,
        ))
        .style(theme.selected())
        .centered(),
        format_inner,
    );
}

fn draw_results_table(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme, hits: &mut Hits) {
    if app.jamendo_plugin.searching {
        let loading = vec![
            Line::from(""),
            Line::from(Span::styled(" Searching Jamendo... ", theme.playing())).centered(),
        ];
        frame.render_widget(Paragraph::new(loading), area);
        return;
    }

    if app.jamendo_plugin.results.is_empty() {
        let empty_msg = if app.config.plugins.jamendo.client_id.trim().is_empty() {
            "Jamendo needs a free client ID — get one at devportal.jamendo.com, then set it in Settings ▸ Plugins."
        } else if app.jamendo_plugin.query.is_empty() {
            "Press '/' to search Jamendo for artists, tracks or genres."
        } else {
            "No tracks found for this query on Jamendo."
        };
        let text = vec![
            Line::from(""),
            Line::from(Span::styled(empty_msg, theme.dim())).centered(),
        ];
        frame.render_widget(Paragraph::new(text), area);
        return;
    }

    let header = Row::new(
        ["", " Title", "Artist", "Album", "Length"]
            .iter()
            .map(|h| Cell::from(*h).style(theme.title()))
            .collect::<Vec<_>>(),
    )
    .height(1);

    let selected_index = app
        .jamendo_plugin
        .selection
        .index
        .min(app.jamendo_plugin.results.len().saturating_sub(1));

    let rows: Vec<Row> = app
        .jamendo_plugin
        .results
        .iter()
        .enumerate()
        .map(|(idx, track)| {
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
                    &format!("{}{}", prefix, track.name),
                    (area.width as usize).saturating_sub(52),
                )),
                Cell::from(crate::ui::widgets::truncate(&track.artist_name, 22)),
                Cell::from(crate::ui::widgets::truncate(&track.album_name, 20)),
                Cell::from(format_duration(std::time::Duration::from_secs(
                    track.duration as u64,
                ))),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(2),
        Constraint::Min(24),
        Constraint::Length(24),
        Constraint::Length(22),
        Constraint::Length(8),
    ];

    let mut state = TableState::default().with_selected(Some(selected_index));
    *state.offset_mut() = app.jamendo_plugin.selection.offset;

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(theme.selected());

    frame.render_stateful_widget(table, area, &mut state);
    app.jamendo_plugin.selection.offset = state.offset();

    // Only the rows actually on screen, starting from the table's scroll
    // offset. Registering one rect per result mapped clicks to the wrong item
    // as soon as the list scrolled — and since the Online pane activates on a
    // single click, that downloaded the wrong thing.
    let first = app.jamendo_plugin.selection.offset;
    let visible = area.height.saturating_sub(1) as usize;
    for slot in 0..visible.min(app.jamendo_plugin.results.len().saturating_sub(first)) {
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

/// The Online badge, matching what these tracks carry once queued.
fn online_badge(app: &App, theme: &Theme) -> Span<'static> {
    use crate::library::SongSource;
    Span::styled(
        crate::ui::widgets::source_glyph(SongSource::Online, app.config.glyphs),
        crate::ui::widgets::source_style(SongSource::Online, theme),
    )
}

fn draw_help_bar(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let msg = if app.jamendo_plugin.editing_query {
        "Editing query: [Enter] Search  •  [Esc] Cancel  •  [Ctrl-U] Clear".to_string()
    } else {
        match app.config.plugins.jamendo.primary_action {
            crate::config::OnlinePrimaryAction::Stream => {
                "[Enter] Play (queues the rest)  •  [d] Download  •  [/] Search  •  [c] Format  •  [o] Source"
                    .to_string()
            }
            crate::config::OnlinePrimaryAction::Download => {
                "[Enter / d] Download  •  [s] Play (queues the rest)  •  [/] Search  •  [c] Format  •  [o] Source"
                    .to_string()
            }
        }
    };

    frame.render_widget(Paragraph::new(msg).style(theme.dim()).centered(), area);
}
