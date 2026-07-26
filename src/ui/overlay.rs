//! Modal popups: sharing a track, and adding one to a playlist.
//!
//! Overlays own the keyboard while they are open — `App::handle_key` routes to
//! [`Overlay::handle_key`] before the keymap gets a look, so a popup can accept
//! free text without every letter also triggering a global binding.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use super::glyphs::{GlyphSet, Icon};
use super::widgets::truncate;
use crate::subsonic::models::{Playlist, Song};
use crate::theme::Theme;

/// Share expiry choices, as `(label, duration)`. `None` means the server's own
/// default (a year on Navidrome unless configured otherwise).
pub const EXPIRIES: [(&str, Option<i64>); 5] = [
    ("1 hour", Some(3_600_000)),
    ("24 hours", Some(86_400_000)),
    ("7 days", Some(604_800_000)),
    ("30 days", Some(2_592_000_000)),
    ("Server default", None),
];

#[derive(Debug)]
pub enum Overlay {
    Share(ShareState),
    Playlist(PlaylistState),
    Palette(PaletteState),
    /// First-run welcome, shown when nothing is configured yet.
    Setup(SetupState),
}

impl Overlay {
    /// Which popup this is, ignoring its contents. Lets the frame loop notice
    /// one popup being replaced by another, which changes what is covered.
    pub fn kind(&self) -> u8 {
        match self {
            Overlay::Share(_) => 0,
            Overlay::Playlist(_) => 1,
            Overlay::Palette(_) => 2,
            Overlay::Setup(_) => 3,
        }
    }
}

/// The first-run chooser.
///
/// wander used to refuse to start without a hand-written `config.toml`. Now it
/// opens instead, asks which kind of library the user has, and drops them on
/// the settings rows that set it up.
#[derive(Debug, Default)]
pub struct SetupState {
    pub selected: usize,
}

/// The choices offered on first run, and where each one leads.
pub const SETUP_CHOICES: &[(&str, &str)] = &[
    (
        "A Navidrome / Subsonic server",
        "Stream from your own server. You will need its URL and your login.",
    ),
    (
        "A folder of music on this computer",
        "Play local files. wander scans the folder and reads their tags.",
    ),
    (
        "Both",
        "Browse and queue server tracks and local files together.",
    ),
];

/// What a palette row does when chosen.
#[derive(Debug, Clone)]
pub enum PaletteTarget {
    /// Play these songs, starting at the given index.
    Songs { songs: Vec<Song>, index: usize },
    /// Open an artist, album or playlist in the Library.
    Reveal { kind: PaletteKind, id: String },
    /// Run a keybinding's action.
    Command(crate::keymap::Action),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteKind {
    Song,
    Album,
    Artist,
    Playlist,
    Command,
}

impl PaletteKind {
    pub fn glyph(self, glyphs: GlyphSet) -> &'static str {
        glyphs.icon(match self {
            PaletteKind::Song => Icon::Song,
            PaletteKind::Album => Icon::Album,
            PaletteKind::Artist => Icon::Artist,
            PaletteKind::Playlist => Icon::Playlist,
            PaletteKind::Command => Icon::Command,
        })
    }
}

#[derive(Debug, Clone)]
pub struct PaletteItem {
    pub kind: PaletteKind,
    pub label: String,
    /// Shown dimmed after the label: artist, album, or the bound key.
    pub detail: String,
    pub target: PaletteTarget,
}

#[derive(Debug, Default)]
pub struct PaletteState {
    pub query: String,
    /// Everything searchable, rebuilt when the palette opens.
    pub items: Vec<PaletteItem>,
    /// Indices into `items`, best match first.
    pub matches: Vec<usize>,
    pub selected: usize,
    /// Bumped per keystroke so a late server result can be discarded.
    pub generation: u64,
}

impl PaletteState {
    /// Re-rank `items` against the current query.
    pub fn refilter(&mut self) {
        let query = self.query.trim().to_lowercase();
        let mut scored: Vec<(i32, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                if query.is_empty() {
                    // Nothing typed yet: keep the natural order.
                    return Some((0, index));
                }
                let haystack = format!("{} {}", item.label, item.detail).to_lowercase();
                fuzzy_score(&query, &haystack).map(|score| (-score, index))
            })
            .collect();
        // Score descending (negated above), then original order as a tiebreak
        // so equally-good matches do not shuffle between keystrokes.
        scored.sort_by_key(|(score, index)| (*score, *index));
        self.matches = scored
            .into_iter()
            .map(|(_, index)| index)
            .take(200)
            .collect();
        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
    }

    pub fn chosen(&self) -> Option<&PaletteItem> {
        self.items.get(*self.matches.get(self.selected)?)
    }
}

