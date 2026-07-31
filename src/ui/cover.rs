use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui_image::picker::Picker;
use ratatui_image::thread::{ResizeRequest, ResizeResponse, ThreadProtocol};
use ratatui_image::{Resize, StatefulImage};

use super::glyphs::Icon;
use crate::app::{App, LoadEvent};
use crate::theme::Theme;

/// Renders album art using the best graphics protocol the terminal supports.
///
/// Resizing and re-encoding an image is slow enough to be visible as a stutter,
/// and `StatefulImage` does it inline during `render`. So it is pushed onto a
/// worker thread instead: the render path only ever draws already-encoded data,
/// and a resize simply shows the previous encoding for a frame or two.
pub struct CoverRenderer {
    picker: Option<Picker>,
    protocol: Option<ThreadProtocol>,
    /// Sends resize work to the encoder thread.
    encoder: std::sync::mpsc::Sender<ResizeRequest>,
}

/// Spawn the encoder thread. Completed encodings come back through the same
/// channel the rest of the app's async work uses.
fn spawn_encoder(
    loads: tokio::sync::mpsc::UnboundedSender<LoadEvent>,
) -> std::sync::mpsc::Sender<ResizeRequest> {
    let (tx, rx) = std::sync::mpsc::channel::<ResizeRequest>();
    std::thread::spawn(move || {
        // Ends when the sender is dropped, i.e. at shutdown.
        while let Ok(request) = rx.recv() {
            if let Ok(response) = request.resize_encode() {
                // A closed receiver means the UI is gone; nothing left to do.
                if loads
                    .send(LoadEvent::CoverResized(Box::new(response)))
                    .is_err()
                {
                    return;
                }
            }
        }
    });
    tx
}

impl CoverRenderer {
    /// Query the terminal for its graphics capabilities.
    ///
    /// Must be called before entering the alternate screen, since it writes a
    /// query escape sequence to stdout and reads the reply.
    pub fn detect(loads: tokio::sync::mpsc::UnboundedSender<LoadEvent>) -> Self {
        // A terminal without graphics support is not an error: ratatui-image
        // falls back to halfblocks, which still looks reasonable.
        Self {
            picker: Picker::from_query_stdio().ok(),
            protocol: None,
            encoder: spawn_encoder(loads),
        }
    }

    /// Apply a finished encoding. Stale results (from a size the user has
    /// already moved past) are recognised by id and dropped.
    pub fn apply_resized(&mut self, response: ResizeResponse) {
        if let Some(protocol) = self.protocol.as_mut() {
            protocol.update_resized_protocol(response);
        }
    }

    fn rebuild(&mut self, bytes: &[u8]) {
        let Some(picker) = self.picker.as_ref() else {
            return;
        };
        match image::load_from_memory(bytes) {
            Ok(image) => {
                let inner = picker.new_resize_protocol(image);
                match self.protocol.as_mut() {
                    Some(protocol) => protocol.replace_protocol(inner),
                    None => {
                        self.protocol = Some(ThreadProtocol::new(self.encoder.clone(), Some(inner)))
                    }
                }
            }
            // A cover that fails to decode must not disturb playback.
            Err(_) => {
                if let Some(protocol) = self.protocol.as_mut() {
                    protocol.empty_protocol();
                }
            }
        }
    }

    /// Cell size in pixels, or a sensible default if the terminal did not say.
    fn cell_size(&self) -> (u16, u16) {
        match self.picker.as_ref() {
            Some(picker) => {
                let size = picker.font_size();
                (size.width.max(1), size.height.max(1))
            }
            None => (1, 2),
        }
    }

    fn square_within(&self, area: Rect) -> Rect {
        let (cell_w, cell_h) = self.cell_size();
        square_within(area, cell_w, cell_h)
    }

    /// Columns a square cover needs to fill `rows` of height, borders included.
    ///
    /// Lets a caller size the cover's *pane* to the artwork rather than the
    /// other way round, so the image is as large as the height allows instead
    /// of being letterboxed inside a pane that is the wrong shape.
    pub fn width_for_height(&self, rows: u16) -> u16 {
        let (cell_w, cell_h) = self.cell_size();
        let inner_rows = rows.saturating_sub(2) as u32;
        let cols = inner_rows * cell_h as u32 / cell_w.max(1) as u32;
        (cols as u16).saturating_add(2)
    }

