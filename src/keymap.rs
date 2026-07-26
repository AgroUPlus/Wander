use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

/// Everything the user can ask the app to do. UI code never acts on raw keys;
/// it resolves them through the keymap first so every binding is rebindable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    NextTab,
    PrevTab,
    TabBack,
    Tab(usize),

    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Left,
    Right,
    /// Move focus between the panes on screen, whatever Left/Right do on this
    /// tab. Settings and Home spend the plain arrows on their own contents, so
    /// without these the side panes are unreachable from the keyboard there.
    FocusNext,
    FocusPrev,
    Confirm,
    Cancel,

    TogglePause,
    Stop,
    NextTrack,
    PrevTrack,
    SeekForward,
    SeekBackward,
    VolumeUp,
    VolumeDown,

    AddToQueue,
    RemoveFromQueue,
    ClearQueue,
    ToggleRepeat,
    ToggleShuffle,

    Refresh,
    ToggleHelp,
    ToggleQueuePane,
    ToggleCoverPane,
    ToggleLyricsPane,
    ToggleVisualiser,
    /// Step through the visualiser's drawing styles.
    CycleVisualiser,
    /// Step through the lyric variants a track offers (translations,
    /// romanisations, other languages).
    CycleLyricVariant,
    /// Ask the configured translation endpoint for the current lyrics.
    TranslateLyrics,
    /// Fetch missing or synced lyrics online from LRCLIB.
    FetchOnlineLyrics,
    ToggleRadio,
    ToggleStar,
    ResizePaneLeft,
    ResizePaneRight,
    ResizePaneUp,
    ResizePaneDown,
    JumpToArtist,
    JumpToAlbum,

    /// Switch between the Library tab's views (artists / albums / …).
    LibraryModeNext,
    LibraryModePrev,
    /// Queue the selection right after the current track.
    PlayNext,
    MoveTrackUp,
    MoveTrackDown,
    AddToPlaylist,
    Share,
    RatingUp,
    RatingDown,
    /// Fuzzy "go to anything" palette.
    OpenPalette,
    /// Undo the last destructive queue change.
    UndoQueue,
    /// Full-screen now-playing view.
    ToggleFocusMode,
}

/// Section of the cheat sheet an action belongs to.
///
/// Ordered as they appear in the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Playback,
    Navigation,
    Library,
    Queue,
    Panels,
    Misc,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::Playback,
        Category::Navigation,
        Category::Library,
        Category::Queue,
        Category::Panels,
        Category::Misc,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Category::Playback => "Playback",
            Category::Navigation => "Navigation",
            Category::Library => "Library",
            Category::Queue => "Queue",
            Category::Panels => "Panels",
            Category::Misc => "Misc",
        }
    }
}

impl Action {
    pub fn category(self) -> Category {
        use Action::*;
        match self {
            TogglePause | Stop | NextTrack | PrevTrack | SeekForward | SeekBackward | VolumeUp
            | VolumeDown | ToggleRepeat | ToggleShuffle | ToggleRadio => Category::Playback,
            NextTab | PrevTab | TabBack | Tab(_) | Up | Down | PageUp | PageDown | Top | Bottom
            | Left | Right | FocusNext | FocusPrev | Confirm | Cancel => Category::Navigation,
            LibraryModeNext | LibraryModePrev | JumpToArtist | JumpToAlbum | ToggleStar
            | RatingUp | RatingDown | AddToPlaylist | Share => Category::Library,
            CycleLyricVariant | TranslateLyrics | FetchOnlineLyrics => Category::Panels,
            AddToQueue | PlayNext | RemoveFromQueue | ClearQueue | MoveTrackUp | MoveTrackDown
            | UndoQueue => Category::Queue,
            ToggleQueuePane | ToggleCoverPane | ToggleLyricsPane | ToggleVisualiser
            | CycleVisualiser | ToggleFocusMode | ResizePaneLeft | ResizePaneRight
            | ResizePaneUp | ResizePaneDown => Category::Panels,
            OpenPalette => Category::Navigation,
            Quit | Refresh | ToggleHelp => Category::Misc,
        }
    }

