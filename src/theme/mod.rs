pub mod palette;

use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};

pub use palette::Palette;

/// Colors are parsed once at startup into ratatui styles. Everything the UI
/// draws must come from here so a user theme can restyle the whole app.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub background: ThemeColor,
    pub foreground: ThemeColor,
    pub border: ThemeColor,
    pub border_focused: ThemeColor,
    pub accent: ThemeColor,
    pub highlight_bg: ThemeColor,
    pub highlight_fg: ThemeColor,
    pub current_track: ThemeColor,
    pub dim: ThemeColor,
    pub progress: ThemeColor,
    pub error: ThemeColor,
    /// Visualiser gradient: `viz_low` at the base of a bar, `viz_high` at the top.
    pub viz_low: ThemeColor,
    pub viz_high: ThemeColor,
}

impl Default for Theme {
    fn default() -> Self {
        Self::tokyo_night()
    }
}

impl Theme {
    pub const PRESET_NAMES: &'static [&'static str] = &[
        "Terminal (ANSI 16)",
        "Tokyo Night",
        "Catppuccin Mocha",
        "Nord",
        "Dracula",
        "Cyberpunk",
        "OLED Black",
        "Gruvbox Dark",
        "Rosé Pine",
        "Everforest",
        "Kanagawa",
        "Catppuccin Latte",
        "Solarized Dark",
        "Ayu Mirage",
        "Monokai Pro",
    ];

    pub fn preset(name: &str) -> Self {
        match name {
            "Terminal (ANSI 16)" => Self::terminal_ansi(),
            "Catppuccin Mocha" => Self::catppuccin_mocha(),
            "Nord" => Self::nord(),
            "Dracula" => Self::dracula(),
            "Cyberpunk" => Self::cyberpunk(),
            "OLED Black" => Self::oled_black(),
            "Gruvbox Dark" => Self::gruvbox_dark(),
            "Rosé Pine" => Self::rose_pine(),
            "Everforest" => Self::everforest(),
            "Kanagawa" => Self::kanagawa(),
            "Catppuccin Latte" => Self::catppuccin_latte(),
            "Solarized Dark" => Self::solarized_dark(),
            "Ayu Mirage" => Self::ayu_mirage(),
            "Monokai Pro" => Self::monokai_pro(),
            _ => Self::tokyo_night(),
        }
    }

    /// Build a preset from hex literals.
    ///
    /// The 13 colours are always given in the same order as the struct fields,
    /// which keeps each palette to a readable block instead of 13 lines of
    /// `ThemeColor(Color::Rgb(..))`.
    #[allow(clippy::too_many_arguments)]
    fn from_hex(
        background: Color,
        [
            foreground,
            border,
            border_focused,
            accent,
            highlight_bg,
            highlight_fg,
            current_track,
            dim,
            progress,
            error,
            viz_low,
            viz_high,
        ]: [u32; 12],
    ) -> Self {
        Self {
            background: ThemeColor(background),
            foreground: rgb(foreground),
            border: rgb(border),
            border_focused: rgb(border_focused),
            accent: rgb(accent),
            highlight_bg: rgb(highlight_bg),
            highlight_fg: rgb(highlight_fg),
            current_track: rgb(current_track),
            dim: rgb(dim),
            progress: rgb(progress),
            error: rgb(error),
            viz_low: rgb(viz_low),
            viz_high: rgb(viz_high),
        }
    }

    pub fn gruvbox_dark() -> Self {
        Self::from_hex(
            Color::Reset,
            [
                0xebdbb2, 0x504945, 0xfabd2f, 0xfabd2f, 0x504945, 0xfbf1c7, 0x8ec07c, 0x928374,
                0xd79921, 0xfb4934, 0xb8bb26, 0xfe8019,
            ],
        )
    }

    pub fn rose_pine() -> Self {
        Self::from_hex(
            Color::Reset,
            [
                0xe0def4, 0x403d52, 0xc4a7e7, 0xc4a7e7, 0x403d52, 0xe0def4, 0x9ccfd8, 0x6e6a86,
                0xebbcba, 0xeb6f92, 0xf6c177, 0xc4a7e7,
            ],
        )
    }

    pub fn everforest() -> Self {
        Self::from_hex(
            Color::Reset,
            [
                0xd3c6aa, 0x4f5b58, 0xa7c080, 0xa7c080, 0x475258, 0xd3c6aa, 0x83c092, 0x859289,
                0xdbbc7f, 0xe67e80, 0xa7c080, 0x7fbbb3,
            ],
        )
    }

    pub fn kanagawa() -> Self {
        Self::from_hex(
            Color::Reset,
            [
                0xdcd7ba, 0x54546d, 0x7e9cd8, 0x7e9cd8, 0x2d4f67, 0xdcd7ba, 0x7aa89f, 0x727169,
                0xe6c384, 0xe82424, 0xd27e99, 0x7fb4ca,
            ],
        )
    }

    /// The one light preset, for terminals with a light background.
    pub fn catppuccin_latte() -> Self {
        Self::from_hex(
            Color::Reset,
            [
                0x4c4f69, 0xbcc0cc, 0x8839ef, 0x8839ef, 0xdce0e8, 0x4c4f69, 0x179299, 0x8c8fa1,
                0xea76cb, 0xd20f39, 0xfe640b, 0x8839ef,
            ],
        )
    }

    pub fn solarized_dark() -> Self {
        Self::from_hex(
            Color::Reset,
            [
                0x93a1a1, 0x073642, 0x268bd2, 0x268bd2, 0x073642, 0xfdf6e3, 0x2aa198, 0x586e75,
                0xb58900, 0xdc322f, 0x859900, 0x6c71c4,
            ],
        )
    }

    pub fn ayu_mirage() -> Self {
        Self::from_hex(
            Color::Reset,
            [
                0xcccac2, 0x393b41, 0xffcc66, 0xffcc66, 0x33415e, 0xcccac2, 0x5ccfe6, 0x707a8c,
                0xffd580, 0xff6666, 0xbae67e, 0x73d0ff,
            ],
        )
    }

    pub fn monokai_pro() -> Self {
        Self::from_hex(
            Color::Reset,
            [
                0xfcfcfa, 0x5b595c, 0xffd866, 0xffd866, 0x423f42, 0xfcfcfa, 0x78dce8, 0x939293,
                0xa9dc76, 0xff6188, 0xab9df2, 0xfc9867,
            ],
        )
    }

    pub fn terminal_ansi() -> Self {
        Self {
            background: ThemeColor(Color::Reset),
            foreground: ThemeColor(Color::Reset),
            border: ThemeColor(Color::Indexed(8)), // Dark Gray (color 8)
            border_focused: ThemeColor(Color::Indexed(6)), // Cyan (color 6)
            accent: ThemeColor(Color::Indexed(6)), // Cyan (color 6)
            highlight_bg: ThemeColor(Color::Indexed(8)), // Dark Gray selection background
            highlight_fg: ThemeColor(Color::Indexed(15)), // Bright White text
            current_track: ThemeColor(Color::Indexed(2)), // Green (color 2)
            dim: ThemeColor(Color::Indexed(8)),    // Dark Gray (color 8)
            progress: ThemeColor(Color::Indexed(4)), // Blue (color 4)
            error: ThemeColor(Color::Indexed(1)),  // Red (color 1)
            viz_low: ThemeColor(Color::Indexed(2)), // Green (color 2)
            viz_high: ThemeColor(Color::Indexed(5)), // Magenta (color 5)
        }
    }

    pub fn tokyo_night() -> Self {
        Self {
            background: ThemeColor(Color::Reset),
            foreground: ThemeColor(Color::Rgb(0xd0, 0xd8, 0xe8)),
            border: ThemeColor(Color::Rgb(0x3a, 0x44, 0x5e)),
            border_focused: ThemeColor(Color::Rgb(0x7a, 0xa2, 0xf7)),
            accent: ThemeColor(Color::Rgb(0x7a, 0xa2, 0xf7)),
            highlight_bg: ThemeColor(Color::Rgb(0x41, 0x5a, 0x8c)),
            highlight_fg: ThemeColor(Color::Rgb(0xff, 0xff, 0xff)),
            current_track: ThemeColor(Color::Rgb(0x7d, 0xcf, 0xff)),
            dim: ThemeColor(Color::Rgb(0x6b, 0x74, 0x8b)),
            progress: ThemeColor(Color::Rgb(0x7a, 0xa2, 0xf7)),
            error: ThemeColor(Color::Rgb(0xf7, 0x76, 0x8e)),
            viz_low: ThemeColor(Color::Rgb(0xff, 0x2d, 0x9b)),
            viz_high: ThemeColor(Color::Rgb(0x3b, 0xc9, 0xf0)),
        }
    }

    pub fn catppuccin_mocha() -> Self {
        Self {
            background: ThemeColor(Color::Reset),
            foreground: ThemeColor(Color::Rgb(0xcd, 0xd6, 0xf4)),
            border: ThemeColor(Color::Rgb(0x45, 0x47, 0x5a)),
            border_focused: ThemeColor(Color::Rgb(0xcb, 0xa6, 0xf7)),
            accent: ThemeColor(Color::Rgb(0xcb, 0xa6, 0xf7)),
            highlight_bg: ThemeColor(Color::Rgb(0x58, 0x5b, 0x70)),
            highlight_fg: ThemeColor(Color::Rgb(0xf5, 0xe0, 0xdc)),
            current_track: ThemeColor(Color::Rgb(0x89, 0xdc, 0xeb)),
            dim: ThemeColor(Color::Rgb(0x6c, 0x70, 0x86)),
            progress: ThemeColor(Color::Rgb(0xf5, 0xc2, 0xe7)),
            error: ThemeColor(Color::Rgb(0xf3, 0x8b, 0xa8)),
            viz_low: ThemeColor(Color::Rgb(0xfa, 0xb3, 0x87)),
            viz_high: ThemeColor(Color::Rgb(0xcb, 0xa6, 0xf7)),
        }
    }

    pub fn nord() -> Self {
        Self {
            background: ThemeColor(Color::Reset),
            foreground: ThemeColor(Color::Rgb(0xec, 0xef, 0xf4)),
            border: ThemeColor(Color::Rgb(0x4c, 0x56, 0x6a)),
            border_focused: ThemeColor(Color::Rgb(0x88, 0xc0, 0xd0)),
            accent: ThemeColor(Color::Rgb(0x88, 0xc0, 0xd0)),
            highlight_bg: ThemeColor(Color::Rgb(0x43, 0x4c, 0x5e)),
            highlight_fg: ThemeColor(Color::Rgb(0xeb, 0xcb, 0x8b)),
            current_track: ThemeColor(Color::Rgb(0x8f, 0xbc, 0xbb)),
            dim: ThemeColor(Color::Rgb(0x4c, 0x56, 0x6a)),
            progress: ThemeColor(Color::Rgb(0x81, 0xa1, 0xc1)),
            error: ThemeColor(Color::Rgb(0xbf, 0x61, 0x6a)),
            viz_low: ThemeColor(Color::Rgb(0xa3, 0xbe, 0x8c)),
            viz_high: ThemeColor(Color::Rgb(0xb4, 0x8e, 0xad)),
        }
    }

    pub fn dracula() -> Self {
        Self {
            background: ThemeColor(Color::Reset),
            foreground: ThemeColor(Color::Rgb(0xf8, 0xf8, 0xf2)),
            border: ThemeColor(Color::Rgb(0x62, 0x72, 0xa4)),
            border_focused: ThemeColor(Color::Rgb(0xbd, 0x93, 0xf9)),
            accent: ThemeColor(Color::Rgb(0xff, 0x79, 0xc6)),
            highlight_bg: ThemeColor(Color::Rgb(0x44, 0x47, 0x5a)),
            highlight_fg: ThemeColor(Color::Rgb(0x50, 0xfa, 0x7b)),
            current_track: ThemeColor(Color::Rgb(0x8b, 0xe9, 0xfd)),
            dim: ThemeColor(Color::Rgb(0x62, 0x72, 0xa4)),
            progress: ThemeColor(Color::Rgb(0xff, 0x79, 0xc6)),
            error: ThemeColor(Color::Rgb(0xff, 0x55, 0x55)),
            viz_low: ThemeColor(Color::Rgb(0xf1, 0xfa, 0x8c)),
            viz_high: ThemeColor(Color::Rgb(0xbd, 0x93, 0xf9)),
        }
    }

    pub fn cyberpunk() -> Self {
        Self {
            background: ThemeColor(Color::Reset),
            foreground: ThemeColor(Color::Rgb(0x00, 0xff, 0x9f)),
            border: ThemeColor(Color::Rgb(0xff, 0x00, 0x55)),
            border_focused: ThemeColor(Color::Rgb(0x00, 0xe5, 0xff)),
            accent: ThemeColor(Color::Rgb(0xff, 0x00, 0x7f)),
            highlight_bg: ThemeColor(Color::Rgb(0x2a, 0x00, 0x3b)),
            highlight_fg: ThemeColor(Color::Rgb(0xff, 0xf0, 0x00)),
            current_track: ThemeColor(Color::Rgb(0x00, 0xe5, 0xff)),
            dim: ThemeColor(Color::Rgb(0x70, 0x50, 0x80)),
            progress: ThemeColor(Color::Rgb(0xff, 0x00, 0x7f)),
            error: ThemeColor(Color::Rgb(0xff, 0x22, 0x22)),
            viz_low: ThemeColor(Color::Rgb(0xff, 0x00, 0x55)),
            viz_high: ThemeColor(Color::Rgb(0x00, 0xe5, 0xff)),
        }
    }

    pub fn oled_black() -> Self {
        Self {
            background: ThemeColor(Color::Black),
            foreground: ThemeColor(Color::Rgb(0xee, 0xee, 0xee)),
            border: ThemeColor(Color::Rgb(0x33, 0x33, 0x33)),
            border_focused: ThemeColor(Color::Rgb(0xff, 0xff, 0xff)),
            accent: ThemeColor(Color::Rgb(0xff, 0xff, 0xff)),
            highlight_bg: ThemeColor(Color::Rgb(0x22, 0x22, 0x22)),
            highlight_fg: ThemeColor(Color::Rgb(0xff, 0xff, 0xff)),
            current_track: ThemeColor(Color::Rgb(0x00, 0xff, 0xcc)),
            dim: ThemeColor(Color::Rgb(0x66, 0x66, 0x66)),
            progress: ThemeColor(Color::Rgb(0xff, 0xff, 0xff)),
            error: ThemeColor(Color::Rgb(0xff, 0x44, 0x44)),
            viz_low: ThemeColor(Color::Rgb(0x88, 0x88, 0x88)),
            viz_high: ThemeColor(Color::Rgb(0xff, 0xff, 0xff)),
        }
    }

    pub fn base(&self) -> Style {
        Style::default().fg(self.foreground.0).bg(self.background.0)
    }

    pub fn border(&self, focused: bool) -> Style {
        let color = if focused {
            self.border_focused.0
        } else {
            self.border.0
        };
        Style::default().fg(color)
    }

    pub fn title(&self) -> Style {
        Style::default()
            .fg(self.accent.0)
            .add_modifier(Modifier::BOLD)
    }

    pub fn selected(&self) -> Style {
        Style::default()
            .fg(self.highlight_fg.0)
            .bg(self.highlight_bg.0)
            .add_modifier(Modifier::BOLD)
    }

    pub fn playing(&self) -> Style {
        Style::default()
            .fg(self.current_track.0)
            .add_modifier(Modifier::BOLD)
    }

    pub fn dim(&self) -> Style {
        Style::default().fg(self.dim.0)
    }

    pub fn error(&self) -> Style {
        Style::default().fg(self.error.0)
    }
}

