//! The settings screen.
//!
//! Rows are built from the config at draw time rather than being a fixed list,
//! because several of them are per-item: one row per local music folder, one
//! per queue column. `App::settings_sel.index` indexes this row list, so
//! keyboard selection, mouse hit-testing and scrolling all agree on what row
//! `n` is.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph};

use super::{Hits, Region};
use crate::app::{App, Pane};
use crate::config::Config;
use crate::theme::Theme;

/// How the settings list is grouped. Purely presentational: a header is drawn
/// whenever the section changes between consecutive rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Server,
    Local,
    Appearance,
    Playback,
    Integrations,
    QueueColumns,
    Keys,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Self::Server => "Server (Navidrome / Subsonic)",
            Self::Local => "Local library",
            Self::Appearance => "Appearance",
            Self::Playback => "Playback",
            Self::Integrations => "Integrations",
            Self::QueueColumns => "Queue columns",
            Self::Keys => "Keybindings",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingItem {
    // Server
    ServerEnabled,
    ServerUrl,
    ServerUsername,
    ServerPassword,
    StreamFormat,
    TestConnection,

    // Local library
    LocalPath(usize),
    AddLocalPath,
    LocalPlaylistDir,
    ScanOnStart,
    Rescan,

    // Appearance
    ThemePreset,
    Glyphs,
    CoverWidth,
    QueueWidth,
    ShowCover,
    ShowQueue,
    ShowLyrics,

    // Playback
    VolumeScale,
    BufferSeconds,
    AutoMix,
    ClearQueue,

    // Integrations
    DiscordEnabled,
    DiscordClientId,
    DiscordCoverArt,

    // Queue columns
    QueueColumn(usize),
    AddQueueColumn,

    // Keybindings
    ShowKeybindings,
}

impl SettingItem {
    pub fn section(self) -> Section {
        match self {
            Self::ServerEnabled
            | Self::ServerUrl
            | Self::ServerUsername
            | Self::ServerPassword
            | Self::StreamFormat
            | Self::TestConnection => Section::Server,

            Self::LocalPath(_)
            | Self::AddLocalPath
            | Self::LocalPlaylistDir
            | Self::ScanOnStart
            | Self::Rescan => Section::Local,

            Self::ThemePreset
            | Self::Glyphs
            | Self::CoverWidth
            | Self::QueueWidth
            | Self::ShowCover
            | Self::ShowQueue
            | Self::ShowLyrics => Section::Appearance,

            Self::VolumeScale | Self::BufferSeconds | Self::AutoMix | Self::ClearQueue => {
                Section::Playback
            }

            Self::DiscordEnabled | Self::DiscordClientId | Self::DiscordCoverArt => {
                Section::Integrations
            }

            Self::QueueColumn(_) | Self::AddQueueColumn => Section::QueueColumns,
            Self::ShowKeybindings => Section::Keys,
        }
    }

    /// Whether pressing Enter opens a text field on this row.
    pub fn is_text(self) -> bool {
        matches!(
            self,
            Self::ServerUrl
                | Self::ServerUsername
                | Self::ServerPassword
                | Self::LocalPath(_)
                | Self::AddLocalPath
                | Self::LocalPlaylistDir
                | Self::DiscordClientId
        )
    }

    /// Passwords are never echoed, and never written to `config.toml`.
    pub fn is_secret(self) -> bool {
        matches!(self, Self::ServerPassword)
    }

    /// Deliberately plain ASCII. Emoji are two columns wide but count as one
    /// character (and `🖼️` counts as two), so the padded value column drifted
    /// by a different amount on every row.
    pub fn title(self) -> String {
        match self {
            Self::ServerEnabled => "Use this server".into(),
            Self::ServerUrl => "Server URL".into(),
            Self::ServerUsername => "Username".into(),
            Self::ServerPassword => "Password".into(),
            Self::StreamFormat => "Audio stream format".into(),
            Self::TestConnection => "Test connection".into(),

            Self::LocalPath(index) => format!("Music folder {}", index + 1),
            Self::AddLocalPath => "Add music folder".into(),
            Self::LocalPlaylistDir => "Playlist folder".into(),
            Self::ScanOnStart => "Rescan at startup".into(),
            Self::Rescan => "Rescan now".into(),

            Self::ThemePreset => "Theme preset".into(),
            Self::Glyphs => "Icon set".into(),
            Self::CoverWidth => "Cover pane width".into(),
            Self::QueueWidth => "Up Next pane width".into(),
            Self::ShowCover => "Show cover pane".into(),
            Self::ShowQueue => "Show Up Next pane".into(),
            Self::ShowLyrics => "Show lyrics pane".into(),

            Self::VolumeScale => "Volume scaling curve".into(),
            Self::BufferSeconds => "Audio buffer".into(),
            Self::AutoMix => "Auto-Mix / radio mode".into(),
            Self::ClearQueue => "Clear the play queue".into(),

            Self::DiscordEnabled => "Discord Rich Presence".into(),
            Self::DiscordClientId => "Discord application ID".into(),
            Self::DiscordCoverArt => "Discord cover art".into(),

            Self::QueueColumn(index) => format!("Column {}", index + 1),
            Self::AddQueueColumn => "Add column".into(),

            Self::ShowKeybindings => "View keybindings".into(),
        }
    }
}

