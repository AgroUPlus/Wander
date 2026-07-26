//! Icons, in three flavours.
//!
//! The UI used to hardcode Nerd Font glyphs. They look best, but a terminal
//! without a patched font renders every one of them as a tofu box — and the
//! transport controls becoming unreadable rectangles is not a cosmetic problem.
//!
//! So icons come from a set chosen by the user. `Nerd` keeps the current look,
//! `Unicode` uses symbols present in most fonts, and `Ascii` assumes nothing at
//! all. Every glyph is one column wide in all three sets, so switching cannot
//! shift the layout.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GlyphSet {
    /// Requires a Nerd Font. The best-looking option, and the default.
    #[default]
    Nerd,
    /// Plain Unicode symbols, present in most modern fonts.
    Unicode,
    /// ASCII only, for a terminal or font that supports nothing else.
    Ascii,
}

/// What the UI can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Paused,
    Stopped,
    Buffering,
    RepeatOff,
    RepeatAll,
    RepeatOne,
    Shuffle,
    VolumeMuted,
    VolumeLow,
    VolumeHigh,
    Song,
    Album,
    Artist,
    Playlist,
    Command,
    Discover,
    Favorite,
    Surprise,
    Streak,
    CoverLoading,
    CoverMissing,
    Star,
}

impl GlyphSet {
    pub fn icon(self, icon: Icon) -> &'static str {
        use Icon::*;
        match (self, icon) {
            (GlyphSet::Nerd, Paused) => "󰏤",
            (GlyphSet::Nerd, Stopped) => "󰓛",
            (GlyphSet::Nerd, Buffering) => "󰦖",
            (GlyphSet::Nerd, RepeatOff) => "󰑗",
            (GlyphSet::Nerd, RepeatAll) => "󰑖",
            (GlyphSet::Nerd, RepeatOne) => "󰑘",
            (GlyphSet::Nerd, Shuffle) => "󰒝",
            (GlyphSet::Nerd, VolumeMuted) => "󰝟",
            (GlyphSet::Nerd, VolumeLow) => "󰖀",
            (GlyphSet::Nerd, VolumeHigh) => "󰕾",
            (GlyphSet::Nerd, Song) => "󰎈",
            (GlyphSet::Nerd, Album) => "󰀥",
            (GlyphSet::Nerd, Artist) => "󰠃",
            (GlyphSet::Nerd, Playlist) => "󰲹",
            (GlyphSet::Nerd, Command) => "󰘳",
            (GlyphSet::Nerd, Discover) => "󰋖",
            (GlyphSet::Nerd, Favorite) => "󰋑",
            (GlyphSet::Nerd, Surprise) => "󰐁",
            (GlyphSet::Nerd, Streak) => "󰥔",
            (GlyphSet::Nerd, CoverLoading) => "󰋞",
            (GlyphSet::Nerd, CoverMissing) => "󰎆",
            (GlyphSet::Nerd, Star) => "󰋑",

            (GlyphSet::Unicode, Paused) => "⏸",
            (GlyphSet::Unicode, Stopped) => "■",
            (GlyphSet::Unicode, Buffering) => "⋯",
            (GlyphSet::Unicode, RepeatOff) => "↻",
            (GlyphSet::Unicode, RepeatAll) => "↻",
            (GlyphSet::Unicode, RepeatOne) => "↺",
            (GlyphSet::Unicode, Shuffle) => "⤨",
            (GlyphSet::Unicode, VolumeMuted) => "×",
            (GlyphSet::Unicode, VolumeLow) => "◔",
            (GlyphSet::Unicode, VolumeHigh) => "◉",
            (GlyphSet::Unicode, Song) => "♪",
            (GlyphSet::Unicode, Album) => "◎",
            (GlyphSet::Unicode, Artist) => "☺",
            (GlyphSet::Unicode, Playlist) => "≡",
            (GlyphSet::Unicode, Command) => "⌘",
            (GlyphSet::Unicode, Discover) => "✦",
            (GlyphSet::Unicode, Favorite) => "★",
            (GlyphSet::Unicode, Surprise) => "⁇",
            (GlyphSet::Unicode, Streak) => "◆",
            (GlyphSet::Unicode, CoverLoading) => "◌",
            (GlyphSet::Unicode, CoverMissing) => "♪",
            (GlyphSet::Unicode, Star) => "★",

            (GlyphSet::Ascii, Paused) => "=",
            (GlyphSet::Ascii, Stopped) => "#",
            (GlyphSet::Ascii, Buffering) => "~",
            (GlyphSet::Ascii, RepeatOff) => "r",
            (GlyphSet::Ascii, RepeatAll) => "R",
            (GlyphSet::Ascii, RepeatOne) => "1",
            (GlyphSet::Ascii, Shuffle) => "z",
            (GlyphSet::Ascii, VolumeMuted) => "x",
            (GlyphSet::Ascii, VolumeLow) => "-",
            (GlyphSet::Ascii, VolumeHigh) => "+",
            (GlyphSet::Ascii, Song) => "-",
            (GlyphSet::Ascii, Album) => "o",
            (GlyphSet::Ascii, Artist) => "@",
            (GlyphSet::Ascii, Playlist) => "=",
            (GlyphSet::Ascii, Command) => ":",
            (GlyphSet::Ascii, Discover) => "?",
            (GlyphSet::Ascii, Favorite) => "*",
            (GlyphSet::Ascii, Surprise) => "!",
            (GlyphSet::Ascii, Streak) => "^",
            (GlyphSet::Ascii, CoverLoading) => ".",
            (GlyphSet::Ascii, CoverMissing) => "-",
            (GlyphSet::Ascii, Star) => "*",
        }
    }
}

