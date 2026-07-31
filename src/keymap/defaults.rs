use super::actions::*;
use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl Default for Keymap {
    fn default() -> Self {
        use Action::*;
        let n = KeyModifiers::NONE;
        let c = KeyModifiers::CONTROL;
        let a = KeyModifiers::ALT;
        let s = KeyModifiers::SHIFT;
        let mut bindings = HashMap::new();
        // Bindings are stored normalised, so lookup can be a single hash hit.
        let mut bind = |code, mods, action| {
            bindings.insert(normalise(code, mods), action);
        };

        bind(KeyCode::Char('q'), n, Quit);
        bind(KeyCode::Tab, n, NextTab);
        bind(KeyCode::BackTab, s, PrevTab);
        bind(KeyCode::Char(']'), n, NextTab);
        bind(KeyCode::Char('['), n, PrevTab);
        bind(KeyCode::Left, a, ResizePaneLeft);
        bind(KeyCode::Right, a, ResizePaneRight);
        bind(KeyCode::Up, a, ResizePaneUp);
        bind(KeyCode::Down, a, ResizePaneDown);
        bind(KeyCode::Char('A'), s, JumpToArtist);
        bind(KeyCode::Char('a'), a, JumpToArtist);
        bind(KeyCode::Char('B'), s, JumpToAlbum);
        bind(KeyCode::Char('b'), a, JumpToAlbum);
        bind(KeyCode::Char('C'), s, ClearQueue);
        bind(KeyCode::Backspace, n, TabBack);
        // One binding per tab that can exist. Enabling an online plugin adds a
        // tab, which used to push Settings to a fifth slot that no key reached.
        for i in 1..=crate::app::Tab::ALL.len() as u8 {
            bind(KeyCode::Char((b'0' + i) as char), n, Tab(i as usize - 1));
        }

        bind(KeyCode::Char('k'), n, Up);
        bind(KeyCode::Up, n, Up);
        bind(KeyCode::Char('j'), n, Down);
        bind(KeyCode::Down, n, Down);
        bind(KeyCode::Char('u'), c, PageUp);
        bind(KeyCode::PageUp, n, PageUp);
        bind(KeyCode::Char('d'), c, PageDown);
        bind(KeyCode::PageDown, n, PageDown);
        bind(KeyCode::Char('g'), n, Top);
        bind(KeyCode::Char('G'), s, Bottom);
        bind(KeyCode::Char('h'), n, Left);
        bind(KeyCode::Left, n, Left);
        bind(KeyCode::Char('l'), n, Right);
        bind(KeyCode::Right, n, Right);
        // Focus cycling that works on every tab, including the ones whose
        // plain arrows are already spoken for.
        bind(KeyCode::Right, c, FocusNext);
        bind(KeyCode::Left, c, FocusPrev);
        bind(KeyCode::Tab, c, FocusNext);
        bind(KeyCode::Enter, n, Confirm);
        bind(KeyCode::Esc, n, Cancel);

        bind(KeyCode::Char(' '), n, TogglePause);
        bind(KeyCode::Char('s'), n, Stop);
        bind(KeyCode::Char('n'), n, NextTrack);
        bind(KeyCode::Char('>'), s, NextTrack);
        bind(KeyCode::Char('p'), n, PrevTrack);
        bind(KeyCode::Char('<'), s, PrevTrack);
        bind(KeyCode::Char('f'), n, SeekForward);
        bind(KeyCode::Char('b'), n, SeekBackward);
        bind(KeyCode::Char('+'), n, VolumeUp);
        bind(KeyCode::Char('='), n, VolumeUp);
        bind(KeyCode::Char('-'), n, VolumeDown);

        bind(KeyCode::Char('a'), n, AddToQueue);
        bind(KeyCode::Char('D'), s, RemoveFromQueue);
        bind(KeyCode::Delete, n, RemoveFromQueue);
        bind(KeyCode::Char('C'), s, ClearQueue);
        bind(KeyCode::Char('r'), n, ToggleRepeat);
        bind(KeyCode::Char('z'), n, ToggleShuffle);

        bind(KeyCode::Char('/'), n, OpenPalette);
        bind(KeyCode::Char('R'), s, Refresh);
        bind(KeyCode::Char('?'), s, ToggleHelp);
        bind(KeyCode::Char('h'), c, ToggleHelp);
        bind(KeyCode::Char('Q'), s, ToggleQueuePane);
        bind(KeyCode::Char('c'), n, ToggleCoverPane);
        bind(KeyCode::Char('L'), s, ToggleLyricsPane);
        bind(KeyCode::Char('y'), n, ToggleLyricsPane);
        bind(KeyCode::Char('v'), n, ToggleVisualiser);
        bind(KeyCode::Char('V'), s, CycleVisualiser);
        bind(KeyCode::Char('Y'), s, CycleLyricVariant);
        bind(KeyCode::Char('T'), s, TranslateLyrics);
        bind(KeyCode::Char('l'), c, FetchOnlineLyrics);
        bind(KeyCode::Char('x'), n, ToggleRadio);
        bind(KeyCode::Char('*'), n, ToggleStar);

        bind(KeyCode::Char('m'), n, LibraryModeNext);
        bind(KeyCode::Char('M'), s, LibraryModePrev);
        bind(KeyCode::Char('P'), s, PlayNext);
        bind(KeyCode::Char('e'), n, AddToPlaylist);
        bind(KeyCode::Char('S'), s, Share);
        bind(KeyCode::Char('.'), n, RatingUp);
        bind(KeyCode::Char(','), n, RatingDown);
        bind(KeyCode::Char('j'), a, MoveTrackDown);
        bind(KeyCode::Char('k'), a, MoveTrackUp);
        bind(KeyCode::Char('p'), c, OpenPalette);
        bind(KeyCode::Char('z'), c, UndoQueue);
        bind(KeyCode::Char('F'), s, ToggleFocusMode);

        Self { bindings }
    }
}