/// Build the row list for the current config.
///
/// The per-item rows (music folders, queue columns) are why this is a function
/// rather than a constant.
pub fn rows(config: &Config) -> Vec<SettingItem> {
    let mut rows = vec![
        SettingItem::ServerEnabled,
        SettingItem::ServerUrl,
        SettingItem::ServerUsername,
        SettingItem::ServerPassword,
        SettingItem::StreamFormat,
        SettingItem::TestConnection,
    ];

    rows.extend((0..config.local.paths.len()).map(SettingItem::LocalPath));
    rows.extend([
        SettingItem::AddLocalPath,
        SettingItem::LocalPlaylistDir,
        SettingItem::ScanOnStart,
        SettingItem::Rescan,
        SettingItem::ThemePreset,
        SettingItem::Glyphs,
        SettingItem::CoverWidth,
        SettingItem::QueueWidth,
        SettingItem::ShowCover,
        SettingItem::ShowQueue,
        SettingItem::ShowLyrics,
        SettingItem::VolumeScale,
        SettingItem::BufferSeconds,
        SettingItem::AutoMix,
        SettingItem::ClearQueue,
        SettingItem::DiscordEnabled,
        SettingItem::DiscordClientId,
        SettingItem::DiscordCoverArt,
    ]);

    rows.extend((0..config.queue_columns.len()).map(SettingItem::QueueColumn));
    rows.push(SettingItem::AddQueueColumn);
    rows.push(SettingItem::ShowKeybindings);

    rows
}

/// The value column for a row, as displayed.
fn value_of(app: &App, item: SettingItem) -> String {
    let config = &app.config;
    match item {
        SettingItem::ServerEnabled => on_off(config.server.enabled),
        SettingItem::ServerUrl => {
            if config.server.url.is_empty() {
                "(not set — Enter to type one)".into()
            } else {
                config.server.url.clone()
            }
        }
        SettingItem::ServerUsername => {
            if config.server.username.is_empty() {
                "(not set)".into()
            } else {
                config.server.username.clone()
            }
        }
        SettingItem::ServerPassword => {
            if app.has_stored_password {
                "•••••••• (stored in the OS keyring)".into()
            } else {
                "(not set)".into()
            }
        }
        SettingItem::StreamFormat => format!(
            "{}  (raw | mp3 | opus | flac)",
            config.server.format.as_deref().unwrap_or("raw (native)")
        ),
        SettingItem::TestConnection => app
            .connection_status
            .clone()
            .unwrap_or_else(|| "[Enter to check the server]".into()),

        SettingItem::LocalPath(index) => config
            .local
            .paths
            .get(index)
            .map(|p| format!("{}  (Enter to edit, Delete to remove)", p.display()))
            .unwrap_or_default(),
        SettingItem::AddLocalPath => "[Enter to add a folder]".into(),
        SettingItem::LocalPlaylistDir => config
            .local
            .playlist_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(not set — local playlists disabled)".into()),
        SettingItem::ScanOnStart => on_off(config.local.scan_on_start),
        SettingItem::Rescan => app.scan_status.clone().unwrap_or_else(|| {
            // Report what the persisted index already holds, so the row is
            // informative before the user has scanned anything this session.
            match app.library_root.as_ref().and_then(|root| root.local()) {
                Some(local) if local.track_count() > 0 => format!(
                    "{} songs, {} albums indexed  [Enter to rescan]",
                    local.track_count(),
                    local.album_count()
                ),
                _ => "[Enter to scan your music folders]".into(),
            }
        }),

        SettingItem::ThemePreset => format!(
            "{}  (Left/Right to cycle)",
            config.theme_preset.as_deref().unwrap_or("Custom")
        ),
        SettingItem::Glyphs => format!("{:?}  (nerd | unicode | ascii)", config.glyphs),
        SettingItem::CoverWidth => format!("{}%", app.cover_percent),
        SettingItem::QueueWidth => format!("{}%", app.queue_percent),
        SettingItem::ShowCover => on_off(app.show_cover_pane),
        SettingItem::ShowQueue => on_off(app.show_queue_pane),
        SettingItem::ShowLyrics => on_off(app.show_lyrics_pane),

        SettingItem::VolumeScale => if config.volume_log {
            "Logarithmic (perceptual)"
        } else {
            "Linear"
        }
        .into(),
        SettingItem::BufferSeconds => format!("{:.1}s  (restart to apply)", config.buffer_seconds),
        SettingItem::AutoMix => {
            if app.player.queue.lock().unwrap().radio {
                "Enabled (auto-queues similar songs)".into()
            } else {
                "Disabled".into()
            }
        }
        SettingItem::ClearQueue => "[Enter to clear]".into(),

        SettingItem::DiscordEnabled => {
            if config.discord.enabled {
                // Rich Presence fails silently, so show what actually happened
                // to the cover art rather than just "Enabled".
                let art = app
                    .discord_diagnostic
                    .as_ref()
                    .and_then(|d| d.lock().ok().map(|d| d.clone()))
                    .unwrap_or_else(|| "starting…".to_string());
                format!("Enabled — art: {}", super::widgets::truncate(&art, 50))
            } else {
                "Disabled".into()
            }
        }
        SettingItem::DiscordClientId => {
            if config.discord.client_id.is_empty() {
                "(using the built-in application)".into()
            } else {
                config.discord.client_id.clone()
            }
        }
        SettingItem::DiscordCoverArt => on_off(config.discord.cover_art),

        SettingItem::QueueColumn(index) => config
            .queue_columns
            .get(index)
            .map(|column| {
                format!(
                    "{:<7} {:>3}%  (Left/Right width, Enter kind, Delete remove)",
                    column.kind.header(),
                    column.width
                )
            })
            .unwrap_or_default(),
        SettingItem::AddQueueColumn => "[Enter to add]".into(),

        SettingItem::ShowKeybindings => "[Enter to open the help screen]".into(),
    }
}