fn rgb(hex: u32) -> ThemeColor {
    ThemeColor(Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8))
}

/// Wrapper so themes can be written as `"#7aa2f7"` or a named color in TOML.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeColor(pub Color);

impl Serialize for ThemeColor {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let text = match self.0 {
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
            other => format!("{other:?}").to_lowercase(),
        };
        s.serialize_str(&text)
    }
}

impl<'de> Deserialize<'de> for ThemeColor {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        parse_color(&text)
            .map(ThemeColor)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid color: {text}")))
    }
}

pub fn parse_color(text: &str) -> Option<Color> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix('#') {
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        return Some(Color::Rgb(r, g, b));
    }

    let lower = text.to_ascii_lowercase();

    let cleaned = lower
        .strip_prefix("indexed")
        .or_else(|| lower.strip_prefix("color"))
        .or_else(|| lower.strip_prefix("ansi"))
        .unwrap_or(&lower);

    let trimmed = cleaned
        .trim()
        .trim_matches(|c| c == '(' || c == ')' || c == ':' || c == ',');
    if let Ok(idx) = trimmed.trim().parse::<u8>() {
        return Some(Color::Indexed(idx));
    }

    if lower.starts_with("rgb(") && lower.ends_with(')') {
        let inner = &lower[4..lower.len() - 1];
        let parts: Vec<&str> = inner.split(',').collect();
        if parts.len() == 3 {
            let r = parts[0].trim().parse::<u8>().ok()?;
            let g = parts[1].trim().parse::<u8>().ok()?;
            let b = parts[2].trim().parse::<u8>().ok()?;
            return Some(Color::Rgb(r, g, b));
        }
    }

    let normalized = lower.replace(['-', '_', ' '], "");
    match normalized.as_str() {
        "reset" | "default" => Some(Color::Reset),
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "darkgray" | "darkgrey" => Some(Color::DarkGray),
        "lightred" => Some(Color::LightRed),
        "lightgreen" => Some(Color::LightGreen),
        "lightyellow" => Some(Color::LightYellow),
        "lightblue" => Some(Color::LightBlue),
        "lightmagenta" => Some(Color::LightMagenta),
        "lightcyan" => Some(Color::LightCyan),
        "white" => Some(Color::White),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_colors() {
        assert_eq!(parse_color("#7aa2f7"), Some(Color::Rgb(0x7a, 0xa2, 0xf7)));
    }

    #[test]
    fn parses_named_and_indexed_colors() {
        assert_eq!(parse_color("cyan"), Some(Color::Cyan));
        assert_eq!(parse_color("42"), Some(Color::Indexed(42)));
        assert_eq!(parse_color("color0"), Some(Color::Indexed(0)));
        assert_eq!(parse_color("color15"), Some(Color::Indexed(15)));
        assert_eq!(parse_color("indexed(6)"), Some(Color::Indexed(6)));
        assert_eq!(parse_color("indexed( 6 )"), Some(Color::Indexed(6)));
        assert_eq!(parse_color("indexed 6"), Some(Color::Indexed(6)));
        assert_eq!(parse_color("ansi(6)"), Some(Color::Indexed(6)));
        assert_eq!(parse_color("default"), Some(Color::Reset));
    }

    #[test]
    fn terminal_ansi_preset_creates_valid_theme() {
        let theme = Theme::preset("Terminal (ANSI 16)");
        assert_eq!(theme.background.0, Color::Reset);
        assert_eq!(theme.accent.0, Color::Indexed(6));
    }

    #[test]
    fn all_presets_round_trip_through_toml() {
        for preset_name in Theme::PRESET_NAMES {
            let original = Theme::preset(preset_name);
            let serialized = toml::to_string(&original).unwrap();
            let deserialized: Theme = toml::from_str(&serialized)
                .unwrap_or_else(|e| panic!("failed to deserialize preset '{preset_name}': {e}"));
            assert_eq!(original.accent, deserialized.accent);
            assert_eq!(original.background, deserialized.background);
        }
    }

    #[test]
    fn rejects_malformed_hex() {
        assert_eq!(parse_color("#abc"), None);
        assert_eq!(parse_color("#gggggg"), None);
    }
}