/// Canonical form of a key for lookup.
///
/// For character keys the shifted character *is* the shift — `F` already says
/// what `shift+f` means — but terminals disagree about whether they also set
/// the SHIFT flag, and crossterm passes that inconsistency straight through.
/// Normalising both the bindings and the incoming events means a shifted
/// binding resolves either way; without it, every `Shift`-bound action was
/// silently unreachable on terminals that do not report the flag.
pub(crate) fn normalise(code: KeyCode, mods: KeyModifiers) -> (KeyCode, KeyModifiers) {
    match code {
        KeyCode::Char(_) => (code, mods - KeyModifiers::SHIFT),
        // BackTab and friends genuinely need the flag to be distinguishable.
        _ => (code, mods),
    }
}

/// Parse a key spec like `ctrl+p`, `M-left`, `F`, `space`.
///
/// Modifiers are `ctrl`/`c`, `alt`/`m`/`meta`, `shift`/`s`, joined by `+` or
/// `-`. The last segment is the key itself.
pub(crate) fn parse_key(spec: &str) -> Option<(KeyCode, KeyModifiers)> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }

    // Split on separators, but never on a trailing literal `+` or `-` key, so
    // the volume bindings can still be written as "+" and "-".
    let mut parts: Vec<&str> = Vec::new();
    let mut rest = spec;
    while let Some(pos) = rest.find(['+', '-']) {
        // A separator at the very end means the key *is* that character.
        if pos + 1 >= rest.len() {
            break;
        }
        parts.push(&rest[..pos]);
        rest = &rest[pos + 1..];
    }
    parts.push(rest);

    let mut mods = KeyModifiers::NONE;
    for part in &parts[..parts.len() - 1] {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "c" => mods |= KeyModifiers::CONTROL,
            "alt" | "meta" | "m" => mods |= KeyModifiers::ALT,
            "shift" | "s" => mods |= KeyModifiers::SHIFT,
            _ => return None,
        }
    }

    let key = parts[parts.len() - 1];
    let code = match key.to_ascii_lowercase().as_str() {
        "space" => KeyCode::Char(' '),
        "enter" | "return" | "cr" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "backtab" | "s-tab" => KeyCode::BackTab,
        "backspace" | "bksp" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" => KeyCode::PageDown,
        other => {
            if let Some(number) = other.strip_prefix('f')
                && let Ok(n) = number.parse::<u8>()
                && (1..=12).contains(&n)
            {
                KeyCode::F(n)
            } else {
                // Anything else must be exactly one character. Take it from the
                // original spelling so case is preserved.
                let mut chars = key.chars();
                let c = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                KeyCode::Char(c)
            }
        }
    };
    Some((code, mods))
}