fn on_off(value: bool) -> String {
    if value { "ON".into() } else { "OFF".into() }
}

pub fn draw(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme, hits: &mut Hits) {
    let focused = app.focus == Pane::Settings;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border(focused))
        .style(theme.base())
        .title(" Settings ")
        .title_style(theme.title());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let chunks = Layout::vertical([
        Constraint::Length(2), // shortcuts bar
        Constraint::Min(3),    // settings list
    ])
    .split(inner);

    // The shortcut hints change while editing, because the keys do.
    let help_line = if app.settings_edit.is_some() {
        Line::from(vec![
            Span::styled("Editing: ", theme.title()),
            Span::styled("[Enter] ", theme.playing()),
            Span::raw("Save  "),
            Span::styled("[Esc] ", theme.playing()),
            Span::raw("Cancel  "),
            Span::styled("[Ctrl-W] ", theme.playing()),
            Span::raw("Delete word  "),
            Span::styled("[Ctrl-U] ", theme.playing()),
            Span::raw("Clear"),
        ])
    } else {
        Line::from(vec![
            Span::styled("Shortcuts: ", theme.title()),
            Span::styled("[j/k] ", theme.playing()),
            Span::raw("Navigate  "),
            Span::styled("[h/l] ", theme.playing()),
            Span::raw("Change  "),
            Span::styled("[Enter] ", theme.playing()),
            Span::raw("Edit / activate  "),
            Span::styled("[Del] ", theme.playing()),
            Span::raw("Remove"),
        ])
    };
    frame.render_widget(Paragraph::new(help_line), chunks[0]);

    let list_area = chunks[1];
    let rows = rows(&app.config);
    if rows.is_empty() {
        return;
    }
    let selected = app.settings_sel.index.min(rows.len() - 1);

    // Label column, sized to the longest title so values line up, but never
    // taking so much of a narrow terminal that no value is visible.
    let label_width = rows
        .iter()
        .map(|item| item.title().chars().count())
        .max()
        .unwrap_or(20)
        .clamp(16, (inner.width as usize / 2).max(16));

    let mut items: Vec<ListItem> = Vec::with_capacity(rows.len());
    let mut last_section: Option<Section> = None;

    for (index, &item) in rows.iter().enumerate() {
        let is_selected = index == selected;

        // A section header is drawn as part of the first row in the section,
        // so the row list and the selection index stay one-to-one.
        let mut lines: Vec<Line> = Vec::new();
        if last_section != Some(item.section()) {
            if last_section.is_some() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                item.section().title().to_string(),
                theme.title(),
            )));
            last_section = Some(item.section());
        }

        let value_width = (inner.width as usize)
            .saturating_sub(label_width + 4)
            .max(8);

        // The row being edited shows the live field instead of the stored value.
        let (value_text, value_style) = match (&app.settings_edit, is_selected) {
            (Some(input), true) => {
                let shown = input.display(value_width);
                let caret = input.display_cursor(value_width);
                // A block caret drawn into the text, since the terminal cursor
                // is not positioned by this widget.
                let mut chars: Vec<char> = shown.chars().collect();
                while chars.len() <= caret {
                    chars.push(' ');
                }
                let mut text: String = chars[..caret].iter().collect();
                text.push('▏');
                text.extend(chars[caret..].iter());
                (text, theme.playing())
            }
            _ => (
                super::widgets::truncate(&value_of(app, item), value_width),
                if is_selected {
                    theme.playing()
                } else {
                    theme.dim()
                },
            ),
        };

        lines.push(Line::from(vec![
            Span::styled(
                if is_selected { "❯ " } else { "  " },
                if is_selected {
                    theme.title()
                } else {
                    theme.dim()
                },
            ),
            Span::styled(
                format!("{:<width$} ", item.title(), width = label_width),
                if is_selected {
                    theme.selected()
                } else {
                    theme.base()
                },
            ),
            Span::styled(value_text, value_style),
        ]));

        items.push(ListItem::new(lines));
    }

    let mut state = ListState::default().with_selected(Some(selected));
    // Carry the scroll offset across frames so a long list does not jump back
    // to the top every redraw.
    *state.offset_mut() = app.settings_sel.offset;
    frame.render_stateful_widget(List::new(items), list_area, &mut state);
    app.settings_sel.offset = state.offset();

    register_hits(hits, list_area, &rows, app.settings_sel.offset);
}