    /// Where to draw the artwork inside `inner`, centred on what it will
    /// actually occupy.
    ///
    /// A graphics protocol cannot upscale past the source image, so on a large
    /// pane the picture can come out smaller than the space allowed for it —
    /// and it is then drawn from the *top-left* of that space, leaving a
    /// lopsided gap. Asking the protocol how many cells it will really use, and
    /// centring that, keeps the frame looking deliberate at any size.
    fn target_within(&self, inner: Rect) -> Rect {
        let square = self.square_within(inner);
        let Some(protocol) = self.protocol.as_ref() else {
            return square;
        };

        let actual = protocol.size_for(
            Resize::Fit(None),
            Size {
                width: square.width,
                height: square.height,
            },
        );
        let Some(actual) = actual else { return square };

        centre_within(inner, actual.width, actual.height)
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        // The frame fills the whole pane, so it lines up with the Lyrics frame
        // below it; only the artwork inside is constrained to its aspect ratio.
        let bg = theme.base();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(theme.border(false))
            .style(bg)
            .title(" Cover ")
            .title_style(theme.title());
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if let Some(response) = app.cover_resized.take() {
            self.apply_resized(*response);
        }

        let target = self.target_within(inner);
        if target.width == 0 || target.height == 0 {
            return;
        }

        // A new cover decodes here (cheap); every *resize* is handed to the
        // encoder thread by `StatefulImage`, which only queues the work.
        if app.cover_dirty {
            app.cover_dirty = false;
            match app.cover_bytes.as_deref() {
                Some(bytes) => self.rebuild(bytes),
                // Dropping the protocol stops us drawing, but the previous
                // picture is already on the terminal's own canvas: a track with
                // no art would otherwise keep showing the last one's.
                None => self.protocol = None,
            }
        }

        // A popup is about to be drawn over this pane. Emitting the image anyway
        // means the graphics layer and the popup's text fight over the same
        // cells, and the leftovers are what survives the popup closing.
        // The empty frame stays, so the layout does not jump; the picture comes
        // back the moment the popup is dismissed.
        if app.overlay.is_some() || app.show_help {
            return;
        }

        match self.protocol.as_mut() {
            Some(protocol) => {
                frame.render_stateful_widget(
                    StatefulImage::<ThreadProtocol>::new(),
                    target,
                    protocol,
                );
            }
            None => draw_placeholder(frame, inner, app, theme),
        }
    }
}