/// fzf-style subsequence score, or `None` when `query` does not match.
///
/// Rewards matches at the start of a word and runs of adjacent characters, so
/// "cla pl" ranks "Claude's Plan" above an incidental scattering of the same
/// letters.
pub fn fuzzy_score(query: &str, haystack: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let haystack: Vec<char> = haystack.chars().collect();
    let mut score = 0;
    let mut at = 0usize;
    let mut previous_index: Option<usize> = None;

    for needle in query.chars() {
        if needle == ' ' {
            continue;
        }
        let found = haystack[at..].iter().position(|c| *c == needle)? + at;

        score += 1;
        if found == 0 || !haystack[found - 1].is_alphanumeric() {
            score += 6; // start of a word
        }
        if previous_index == Some(found.saturating_sub(1)) {
            score += 4; // contiguous with the previous match
        }
        // Earlier matches are usually the ones meant.
        score -= (found as i32 / 8).min(6);

        previous_index = Some(found);
        at = found + 1;
    }
    Some(score)
}

#[derive(Debug)]
pub struct ShareState {
    pub songs: Vec<Song>,
    /// What the link will be called for whoever opens it.
    pub label: String,
    pub description: String,
    pub expiry: usize,
    pub downloadable: bool,
    /// Which field the cursor is on: 0 description, 1 expiry, 2 downloadable.
    pub field: usize,
    pub pending: bool,
    /// Set once the server answers: the link, or the reason there isn't one.
    pub result: Option<Result<String, String>>,
}

impl ShareState {
    pub fn new(songs: Vec<Song>) -> Self {
        let label = match songs.as_slice() {
            [song] => format!("{} — {}", song.title, song.artist_or_unknown()),
            songs => format!("{} tracks", songs.len()),
        };
        Self {
            songs,
            label,
            description: String::new(),
            expiry: 2,
            downloadable: false,
            field: 0,
            pending: false,
            result: None,
        }
    }

    pub fn expires_ms(&self) -> Option<i64> {
        let (_, offset) = EXPIRIES[self.expiry.min(EXPIRIES.len() - 1)];
        let offset = offset?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        Some(now + offset)
    }
}

#[derive(Debug)]
pub struct PlaylistState {
    pub songs: Vec<Song>,
    pub playlists: Vec<Playlist>,
    pub selected: usize,
    /// Typing a name creates a new playlist instead of picking an existing one.
    pub new_name: String,
    pub creating: bool,
}

impl PlaylistState {
    pub fn new(songs: Vec<Song>, playlists: Vec<Playlist>) -> Self {
        Self {
            songs,
            playlists,
            selected: 0,
            new_name: String::new(),
            creating: false,
        }
    }
}

/// Centre a fixed-size box in `area`, clamped so it always fits.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

fn popup_block(title: &str, theme: &Theme) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border(true))
        // `Clear` wipes cells back to the terminal default, so a popup has to
        // repaint the theme's background itself.
        .style(theme.base())
        .title(format!(" {title} "))
        .title_style(theme.title())
}

pub fn draw(frame: &mut Frame, area: Rect, overlay: &Overlay, theme: &Theme, glyphs: GlyphSet) {
    match overlay {
        Overlay::Share(state) => draw_share(frame, area, state, theme),
        Overlay::Playlist(state) => draw_playlist(frame, area, state, theme),
        Overlay::Palette(state) => draw_palette(frame, area, state, theme, glyphs),
        Overlay::Setup(state) => draw_setup(frame, area, state, theme),
    }
}

