use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// A horizontal slider with a filled track, a knob, and an optional gradient.
///
/// Shared by the seek bar and the volume control so they look and behave
/// identically, and so mouse hit-testing can use one `ratio_at` for both.
pub struct Slider {
    pub ratio: f64,
    pub filled_from: Color,
    pub filled_to: Color,
    pub empty: Color,
    pub knob: Option<char>,
}

impl Slider {
    pub fn new(ratio: f64, filled_from: Color, filled_to: Color, empty: Color) -> Self {
        Self {
            ratio: ratio.clamp(0.0, 1.0),
            filled_from,
            filled_to,
            empty,
            knob: None,
        }
    }

    pub fn knob(mut self, knob: Option<char>) -> Self {
        self.knob = knob;
        self
    }

    /// Render into a line `width` cells wide.
    pub fn render(&self, width: u16) -> Line<'static> {
        let width = width as usize;
        if width == 0 {
            return Line::default();
        }

        let track = if self.knob.is_some() {
            width.saturating_sub(1)
        } else {
            width
        };
        let knob_at = (self.ratio * track as f64).round() as usize;

        let exact = self.ratio * track as f64;
        let full = exact.floor() as usize;

        let mut spans = Vec::with_capacity(width);
        for cell in 0..track {
            let color = gradient(
                self.filled_from,
                self.filled_to,
                cell as f64 / track.max(1) as f64,
            );
            if cell < full {
                spans.push(Span::styled("━", Style::default().fg(color)));
            } else {
                spans.push(Span::styled("─", Style::default().fg(self.empty)));
            }
        }

        if let Some(knob) = self.knob {
            let color = gradient(self.filled_from, self.filled_to, self.ratio);
            let idx = knob_at.min(spans.len());
            if idx < spans.len() {
                spans[idx] = Span::styled(knob.to_string(), Style::default().fg(color));
            } else {
                spans.push(Span::styled(knob.to_string(), Style::default().fg(color)));
            }
        }

        Line::from(spans)
    }

    /// Ratio corresponding to a click `offset` cells into a slider of `width`.
    pub fn ratio_at(offset: u16, width: u16) -> f64 {
        let track = width.saturating_sub(1).max(1) as f64;
        (offset as f64 / track).clamp(0.0, 1.0)
    }
}

/// Linear interpolation between two colors. Non-RGB colors are passed through,
/// since there is no meaningful way to blend palette indices.
pub fn gradient(from: Color, to: Color, t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (from, to) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let mix = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * t).round() as u8;
            Color::Rgb(mix(r1, r2), mix(g1, g2), mix(b1, b2))
        }
        _ => from,
    }
}

/// Truncate to `width` display cells, appending an ellipsis when it does not
/// fit. Uses character counts, which is adequate for the Latin/CJK mix in
/// library metadata and avoids pulling in a full width-measuring dependency.
pub fn truncate(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// A single-line text field.
///
/// The settings panel and the setup wizard both edit free text — server URLs,
/// usernames, passwords, folder paths — which nothing in the app could do
/// before. Kept deliberately small: no selection, no clipboard, no scrolling
/// history, just enough to type a value correctly.
#[derive(Debug, Default, Clone)]
pub struct TextInput {
    value: String,
    /// Caret position as a character index, not a byte offset, so multi-byte
    /// input cannot split a character.
    cursor: usize,
    /// Render as bullets. Set for the password field.
    pub masked: bool,
}

impl TextInput {
    pub fn new(value: impl Into<String>) -> Self {
        let value: String = value.into();
        let cursor = value.chars().count();
        Self {
            value,
            cursor,
            masked: false,
        }
    }

    pub fn masked(mut self, masked: bool) -> Self {
        self.masked = masked;
        self
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    fn byte_at(&self, index: usize) -> usize {
        self.value
            .char_indices()
            .nth(index)
            .map(|(byte, _)| byte)
            .unwrap_or(self.value.len())
    }

    pub fn insert(&mut self, ch: char) {
        let at = self.byte_at(self.cursor);
        self.value.insert(at, ch);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let from = self.byte_at(self.cursor - 1);
        let to = self.byte_at(self.cursor);
        self.value.replace_range(from..to, "");
        self.cursor -= 1;
    }

    pub fn delete(&mut self) {
        let len = self.value.chars().count();
        if self.cursor >= len {
            return;
        }
        let from = self.byte_at(self.cursor);
        let to = self.byte_at(self.cursor + 1);
        self.value.replace_range(from..to, "");
    }

    /// Delete the word before the caret, as Ctrl-W does in a shell.
    pub fn delete_word(&mut self) {
        // Skip the run of spaces first, so deleting after "foo   " removes the
        // spaces *and* the word, rather than only the spaces.
        while self.cursor > 0 && self.char_before().is_some_and(char::is_whitespace) {
            self.backspace();
        }
        while self.cursor > 0 && self.char_before().is_some_and(|c| !c.is_whitespace()) {
            self.backspace();
        }
    }

    fn char_before(&self) -> Option<char> {
        self.value.chars().nth(self.cursor.checked_sub(1)?)
    }

    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.value.chars().count());
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.value.chars().count();
    }

    /// First visible character, chosen so the caret is always inside the
    /// window. The caret needs a cell of its own, which is why it scrolls at
    /// `width - 1` rather than at `width`.
    fn window_start(&self, width: usize) -> usize {
        if width == 0 {
            return 0;
        }
        self.cursor.saturating_sub(width.saturating_sub(1))
    }

    /// The text to draw, masked if required, windowed to `width` cells.
    pub fn display(&self, width: usize) -> String {
        if width == 0 {
            return String::new();
        }
        let shown: String = if self.masked {
            "•".repeat(self.value.chars().count())
        } else {
            self.value.clone()
        };
        shown
            .chars()
            .skip(self.window_start(width))
            .take(width)
            .collect()
    }

    /// Caret offset within the string [`Self::display`] returned.
    pub fn display_cursor(&self, width: usize) -> usize {
        self.cursor - self.window_start(width)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Color = Color::Rgb(0, 0, 0);
    const B: Color = Color::Rgb(100, 100, 100);

    #[test]
    fn slider_renders_exactly_the_requested_width() {
        for ratio in [0.0, 0.25, 0.5, 0.99, 1.0] {
            let line = Slider::new(ratio, A, B, A).render(20);
            let cells: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            assert_eq!(cells, 20, "ratio {ratio} produced {cells} cells");
        }
    }

    #[test]
    fn slider_handles_zero_width() {
        assert_eq!(Slider::new(0.5, A, B, A).render(0).spans.len(), 0);
    }

    #[test]
    fn knob_sits_at_both_extremes() {
        let start = Slider::new(0.0, A, B, A).knob(Some('⏺')).render(10);
        assert_eq!(start.spans[0].content.as_ref(), "⏺");
        let end = Slider::new(1.0, A, B, A).knob(Some('⏺')).render(10);
        assert_eq!(end.spans.last().unwrap().content.as_ref(), "⏺");
    }

    #[test]
    fn click_maps_to_the_ratio_it_looks_like() {
        assert_eq!(Slider::ratio_at(0, 11), 0.0);
        assert_eq!(Slider::ratio_at(10, 11), 1.0);
        assert!((Slider::ratio_at(5, 11) - 0.5).abs() < 1e-9);
        // Clicking past the end clamps rather than seeking beyond the track.
        assert_eq!(Slider::ratio_at(99, 11), 1.0);
    }

    #[test]
    fn gradient_interpolates_rgb_endpoints() {
        assert_eq!(gradient(A, B, 0.0), A);
        assert_eq!(gradient(A, B, 1.0), B);
        assert_eq!(gradient(A, B, 0.5), Color::Rgb(50, 50, 50));
    }

    #[test]
    fn gradient_passes_through_non_rgb() {
        assert_eq!(gradient(Color::Cyan, Color::Red, 0.5), Color::Cyan);
    }

    #[test]
    fn truncate_adds_ellipsis_only_when_needed() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello world", 8), "hello w…");
        assert_eq!(truncate("hello", 0), "");
    }

    #[test]
    fn truncate_counts_multibyte_characters() {
        // CJK metadata is common in this library; must not panic or split chars.
        assert_eq!(truncate("初音ミクの曲", 4), "初音ミ…");
    }
}