/// A `width` x `height` rect centred in `area`, clamped to fit.
fn centre_within(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// Largest square (in cells) that fits `area`, centred.
///
/// Terminal cells are roughly twice as tall as they are wide, so a square
/// image needs about twice as many columns as rows. Using the real font
/// metrics keeps the art square rather than stretched.
fn square_within(area: Rect, cell_w: u16, cell_h: u16) -> Rect {
    if area.width == 0 || area.height == 0 {
        return area;
    }
    // Zero metrics would mean an unusable terminal report; fall back to 1x2.
    let (cell_w, cell_h) = if cell_w == 0 || cell_h == 0 {
        (1, 2)
    } else {
        (cell_w, cell_h)
    };

    // A square in pixels: cols * cell_w == rows * cell_h.
    let rows_by_width = (area.width as u32 * cell_w as u32 / cell_h as u32) as u16;
    let rows = area.height.min(rows_by_width);
    let cols = ((rows as u32 * cell_h as u32 / cell_w as u32) as u16).min(area.width);

    Rect {
        x: area.x + (area.width.saturating_sub(cols)) / 2,
        y: area.y + (area.height.saturating_sub(rows)) / 2,
        width: cols,
        height: rows,
    }
}

/// Shown while art loads, or when a track has none.
fn draw_placeholder(frame: &mut Frame, inner: Rect, app: &App, theme: &Theme) {
    let (glyph, label) = if app.cover_id.is_some() {
        (app.config.glyphs.icon(Icon::CoverLoading), "Loading cover…")
    } else {
        (app.config.glyphs.icon(Icon::CoverMissing), "No cover art")
    };

    let lines = vec![
        Line::from(Span::styled(glyph, theme.dim())),
        Line::default(),
        Line::from(Span::styled(label, theme.dim())),
    ];

    // Vertically centre the block of text in the pane.
    let height = lines.len() as u16;
    let area = Rect {
        y: inner.y + inner.height.saturating_sub(height) / 2,
        height: height.min(inner.height),
        ..inner
    };
    frame.render_widget(Paragraph::new(lines).centered(), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_is_centred_in_a_wide_pane() {
        let square = square_within(Rect::new(0, 0, 60, 20), 10, 20);
        assert_eq!(square.width, 40);
        assert_eq!(square.height, 20);
        assert_eq!(square.x, 10, "centred horizontally");
        assert_eq!(square.y, 0);
    }

    #[test]
    fn square_is_centred_in_a_tall_pane() {
        let square = square_within(Rect::new(0, 0, 20, 40), 10, 20);
        assert_eq!(square.width, 20);
        assert_eq!(square.height, 10);
        assert_eq!(square.y, 15, "centred vertically");
    }

    #[test]
    fn square_never_exceeds_the_available_area() {
        for (w, h) in [(1, 1), (3, 40), (80, 2), (0, 0)] {
            let area = Rect::new(0, 0, w, h);
            let square = square_within(area, 8, 16);
            assert!(square.width <= area.width, "{w}x{h} overflowed width");
            assert!(square.height <= area.height, "{w}x{h} overflowed height");
        }
    }

    #[test]
    fn falls_back_to_a_sane_aspect_without_font_metrics() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let r = CoverRenderer {
            picker: None,
            protocol: None,
            encoder: tx,
        };
        assert_eq!(r.cell_size(), (1, 2));
        let square = r.square_within(Rect::new(0, 0, 40, 20));
        // Default 1x2 cells: 40 cols wide == 20 rows tall in pixels.
        assert_eq!((square.width, square.height), (40, 20));
    }

    #[test]
    fn width_for_height_produces_a_pane_the_artwork_fills() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let r = CoverRenderer {
            picker: None,
            protocol: None,
            encoder: tx,
        };

        for rows in [10u16, 20, 33, 40] {
            let width = r.width_for_height(rows);
            let inner = Rect::new(1, 1, width - 2, rows - 2);
            let square = r.square_within(inner);
            // The whole point: the image is limited by the height, not boxed in
            // by a pane that is the wrong shape.
            assert_eq!(
                square.height, inner.height,
                "artwork did not fill {rows} rows (pane {width} wide)"
            );
        }
    }

    #[test]
    fn width_for_height_survives_a_pane_too_short_for_borders() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let r = CoverRenderer {
            picker: None,
            protocol: None,
            encoder: tx,
        };
        assert!(r.width_for_height(0) >= 2);
        assert!(r.width_for_height(1) >= 2);
    }

    #[test]
    fn artwork_smaller_than_its_pane_is_centred_not_cornered() {
        let pane = Rect::new(10, 5, 80, 40);
        // A protocol that cannot fill the pane, because the source image ran
        // out of pixels.
        let target = centre_within(pane, 40, 20);
        assert_eq!(target.width, 40);
        assert_eq!(target.height, 20);
        assert_eq!(target.x, 10 + 20, "equal gap either side");
        assert_eq!(target.y, 5 + 10, "equal gap above and below");
    }

    #[test]
    fn artwork_larger_than_its_pane_is_clamped_to_it() {
        let pane = Rect::new(0, 0, 10, 4);
        let target = centre_within(pane, 999, 999);
        assert_eq!((target.width, target.height), (10, 4));
        assert_eq!((target.x, target.y), (0, 0));
    }

    #[test]
    fn zero_cell_metrics_do_not_divide_by_zero() {
        let square = square_within(Rect::new(0, 0, 10, 10), 0, 0);
        assert!(square.width <= 10 && square.height <= 10);
    }
}