fn draw_setup(frame: &mut Frame, area: Rect, state: &SetupState, theme: &Theme) {
    let width = (area.width * 3 / 4).clamp(40.min(area.width), 72);
    let height = (SETUP_CHOICES.len() as u16 * 3 + 7).min(area.height);
    let rect = centered(area, width, height);
    frame.render_widget(Clear, rect);

    let block = popup_block("Welcome to wander", theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.height < 3 {
        return;
    }

    let mut lines = vec![
        Line::from(Span::styled("Where is your music?", theme.title())),
        Line::from(""),
    ];

    for (index, (label, detail)) in SETUP_CHOICES.iter().enumerate() {
        let selected = index == state.selected;
        lines.push(Line::from(vec![
            Span::styled(
                if selected { "❯ " } else { "  " },
                if selected { theme.title() } else { theme.dim() },
            ),
            Span::styled(
                (*label).to_string(),
                if selected {
                    theme.selected()
                } else {
                    theme.base()
                },
            ),
        ]));
        lines.push(Line::from(Span::styled(
            format!("    {detail}"),
            theme.dim(),
        )));
        lines.push(Line::from(""));
    }

    lines.push(Line::from(vec![
        Span::styled("[↑/↓] ", theme.playing()),
        Span::styled("Choose   ", theme.dim()),
        Span::styled("[Enter] ", theme.playing()),
        Span::styled("Continue   ", theme.dim()),
        Span::styled("[Esc] ", theme.playing()),
        Span::styled("Skip", theme.dim()),
    ]));

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_palette(
    frame: &mut Frame,
    area: Rect,
    state: &PaletteState,
    theme: &Theme,
    glyphs: GlyphSet,
) {
    let width = (area.width * 3 / 4).clamp(40.min(area.width), 96);
    let height = (area.height * 2 / 3).clamp(8.min(area.height), 24);
    let rect = centered(area, width, height);
    frame.render_widget(Clear, rect);

    let block = popup_block("Go to", theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.height < 2 {
        return;
    }

    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("❯ ", theme.title()),
            Span::styled(format!("{}▏", state.query), theme.base()),
        ])),
        rows[0],
    );

    // Keep the selection on screen without a scrollbar: show the window that
    // ends at the selection once it passes the bottom.
    let visible = rows[1].height as usize;
    let start = state.selected.saturating_sub(visible.saturating_sub(1));
    let label_width = (rows[1].width as usize).saturating_sub(24);

    let lines: Vec<Line> = state
        .matches
        .iter()
        .skip(start)
        .take(visible)
        .enumerate()
        .filter_map(|(offset, item_index)| {
            let item = state.items.get(*item_index)?;
            let selected = start + offset == state.selected;
            let style = if selected {
                theme.selected()
            } else {
                theme.base()
            };
            Some(Line::from(vec![
                Span::styled(format!(" {} ", item.kind.glyph(glyphs)), theme.title()),
                Span::styled(
                    format!("{:<label_width$}", truncate(&item.label, label_width)),
                    style,
                ),
                Span::styled(format!("  {}", truncate(&item.detail, 20)), theme.dim()),
            ]))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), rows[1]);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(
                "{} matches  ·  [↑↓] pick  [enter] go  [esc] close",
                state.matches.len()
            ),
            theme.dim(),
        ))),
        rows[2],
    );
}

