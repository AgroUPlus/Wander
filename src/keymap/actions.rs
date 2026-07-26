use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
    pub(crate) fn description(self) -> Option<&'static str> {
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