/// Register one clickable rect per visible row.
///
/// Row heights vary because section headers ride along with the first row of
/// each section, so this walks the same heights the list widget used.
fn register_hits(hits: &mut Hits, area: Rect, rows: &[SettingItem], offset: usize) {
    let mut y = area.y;
    let mut last_section: Option<Section> = rows
        .get(offset.saturating_sub(1))
        .filter(|_| offset > 0)
        .map(|item| item.section());

    for (index, item) in rows.iter().enumerate().skip(offset) {
        let mut height = 1;
        if last_section != Some(item.section()) {
            height += 1; // the section header
            if last_section.is_some() {
                height += 1; // the blank spacer above it
            }
            last_section = Some(item.section());
        }

        if y >= area.y + area.height {
            break;
        }
        let visible = height.min((area.y + area.height - y) as usize) as u16;
        hits.push(
            Rect {
                x: area.x,
                y,
                width: area.width,
                height: visible,
            },
            Region::Row {
                pane: Pane::Settings,
                index,
            },
        );
        y += visible;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The value column is padded by character count, so a title whose display
    /// width differs from its character count shifts that row's value out of
    /// line with every other row.
    #[test]
    fn setting_titles_are_one_column_per_character() {
        for item in rows(&Config::default()) {
            let title = item.title();
            assert!(
                title.is_ascii(),
                "{title:?} is not plain ASCII, so its width is not its length"
            );
        }
    }

    #[test]
    fn setting_titles_fit_the_padded_column() {
        for item in rows(&Config::default()) {
            assert!(
                item.title().len() <= 24,
                "{:?} overflows the label column",
                item.title()
            );
        }
    }

    /// Rows are grouped by section for rendering; a section that reappears
    /// after another one would draw a duplicate header.
    #[test]
    fn sections_appear_in_one_contiguous_run() {
        let rows = rows(&Config::default());
        let mut seen: Vec<Section> = Vec::new();
        let mut last: Option<Section> = None;
        for item in rows {
            if last != Some(item.section()) {
                assert!(
                    !seen.contains(&item.section()),
                    "{:?} is split across the list",
                    item.section()
                );
                seen.push(item.section());
                last = Some(item.section());
            }
        }
    }

    /// Per-item rows must line up with the config they came from, or editing
    /// row N would edit a different folder.
    #[test]
    fn per_item_rows_track_the_config() {
        let mut config = Config::default();
        let before = rows(&config).len();

        config.local.paths.push("/music".into());
        config.local.paths.push("/more".into());
        let rows = rows(&config);
        assert_eq!(rows.len(), before + 2);
        assert!(rows.contains(&SettingItem::LocalPath(0)));
        assert!(rows.contains(&SettingItem::LocalPath(1)));
        assert!(!rows.contains(&SettingItem::LocalPath(2)));
    }

    #[test]
    fn only_the_password_row_is_secret() {
        let secret: Vec<SettingItem> = rows(&Config::default())
            .into_iter()
            .filter(|item| item.is_secret())
            .collect();
        assert_eq!(secret, vec![SettingItem::ServerPassword]);
        assert!(SettingItem::ServerPassword.is_text());
    }
}