    /// Human description, used to generate the help overlay.
    fn description(self) -> Option<&'static str> {
        Some(match self {
            Action::Quit => "quit",
            Action::NextTab => "next tab",
            Action::PrevTab => "previous tab",
            Action::TabBack => "back to last tab",
            Action::Up => "move up",
            Action::Down => "move down",
            Action::PageUp => "page up",
            Action::PageDown => "page down",
            Action::Top => "jump to top",
            Action::Bottom => "jump to bottom",
            Action::Left => "focus left pane",
            Action::Right => "focus right pane",
            Action::Confirm => "play selection",
            Action::TogglePause => "play / pause",
            Action::Stop => "stop",
            Action::NextTrack => "next track",
            Action::PrevTrack => "previous track",
            Action::SeekForward => "seek forward",
            Action::SeekBackward => "seek backward",
            Action::VolumeUp => "volume up",
            Action::VolumeDown => "volume down",
            Action::AddToQueue => "add to queue",
            Action::RemoveFromQueue => "remove from queue",
            Action::ClearQueue => "clear queue",
            Action::ToggleRepeat => "cycle repeat",
            Action::ToggleShuffle => "toggle shuffle",
            Action::Refresh => "refresh library",
            Action::ToggleHelp => "this help",
            Action::ToggleQueuePane => "toggle queue pane",
            Action::ToggleCoverPane => "toggle cover pane",
            Action::ToggleLyricsPane => "toggle lyrics pane",
            Action::ToggleVisualiser => "toggle visualiser",
            Action::CycleVisualiser => "cycle visualiser style",
            Action::CycleLyricVariant => "cycle lyric language / romanisation",
            Action::TranslateLyrics => "translate the current lyrics",
            Action::FetchOnlineLyrics => "fetch lyrics online (LRCLIB)",
            Action::ToggleRadio => "radio mode (auto-queue)",
            Action::ToggleStar => "star / unstar track",
            Action::FocusNext => "focus the next pane (e.g. Up Next)",
            Action::FocusPrev => "focus the previous pane",
            Action::ResizePaneLeft => "move divider left (wider side panes)",
            Action::ResizePaneRight => "move divider right (wider content)",
            Action::ResizePaneUp => "increase visualiser height / move divider up",
            Action::ResizePaneDown => "decrease visualiser height / move divider down",
            Action::JumpToArtist => "jump to track artist",
            Action::JumpToAlbum => "jump to track album",
            Action::LibraryModeNext => "next library view",
            Action::LibraryModePrev => "previous library view",
            Action::PlayNext => "play selection next",
            Action::MoveTrackUp => "move queued track up",
            Action::MoveTrackDown => "move queued track down",
            Action::AddToPlaylist => "add to playlist",
            Action::Share => "share a link to the track",
            Action::RatingUp => "raise rating",
            Action::RatingDown => "lower rating",
            Action::OpenPalette => "go to anything (fuzzy)",
            Action::UndoQueue => "undo queue change",
            Action::ToggleFocusMode => "focus mode (full screen)",
            // Not worth a help line.
            Action::Tab(_) | Action::Cancel => return None,
        })
    }
}

