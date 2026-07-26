//! The keybinding cheat sheet.
//!
//! Every entry is generated from the live [`crate::keymap::Keymap`], so the
//! sheet can never describe a key that is not actually bound — including keys
//! the user has rebound.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::app::App;
use crate::keymap::Category;
use crate::theme::Theme;

/// Leave this much of the screen visible around the sheet.
const MARGIN: u16 = 4;
/// Narrowest a column can usefully be; below this the sheet uses fewer.
const MIN_COLUMN: u16 = 32;
/// Gap between the key column and its description.
const KEY_GAP: usize = 2;

pub fn draw(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let groups = app.keymap.describe_grouped();
    if area.width < 20 || area.height < 6 {
        return;
    }

    let max_width = area.width.saturating_sub(MARGIN).min(112);
    let column_count = (max_width / MIN_COLUMN).clamp(1, 3) as usize;

    // Keys are right-aligned in a column of their own so every description
    // starts on the same screen column — including the section headings.
    let key_width = groups
        .iter()
        .flat_map(|(_, entries)| entries.iter())
        .map(|(keys, _)| keys.chars().count())
        .max()
        .unwrap_or(8)
        .min(14);

    let columns = pack(&groups, column_count);
    // Size the popup to the content: a cheat sheet that silently drops the
    // bindings that did not fit is worse than one that is simply tall.
    let needed_height = columns.iter().map(|c| c.len()).max().unwrap_or(0) as u16;
    let height = (needed_height + 3).min(area.height.saturating_sub(2));
    let width = max_width;

    let popup = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border(true))
        .style(theme.base())
        .title(" Keybindings ")
        .title_style(theme.title());
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.width == 0 || inner.height < 2 {
        return;
    }

    let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
    let areas = Layout::horizontal(
        (0..column_count)
            .map(|_| Constraint::Ratio(1, column_count as u32))
            .collect::<Vec<_>>(),
    )
    .split(rows[0]);

    let mut clipped = false;
    for (column, column_area) in columns.iter().zip(areas.iter()) {
        let visible = column_area.height as usize;
        clipped |= column.len() > visible;

        let lines: Vec<Line> = column
            .iter()
            .take(visible)
            .map(|entry| match entry {
                Entry::Blank => Line::default(),
                Entry::Heading(title) => Line::from(vec![
                    Span::raw(" ".repeat(key_width + KEY_GAP)),
                    Span::styled(title.to_string(), theme.playing()),
                ]),
                Entry::Binding { keys, description } => Line::from(vec![
                    Span::styled(
                        format!("{:>key_width$}{}", keys, " ".repeat(KEY_GAP)),
                        theme.title(),
                    ),
                    Span::styled(description.clone(), theme.base()),
                ]),
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), *column_area);
    }

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            if clipped {
                "window too short — some bindings are hidden  ·  any key to close"
            } else {
                "any key to close"
            },
            theme.dim(),
        )))
        .centered(),
        rows[1],
    );
}

/// One rendered row of the sheet.
#[derive(Debug, Clone, PartialEq)]
enum Entry {
    Heading(&'static str),
    Binding { keys: String, description: String },
    Blank,
}

/// Distribute sections over `column_count` columns, keeping them balanced.
///
/// Round-robin assignment looks tidy in code but leaves ragged columns, because
/// the sections differ wildly in length. Placing each section in whichever
/// column is currently shortest keeps the block roughly rectangular, and keeps
/// a section's bindings together.
fn pack(groups: &[(Category, Vec<(String, String)>)], column_count: usize) -> Vec<Vec<Entry>> {
    let mut columns: Vec<Vec<Entry>> = vec![Vec::new(); column_count.max(1)];

    for (category, entries) in groups {
        let target = columns
            .iter()
            .enumerate()
            .min_by_key(|(index, column)| (column.len(), *index))
            .map(|(index, _)| index)
            .unwrap_or(0);

        let column = &mut columns[target];
        if !column.is_empty() {
            column.push(Entry::Blank);
        }
        column.push(Entry::Heading(category.title()));
        for (keys, description) in entries {
            column.push(Entry::Binding {
                keys: keys.clone(),
                description: description.clone(),
            });
        }
    }
    columns
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(category: Category, count: usize) -> (Category, Vec<(String, String)>) {
        let entries = (0..count)
            .map(|i| (format!("k{i}"), format!("does thing {i}")))
            .collect();
        (category, entries)
    }

    #[test]
    fn packing_keeps_columns_balanced() {
        let groups = vec![
            group(Category::Playback, 11),
            group(Category::Navigation, 9),
            group(Category::Library, 8),
            group(Category::Queue, 6),
            group(Category::Panels, 8),
            group(Category::Misc, 3),
        ];
        let columns = pack(&groups, 3);

        let heights: Vec<usize> = columns.iter().map(|c| c.len()).collect();
        let (min, max) = (
            *heights.iter().min().unwrap(),
            *heights.iter().max().unwrap(),
        );
        // Round-robin would put sections 0 and 3 together (11 + 6 + heading
        // rows) against 2 and 5 (8 + 3), a far wider spread than this.
        assert!(max - min <= 6, "columns are ragged: {heights:?}");
    }

    #[test]
    fn every_binding_survives_packing() {
        let groups = vec![
            group(Category::Playback, 4),
            group(Category::Queue, 7),
            group(Category::Misc, 2),
        ];
        let packed = pack(&groups, 2);
        let bindings = packed
            .iter()
            .flatten()
            .filter(|entry| matches!(entry, Entry::Binding { .. }))
            .count();
        assert_eq!(bindings, 13);

        let headings = packed
            .iter()
            .flatten()
            .filter(|entry| matches!(entry, Entry::Heading(_)))
            .count();
        assert_eq!(headings, 3, "each section keeps exactly one heading");
    }

    #[test]
    fn a_sections_bindings_stay_with_their_heading() {
        let groups = vec![group(Category::Playback, 3), group(Category::Misc, 3)];
        for column in pack(&groups, 2) {
            // Within a column, a binding must always be preceded by a heading.
            let mut seen_heading = false;
            for entry in &column {
                match entry {
                    Entry::Heading(_) => seen_heading = true,
                    Entry::Binding { .. } => assert!(seen_heading, "orphaned binding"),
                    Entry::Blank => {}
                }
            }
        }
    }

    #[test]
    fn a_single_column_still_lays_out() {
        let groups = vec![group(Category::Playback, 2)];
        let packed = pack(&groups, 1);
        assert_eq!(packed.len(), 1);
        assert_eq!(packed[0][0], Entry::Heading("Playback"));
        // No leading blank before the very first section.
        assert!(!matches!(packed[0][0], Entry::Blank));
    }
}
