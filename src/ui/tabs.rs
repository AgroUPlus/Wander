use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use super::{Hits, Region};
use crate::app::{App, Tab};
use crate::theme::Theme;

/// Tab bar. Each tab is registered as a click target so the mouse can switch
/// tabs, and the rendering is laid out manually (rather than with `Tabs`) so we
/// know each tab's exact rect.
pub fn draw(frame: &mut Frame, area: Rect, app: &App, theme: &Theme, hits: &mut Hits) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border(false));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut spans = Vec::new();
    let mut x = inner.x;

    let available = Tab::available(&app.config);
    for (index, tab) in available.iter().enumerate() {
        let label = format!("  {}  ", tab.title());
        let width = label.chars().count() as u16;

        let style = if *tab == app.tab {
            theme.selected()
        } else {
            theme.dim()
        };
        spans.push(Span::styled(label, style));

        if x < inner.right() {
            hits.push(
                Rect {
                    x,
                    y: inner.y,
                    width: width.min(inner.right().saturating_sub(x)),
                    height: inner.height,
                },
                Region::Tab(index),
            );
        }
        x = x.saturating_add(width);
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
}