fn draw_share(frame: &mut Frame, area: Rect, state: &ShareState, theme: &Theme) {
    let rect = centered(area, 66, 13);
    frame.render_widget(Clear, rect);
    let block = popup_block("Share", theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let field = |index: usize, label: &str, value: String| -> Line<'static> {
        let marker = if state.field == index && state.result.is_none() {
            "❯ "
        } else {
            "  "
        };
        Line::from(vec![
            Span::styled(marker.to_string(), theme.playing()),
            Span::styled(format!("{label:<14}"), theme.dim()),
            Span::styled(value, theme.base()),
        ])
    };

    let mut lines = vec![
        Line::from(Span::styled(state.label.clone(), theme.playing())),
        Line::default(),
    ];

    match &state.result {
        Some(Ok(url)) => {
            lines.push(Line::from(Span::styled("Link created:", theme.dim())));
            lines.push(Line::from(Span::styled(url.clone(), theme.title())));
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                "Copied to the clipboard.  [esc] close",
                theme.dim(),
            )));
        }
        Some(Err(error)) => {
            lines.push(Line::from(Span::styled(error.clone(), theme.error())));
            lines.push(Line::default());
            lines.push(Line::from(Span::styled("[esc] close", theme.dim())));
        }
        None => {
            let description = if state.description.is_empty() {
                "(none)".to_string()
            } else {
                state.description.clone()
            };
            lines.push(field(0, "Description", description));
            lines.push(field(1, "Expires", EXPIRIES[state.expiry].0.to_string()));
            lines.push(field(
                2,
                "Downloadable",
                if state.downloadable {
                    "yes".into()
                } else {
                    "no".to_string()
                },
            ));
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                if state.pending {
                    "Creating link…"
                } else {
                    "[↑↓] field  [←→] change  [enter] create  [esc] cancel"
                },
                theme.dim(),
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_playlist(frame: &mut Frame, area: Rect, state: &PlaylistState, theme: &Theme) {
    let height = (state.playlists.len() as u16 + 7)
        .min(area.height.saturating_sub(4))
        .max(8);
    let rect = centered(area, 56, height);
    frame.render_widget(Clear, rect);
    let block = popup_block("Add to playlist", theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{} track(s)", state.songs.len()),
            theme.playing(),
        ))),
        rows[0],
    );

    if state.creating {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("New playlist: ", theme.dim()),
                Span::styled(format!("{}▏", state.new_name), theme.base()),
            ])),
            rows[1],
        );
    } else {
        let visible = rows[1].height as usize;
        let start = state.selected.saturating_sub(visible.saturating_sub(1));
        let lines: Vec<Line> = state
            .playlists
            .iter()
            .enumerate()
            .skip(start)
            .take(visible)
            .map(|(index, playlist)| {
                let style = if index == state.selected {
                    theme.selected()
                } else {
                    theme.base()
                };
                Line::from(Span::styled(
                    format!(" {} ({})", playlist.name, playlist.song_count),
                    style,
                ))
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), rows[1]);
    }

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            if state.creating {
                "[enter] create  [esc] back"
            } else {
                "[↑↓] pick  [enter] add  [n] new playlist  [esc] cancel"
            },
            theme.dim(),
        ))),
        rows[2],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frame loop compares these tags to notice one popup replacing
    /// another, which it cannot do if two popups share a tag.
    #[test]
    fn every_popup_has_its_own_kind() {
        let kinds = [
            Overlay::Share(ShareState::new(Vec::new())).kind(),
            Overlay::Playlist(PlaylistState::new(Vec::new(), Vec::new())).kind(),
            Overlay::Palette(PaletteState::default()).kind(),
            Overlay::Setup(SetupState::default()).kind(),
        ];
        let mut unique = kinds.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), kinds.len(), "kinds collide: {kinds:?}");
    }

    #[test]
    fn popup_is_clamped_to_a_small_screen() {
        let rect = centered(Rect::new(0, 0, 20, 6), 66, 13);
        assert_eq!((rect.width, rect.height), (20, 6));
        assert_eq!((rect.x, rect.y), (0, 0));
    }

    #[test]
    fn expiry_is_an_absolute_epoch_millisecond_stamp() {
        let mut state = ShareState::new(Vec::new());
        state.expiry = 0; // one hour
        let expires = state.expires_ms().expect("a bounded expiry");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert!(expires > now, "expiry is in the future");
        assert!(
            expires - now <= 3_600_000,
            "and no further out than an hour"
        );
    }

    fn item(label: &str) -> PaletteItem {
        PaletteItem {
            kind: PaletteKind::Song,
            label: label.to_string(),
            detail: String::new(),
            target: PaletteTarget::Songs {
                songs: Vec::new(),
                index: 0,
            },
        }
    }

    #[test]
    fn fuzzy_matching_needs_every_character_in_order() {
        assert!(fuzzy_score("cla", "claude's plan").is_some());
        assert!(fuzzy_score("clp", "claude's plan").is_some());
        assert!(
            fuzzy_score("plc", "claude's plan").is_none(),
            "order matters"
        );
        assert!(fuzzy_score("xyz", "claude's plan").is_none());
    }

    #[test]
    fn word_starts_outrank_incidental_letters() {
        // "cp" as two word initials should beat the same letters mid-word.
        let initials = fuzzy_score("cp", "claude plan").unwrap();
        let scattered = fuzzy_score("cp", "arcane opus").unwrap();
        assert!(initials > scattered, "{initials} should beat {scattered}");
    }

    #[test]
    fn contiguous_runs_outrank_spread_out_matches() {
        let together = fuzzy_score("plan", "the plan").unwrap();
        let apart = fuzzy_score("plan", "pale and neat").unwrap();
        assert!(together > apart, "{together} should beat {apart}");
    }

    #[test]
    fn an_empty_query_keeps_everything_in_its_original_order() {
        let mut state = PaletteState {
            items: vec![item("one"), item("two"), item("three")],
            ..Default::default()
        };
        state.refilter();
        assert_eq!(state.matches, vec![0, 1, 2]);
    }

    #[test]
    fn filtering_drops_non_matches_and_ranks_the_rest() {
        let mut state = PaletteState {
            items: vec![item("Gustave"), item("Lumière"), item("Lune")],
            ..Default::default()
        };
        state.query = "lu".to_string();
        state.refilter();

        let labels: Vec<&str> = state
            .matches
            .iter()
            .map(|i| state.items[*i].label.as_str())
            .collect();
        assert_eq!(labels.len(), 2, "Gustave does not match");
        assert!(labels.contains(&"Lune") && labels.contains(&"Lumière"));
    }

    #[test]
    fn the_selection_cannot_point_past_a_shrinking_match_list() {
        let mut state = PaletteState {
            items: vec![item("alpha"), item("beta"), item("gamma")],
            selected: 2,
            ..Default::default()
        };
        state.query = "alpha".to_string();
        state.refilter();
        assert_eq!(state.matches.len(), 1);
        assert_eq!(state.selected, 0);
        assert!(state.chosen().is_some());
    }

    #[test]
    fn server_default_expiry_sends_nothing() {
        let mut state = ShareState::new(Vec::new());
        state.expiry = EXPIRIES.len() - 1;
        assert_eq!(state.expires_ms(), None);
    }
}
