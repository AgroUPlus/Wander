use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::widgets::gradient;
use crate::player::spectrum::Spectrum;
use crate::theme::Theme;

/// Vertical eighths, so a bar can end part-way through a cell.
const BLOCKS: [char; 8] = [' ', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// One space between bars, so they read as distinct columns.
const BAR_WIDTH: u16 = 2;

/// Spectrum bars, drawn bottom-up with a vertical colour gradient.
pub fn draw(frame: &mut Frame, area: Rect, spectrum: &mut Spectrum, theme: &Theme) {
    if area.width < 2 || area.height == 0 {
        return;
    }

    let count = (area.width / BAR_WIDTH).max(1) as usize;
    spectrum.resize(count);
    spectrum.update();

    let bars = spectrum.bars();
    let rows = area.height;

    let pad_len = (area.width.saturating_sub(count as u16 * BAR_WIDTH) / 2) as usize;
    let pad_str = " ".repeat(pad_len);

    // Build the picture row by row, from the top down.
    let mut lines = Vec::with_capacity(rows as usize);
    for row in 0..rows {
        // How far up the display this row sits, 0 at the bottom.
        let from_bottom = (rows - 1 - row) as f32;
        let mut spans = Vec::with_capacity(count * 2 + 1);
        if !pad_str.is_empty() {
            spans.push(Span::raw(pad_str.clone()));
        }

        for &level in bars.iter().take(count) {
            let height = level * rows as f32;
            let filled = height - from_bottom;

            let glyph = if filled >= 1.0 {
                Some(BLOCKS[7])
            } else if filled > 0.0 {
                Some(BLOCKS[((filled * 8.0).ceil() as usize).clamp(1, 8) - 1])
            } else {
                None
            };

            match glyph {
                Some(glyph) => {
                    // Colour by height, so bar tops differ from their bases.
                    let t = (from_bottom / rows as f32).clamp(0.0, 1.0) as f64;
                    let color = gradient(theme.viz_low.0, theme.viz_high.0, t);
                    spans.push(Span::styled(glyph.to_string(), Style::default().fg(color)));
                }
                None => spans.push(Span::raw(" ")),
            }
            spans.push(Span::raw(" "));
        }

        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_width_divides_the_available_columns() {
        // Guards the count calculation used to size the spectrum.
        for width in [2u16, 10, 41, 80] {
            let count = (width / BAR_WIDTH).max(1);
            assert!(count * BAR_WIDTH <= width + 1, "width {width} overflowed");
            assert!(count >= 1);
        }
    }
}