#[cfg(test)]
mod input_tests {
    use super::TextInput;

    #[test]
    fn typing_and_deleting_track_the_caret() {
        let mut input = TextInput::default();
        for ch in "abc".chars() {
            input.insert(ch);
        }
        assert_eq!(input.value(), "abc");

        input.left();
        input.insert('X');
        assert_eq!(input.value(), "abXc");

        input.backspace();
        assert_eq!(input.value(), "abc");
    }

    /// Byte offsets and character offsets diverge as soon as the user types a
    /// non-ASCII character, and a URL or folder path may well contain one.
    #[test]
    fn multi_byte_characters_are_not_split() {
        let mut input = TextInput::new("héllo");
        input.home();
        input.right();
        input.delete();
        assert_eq!(input.value(), "hllo");

        let mut input = TextInput::new("日本語");
        input.backspace();
        assert_eq!(input.value(), "日本");
    }

    #[test]
    fn delete_word_removes_trailing_space_and_the_word() {
        let mut input = TextInput::new("one two   ");
        input.delete_word();
        assert_eq!(input.value(), "one ");
        input.delete_word();
        assert_eq!(input.value(), "");
    }

    #[test]
    fn masking_hides_the_value_but_keeps_its_length() {
        let input = TextInput::new("secret").masked(true);
        assert_eq!(input.display(20), "••••••");
        assert!(!input.display(20).contains("secret"));
    }

    /// A long URL in a narrow field must still show where the user is typing.
    #[test]
    fn the_window_follows_the_caret_in_a_narrow_field() {
        let mut input = TextInput::new("https://music.example.com/subsonic");
        let shown = input.display(10);
        assert!(
            shown.ends_with("subsonic"),
            "caret at the end shows the tail"
        );
        // The caret sits just past the last character, still inside the field.
        assert!(shown.chars().count() < 10);
        assert!(input.display_cursor(10) < 10);

        input.home();
        assert_eq!(
            input.display(10),
            "https://mu",
            "caret at the start shows the head"
        );
        assert_eq!(input.display_cursor(10), 0);

        // Every caret position must map inside the visible window.
        for _ in 0..40 {
            input.right();
            assert!(input.display_cursor(10) < 10);
        }
    }

    #[test]
    fn caret_movement_is_clamped_to_the_value() {
        let mut input = TextInput::new("ab");
        // Walking past the end must not move the caret out of the value.
        input.right();
        input.right();
        input.right();
        input.insert('c');
        assert_eq!(input.value(), "abc");

        input.left();
        input.left();
        input.left();
        input.left();
        input.backspace();
        assert_eq!(input.value(), "abc", "backspace at the start is a no-op");
        input.insert('z');
        assert_eq!(input.value(), "zabc", "caret is clamped to the start");
    }
}