pub struct Keymap {
    bindings: HashMap<(KeyCode, KeyModifiers), Action>,
}

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
        for i in 1..=4u8 {
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
fn normalise(code: KeyCode, mods: KeyModifiers) -> (KeyCode, KeyModifiers) {
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
pub fn parse_key(spec: &str) -> Option<(KeyCode, KeyModifiers)> {
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

/// `TogglePause` -> `toggle_pause`, so config files read naturally.
fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (index, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if index != 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

impl Action {
    /// Every rebindable action. Add new variants here so they can be named in
    /// the config; the tests check that the names stay unique and parseable.
    pub const ALL: &'static [Action] = {
        use Action::*;
        &[
            Quit,
            NextTab,
            PrevTab,
            TabBack,
            Up,
            Down,
            PageUp,
            PageDown,
            Top,
            Bottom,
            Left,
            Right,
            FocusNext,
            FocusPrev,
            Confirm,
            Cancel,
            TogglePause,
            Stop,
            NextTrack,
            PrevTrack,
            SeekForward,
            SeekBackward,
            VolumeUp,
            VolumeDown,
            AddToQueue,
            RemoveFromQueue,
            ClearQueue,
            ToggleRepeat,
            ToggleShuffle,
            Refresh,
            ToggleHelp,
            ToggleQueuePane,
            ToggleCoverPane,
            ToggleLyricsPane,
            ToggleVisualiser,
            CycleVisualiser,
            CycleLyricVariant,
            TranslateLyrics,
            FetchOnlineLyrics,
            ToggleRadio,
            ToggleStar,
            ResizePaneLeft,
            ResizePaneRight,
            ResizePaneUp,
            ResizePaneDown,
            JumpToArtist,
            JumpToAlbum,
            LibraryModeNext,
            LibraryModePrev,
            PlayNext,
            MoveTrackUp,
            MoveTrackDown,
            AddToPlaylist,
            Share,
            RatingUp,
            RatingDown,
            OpenPalette,
            UndoQueue,
            ToggleFocusMode,
        ]
    };

    /// The name used in `config.toml`.
    pub fn name(self) -> String {
        match self {
            // The only variant carrying data; `tab_1` … `tab_9`.
            Action::Tab(index) => format!("tab_{}", index + 1),
            other => snake_case(&format!("{other:?}")),
        }
    }

    pub fn from_name(name: &str) -> Option<Action> {
        let name = name.trim().to_ascii_lowercase();
        if let Some(index) = name.strip_prefix("tab_")
            && let Ok(n) = index.parse::<usize>()
            && (1..=9).contains(&n)
        {
            return Some(Action::Tab(n - 1));
        }
        Action::ALL
            .iter()
            .copied()
            .find(|action| action.name() == name)
    }
}

impl Keymap {
    /// Apply user bindings from `config.toml`'s `[keys]` table.
    ///
    /// Each entry replaces whatever the key was bound to; the value `"none"`
    /// unbinds it. Unusable entries are reported rather than ignored, so a typo
    /// is visible instead of silently doing nothing.
    pub fn apply_overrides(&mut self, overrides: &HashMap<String, String>) -> Vec<String> {
        let mut problems = Vec::new();
        for (spec, action_name) in overrides {
            let Some((code, mods)) = parse_key(spec) else {
                problems.push(format!("unknown key '{spec}'"));
                continue;
            };
            if action_name.trim().eq_ignore_ascii_case("none") {
                self.bindings.remove(&normalise(code, mods));
                continue;
            }
            let Some(action) = Action::from_name(action_name) else {
                problems.push(format!("unknown action '{action_name}' for '{spec}'"));
                continue;
            };
            self.bindings.insert(normalise(code, mods), action);
        }
        problems.sort();
        problems
    }

    pub fn resolve(&self, key: KeyEvent) -> Option<Action> {
        self.bindings
            .get(&normalise(key.code, key.modifiers))
            .copied()
    }

    /// Every bound action, grouped into cheat-sheet sections.
    ///
    /// Covers the whole keymap rather than a curated shortlist, which is
    /// the point of a cheat sheet.
    pub fn describe_grouped(&self) -> Vec<(Category, Vec<(String, String)>)> {
        // De-duplicate: one action usually has several bindings.
        let mut actions: Vec<Action> = self.bindings.values().copied().collect();
        actions.sort_by_key(|a| format!("{a:?}"));
        actions.dedup();

        Category::ALL
            .iter()
            .filter_map(|category| {
                let mut entries: Vec<(String, String)> = actions
                    .iter()
                    .filter(|action| action.category() == *category)
                    .filter_map(|action| {
                        Some((self.keys_for(*action)?, action.description()?.to_string()))
                    })
                    .collect();
                if entries.is_empty() {
                    return None;
                }
                entries.sort_by(|a, b| a.1.cmp(&b.1));
                Some((*category, entries))
            })
            .collect()
    }

    /// Look an action back up from the description shown to the user.
    ///
    /// Descriptions are unique per action (asserted in the tests), so this is
    /// the cheapest way for the palette to offer commands without duplicating
    /// the action list.
    pub fn action_for_description(&self, description: &str) -> Option<Action> {
        self.bindings
            .values()
            .find(|action| action.description() == Some(description))
            .copied()
    }

    /// The (at most two) keys bound to an action, shortest first.
    fn keys_for(&self, action: Action) -> Option<String> {
        let mut keys: Vec<String> = self
            .bindings
            .iter()
            .filter(|(_, bound)| **bound == action)
            .map(|((code, mods), _)| render_key(*code, *mods))
            .collect();
        if keys.is_empty() {
            return None;
        }
        keys.sort_by_key(|k| (k.chars().count(), k.clone()));
        keys.truncate(2);
        Some(keys.join(" / "))
    }
}

fn render_key(code: KeyCode, mods: KeyModifiers) -> String {
    let name = match code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Esc => "esc".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::BackTab => "S-tab".to_string(),
        KeyCode::Backspace => "bksp".to_string(),
        KeyCode::Delete => "del".to_string(),
        KeyCode::Up => "↑".to_string(),
        KeyCode::Down => "↓".to_string(),
        KeyCode::Left => "←".to_string(),
        KeyCode::Right => "→".to_string(),
        KeyCode::PageUp => "pgup".to_string(),
        KeyCode::PageDown => "pgdn".to_string(),
        other => format!("{other:?}").to_lowercase(),
    };
    let mut out = String::new();
    if mods.contains(KeyModifiers::CONTROL) {
        out.push_str("C-");
    }
    if mods.contains(KeyModifiers::ALT) {
        out.push_str("M-");
    }
    out.push_str(&name);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn resolves_vim_movement() {
        let map = Keymap::default();
        assert_eq!(map.resolve(key('j')), Some(Action::Down));
        assert_eq!(map.resolve(key('k')), Some(Action::Up));
    }

    #[test]
    fn resolves_shifted_bindings_with_shift_flag_set() {
        let map = Keymap::default();
        let shifted = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert_eq!(map.resolve(shifted), Some(Action::Bottom));
    }

    /// Terminals disagree about whether a shifted letter also carries the SHIFT
    /// flag. Both spellings must reach the same action, or half the bindings
    /// are unusable depending on the terminal.
    #[test]
    fn resolves_shifted_bindings_without_the_shift_flag() {
        let map = Keymap::default();
        for (c, expected) in [
            ('G', Action::Bottom),
            ('F', Action::ToggleFocusMode),
            ('C', Action::ClearQueue),
            ('S', Action::Share),
            ('P', Action::PlayNext),
            ('Q', Action::ToggleQueuePane),
        ] {
            assert_eq!(
                map.resolve(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)),
                Some(expected),
                "{c} without the SHIFT flag"
            );
            assert_eq!(
                map.resolve(KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT)),
                Some(expected),
                "{c} with the SHIFT flag"
            );
        }
    }

    #[test]
    fn shift_still_distinguishes_non_character_keys() {
        let map = Keymap::default();
        assert_eq!(
            map.resolve(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
            Some(Action::PrevTab)
        );
    }

    #[test]
    fn control_and_alt_bindings_are_not_confused_with_plain_letters() {
        let map = Keymap::default();
        assert_eq!(
            map.resolve(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)),
            Some(Action::OpenPalette)
        );
        assert_eq!(
            map.resolve(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)),
            Some(Action::PrevTrack)
        );
    }

    #[test]
    fn unbound_keys_resolve_to_nothing() {
        let map = Keymap::default();
        assert_eq!(map.resolve(key('%')), None);
    }

    #[test]
    fn bracket_and_alt_arrows_work() {
        let map = Keymap::default();
        assert_eq!(map.resolve(key(']')), Some(Action::NextTab));
        assert_eq!(map.resolve(key('[')), Some(Action::PrevTab));
        let alt_right = KeyEvent::new(KeyCode::Right, KeyModifiers::ALT);
        assert_eq!(map.resolve(alt_right), Some(Action::ResizePaneRight));
        let alt_up = KeyEvent::new(KeyCode::Up, KeyModifiers::ALT);
        assert_eq!(map.resolve(alt_up), Some(Action::ResizePaneUp));
        let alt_down = KeyEvent::new(KeyCode::Down, KeyModifiers::ALT);
        assert_eq!(map.resolve(alt_down), Some(Action::ResizePaneDown));
    }

    /// Focus cycling has to be on its own keys: the plain arrows are already
    /// taken by Home's mix row and by Settings' value editing, so the side
    /// panes would otherwise be reachable only with the mouse.
    #[test]
    fn control_arrows_cycle_focus_without_disturbing_the_plain_ones() {
        let map = Keymap::default();
        let ctrl = |code| map.resolve(KeyEvent::new(code, KeyModifiers::CONTROL));

        assert_eq!(ctrl(KeyCode::Right), Some(Action::FocusNext));
        assert_eq!(ctrl(KeyCode::Left), Some(Action::FocusPrev));
        assert_eq!(ctrl(KeyCode::Tab), Some(Action::FocusNext));

        // The unmodified keys keep their meanings.
        let plain = |code| map.resolve(KeyEvent::new(code, KeyModifiers::NONE));
        assert_eq!(plain(KeyCode::Right), Some(Action::Right));
        assert_eq!(plain(KeyCode::Left), Some(Action::Left));
        assert_eq!(plain(KeyCode::Tab), Some(Action::NextTab));
    }

    /// `[keys]` rebinding looks actions up by name, so a new action that is not
    /// in `ALL` silently cannot be rebound.
    #[test]
    fn the_focus_actions_can_be_rebound_by_name() {
        assert_eq!(Action::from_name("focus_next"), Some(Action::FocusNext));
        assert_eq!(Action::from_name("focus_prev"), Some(Action::FocusPrev));
    }

    #[test]
    fn the_cheat_sheet_covers_every_described_binding() {
        let map = Keymap::default();
        let grouped = map.describe_grouped();

        // Every bound action that has a description must appear exactly once.
        let mut listed: Vec<&String> = grouped
            .iter()
            .flat_map(|(_, e)| e.iter().map(|(_, d)| d))
            .collect();
        let total = listed.len();
        listed.sort();
        listed.dedup();
        assert_eq!(listed.len(), total, "an action is listed in two sections");

        let describable: std::collections::HashSet<&'static str> = map
            .bindings
            .values()
            .filter_map(|action| action.description())
            .collect();
        assert_eq!(
            total,
            describable.len(),
            "the cheat sheet is missing bindings that have a description"
        );
    }

    #[test]
    fn the_cheat_sheet_is_grouped_and_not_empty() {
        let grouped = Keymap::default().describe_grouped();
        assert!(grouped.len() >= 4, "expected several sections");
        assert!(grouped.iter().all(|(_, entries)| !entries.is_empty()));
        // Sections keep their declared order.
        let order: Vec<Category> = grouped.iter().map(|(c, _)| *c).collect();
        let mut sorted = order.clone();
        sorted.sort_by_key(|c| Category::ALL.iter().position(|x| x == c));
        assert_eq!(order, sorted);
    }

    #[test]
    fn key_specs_parse_in_the_spellings_people_actually_write() {
        assert_eq!(
            parse_key("p"),
            Some((KeyCode::Char('p'), KeyModifiers::NONE))
        );
        assert_eq!(
            parse_key("ctrl+p"),
            Some((KeyCode::Char('p'), KeyModifiers::CONTROL))
        );
        assert_eq!(
            parse_key("C-p"),
            Some((KeyCode::Char('p'), KeyModifiers::CONTROL))
        );
        assert_eq!(
            parse_key("alt+left"),
            Some((KeyCode::Left, KeyModifiers::ALT))
        );
        assert_eq!(
            parse_key("space"),
            Some((KeyCode::Char(' '), KeyModifiers::NONE))
        );
        assert_eq!(parse_key("F5"), Some((KeyCode::F(5), KeyModifiers::NONE)));
        // Case is preserved for characters, since `F` and `f` differ.
        assert_eq!(
            parse_key("F"),
            Some((KeyCode::Char('F'), KeyModifiers::NONE))
        );
    }

    #[test]
    fn separator_characters_can_themselves_be_bound() {
        assert_eq!(
            parse_key("+"),
            Some((KeyCode::Char('+'), KeyModifiers::NONE))
        );
        assert_eq!(
            parse_key("-"),
            Some((KeyCode::Char('-'), KeyModifiers::NONE))
        );
    }

    #[test]
    fn malformed_key_specs_are_rejected_rather_than_guessed() {
        assert_eq!(parse_key(""), None);
        assert_eq!(parse_key("hyper+p"), None);
        assert_eq!(parse_key("notakey"), None);
        assert_eq!(parse_key("F99"), None);
    }

    #[test]
    fn action_names_round_trip() {
        for action in Action::ALL {
            let name = action.name();
            assert_eq!(
                Action::from_name(&name),
                Some(*action),
                "'{name}' did not round-trip"
            );
        }
        assert_eq!(Action::from_name("tab_3"), Some(Action::Tab(2)));
        assert_eq!(Action::Tab(2).name(), "tab_3");
        assert_eq!(Action::from_name("nonsense"), None);
    }

    #[test]
    fn action_names_are_unique() {
        let mut names: Vec<String> = Action::ALL.iter().map(|a| a.name()).collect();
        let total = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), total, "two actions share a config name");
    }

    #[test]
    fn every_bound_action_is_rebindable() {
        // An action reachable by default but missing from ALL could not be
        // named in the config, which would be a confusing gap.
        let map = Keymap::default();
        for action in map.bindings.values() {
            if matches!(action, Action::Tab(_)) {
                continue;
            }
            assert!(
                Action::ALL.contains(action),
                "{action:?} is bound by default but not listed in Action::ALL"
            );
        }
    }

    #[test]
    fn overrides_replace_the_default_binding() {
        let mut map = Keymap::default();
        assert_eq!(map.resolve(key('z')), Some(Action::ToggleShuffle));

        let overrides = HashMap::from([("z".to_string(), "quit".to_string())]);
        assert!(map.apply_overrides(&overrides).is_empty());
        assert_eq!(map.resolve(key('z')), Some(Action::Quit));
    }

    #[test]
    fn overrides_can_unbind_a_key() {
        let mut map = Keymap::default();
        let overrides = HashMap::from([("z".to_string(), "none".to_string())]);
        assert!(map.apply_overrides(&overrides).is_empty());
        assert_eq!(map.resolve(key('z')), None);
    }

    #[test]
    fn a_shifted_override_resolves_without_the_shift_flag_too() {
        let mut map = Keymap::default();
        let overrides = HashMap::from([("shift+w".to_string(), "quit".to_string())]);
        assert!(map.apply_overrides(&overrides).is_empty());
        assert_eq!(
            map.resolve(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE)),
            Some(Action::Quit)
        );
    }

    #[test]
    fn bad_overrides_are_reported_and_leave_the_rest_working() {
        let mut map = Keymap::default();
        let overrides = HashMap::from([
            ("nonsense-key".to_string(), "quit".to_string()),
            ("y".to_string(), "no_such_action".to_string()),
            ("w".to_string(), "quit".to_string()),
        ]);
        let problems = map.apply_overrides(&overrides);

        assert_eq!(problems.len(), 2, "got {problems:?}");
        assert!(problems.iter().any(|p| p.contains("unknown key")));
        assert!(problems.iter().any(|p| p.contains("unknown action")));
        // The valid entry still applied.
        assert_eq!(map.resolve(key('w')), Some(Action::Quit));
    }

    #[test]
    fn help_renders_modifiers_readably() {
        assert_eq!(render_key(KeyCode::Char('d'), KeyModifiers::CONTROL), "C-d");
        assert_eq!(render_key(KeyCode::Char(' '), KeyModifiers::NONE), "space");
        assert_eq!(render_key(KeyCode::Right, KeyModifiers::ALT), "M-→");
    }
}