/// Every icon the UI can ask for, so the tests can sweep them.
#[cfg(test)]
pub const ALL_ICONS: [Icon; 22] = [
    Icon::Paused,
    Icon::Stopped,
    Icon::Buffering,
    Icon::RepeatOff,
    Icon::RepeatAll,
    Icon::RepeatOne,
    Icon::Shuffle,
    Icon::VolumeMuted,
    Icon::VolumeLow,
    Icon::VolumeHigh,
    Icon::Song,
    Icon::Album,
    Icon::Artist,
    Icon::Playlist,
    Icon::Command,
    Icon::Discover,
    Icon::Favorite,
    Icon::Surprise,
    Icon::Streak,
    Icon::CoverLoading,
    Icon::CoverMissing,
    Icon::Star,
];

#[cfg(test)]
pub const ALL_SETS: [GlyphSet; 3] = [GlyphSet::Nerd, GlyphSet::Unicode, GlyphSet::Ascii];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_defined_in_every_set() {
        for set in ALL_SETS {
            for icon in ALL_ICONS {
                assert!(!set.icon(icon).is_empty(), "{set:?}/{icon:?} is empty");
            }
        }
    }

    /// A two-column glyph in one set and a one-column glyph in another would
    /// shift every column of the player bar when the setting changed.
    #[test]
    fn every_glyph_is_a_single_character() {
        for set in ALL_SETS {
            for icon in ALL_ICONS {
                let glyph = set.icon(icon);
                assert_eq!(
                    glyph.chars().count(),
                    1,
                    "{set:?}/{icon:?} is {glyph:?}, not one character"
                );
            }
        }
    }

    #[test]
    fn the_ascii_set_is_actually_ascii() {
        for icon in ALL_ICONS {
            let glyph = GlyphSet::Ascii.icon(icon);
            assert!(
                glyph.is_ascii(),
                "{icon:?} is {glyph:?}, which needs more than ASCII"
            );
        }
    }

    #[test]
    fn glyph_sets_round_trip_through_config() {
        for set in ALL_SETS {
            let encoded = serde_json::to_string(&set).unwrap();
            let decoded: GlyphSet = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, set);
        }
        // Written in lowercase, as a config file would spell it.
        assert_eq!(
            serde_json::from_str::<GlyphSet>("\"ascii\"").unwrap(),
            GlyphSet::Ascii
        );
    }
}
