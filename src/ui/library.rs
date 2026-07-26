use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph};

use super::glyphs::Icon;
use super::queue::register_rows;
use super::widgets::truncate;
use super::{Hits, Region};
use crate::app::{App, LibraryMode, Pane, Selection, format_duration};
use crate::theme::Theme;

/// Draw a bordered, scrollable list pane and register its rows for the mouse.
#[allow(clippy::too_many_arguments)]
fn list_pane(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    items: Vec<String>,
    selection: &mut Selection,
    pane: Pane,
    focused: bool,
    theme: &Theme,
    hits: &mut Hits,
) {
    selection.clamp(items.len());
    let count = items.len();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border(focused))
        .style(theme.base())
        .title(truncate(
            &format!(" {title} ({count}) "),
            area.width.saturating_sub(2) as usize,
        ))
        .title_style(if focused { theme.title() } else { theme.dim() });
    let inner = block.inner(area);

    let list = List::new(items.into_iter().map(ListItem::new).collect::<Vec<_>>())
        .highlight_style(theme.selected())
        .block(block);

    let mut state = ListState::default().with_selected(Some(selection.index));
    frame.render_stateful_widget(list, area, &mut state);

    register_rows(hits, inner, count, selection.index, pane, 0);
}

/// The Library tab: a mode selector, then the browser for that mode.
///
/// Artists, albums and tracks are three ways into the same library rather than
/// separate destinations, so they share one tab and one header.
pub fn draw_library(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme, hits: &mut Hits) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).split(area);
    draw_mode_selector(frame, rows[0], app, theme, hits);

    match app.library_mode {
        LibraryMode::Artists => draw_artists(frame, rows[1], app, theme, hits),
        LibraryMode::Albums => draw_albums(frame, rows[1], app, theme, hits),
        LibraryMode::Playlists => draw_playlists(frame, rows[1], app, theme, hits),
        LibraryMode::Tracks => {
            let songs: Vec<String> = app
                .tracks
                .iter()
                .map(|song| flat_track_label(song, app))
                .collect();
            let focused = app.focus == Pane::Tracks;
            list_pane(
                frame,
                rows[1],
                "Tracks",
                songs,
                &mut app.track_sel,
                Pane::Tracks,
                focused,
                theme,
                hits,
            );
        }
        LibraryMode::Favorites => {
            let songs: Vec<String> = app
                .favorites
                .iter()
                .map(|song| flat_track_label(song, app))
                .collect();
            let focused = app.focus == Pane::Favorites;
            list_pane(
                frame,
                rows[1],
                "Favorites",
                songs,
                &mut app.favorite_sel,
                Pane::Favorites,
                focused,
                theme,
                hits,
            );
        }
    }
}

fn draw_mode_selector(frame: &mut Frame, area: Rect, app: &App, theme: &Theme, hits: &mut Hits) {
    let mut spans = vec![Span::styled("  ", theme.dim())];
    let mut x = area.x + 2;
    for (index, mode) in LibraryMode::ALL.iter().enumerate() {
        let label = format!(" {} ", mode.title());
        let width = label.chars().count() as u16;
        let style = if *mode == app.library_mode {
            theme.selected()
        } else {
            theme.dim()
        };
        // Registered per-label so the selector is clickable, like the tab bar.
        hits.push(
            Rect {
                x,
                y: area.y,
                width,
                height: 1,
            },
            Region::LibraryMode(index),
        );
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
        x += width + 1;
    }
    spans.push(Span::styled("  [m] switch", theme.dim()));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Ranger-style three-column browser: artists, their albums, then tracks.
pub fn draw_artists(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme, hits: &mut Hits) {
    let columns = Layout::horizontal([
        Constraint::Percentage(30),
        Constraint::Percentage(35),
        Constraint::Percentage(35),
    ])
    .split(area);

    let artists: Vec<String> = app.artists.iter().map(|a| a.name.clone()).collect();
    let albums: Vec<String> = app
        .artist_albums
        .iter()
        .map(|a| match a.year {
            Some(year) => format!("{} ({year})", a.name),
            None => a.name.clone(),
        })
        .collect();
    let songs: Vec<String> = app.artist_songs.iter().map(track_label).collect();

    let focus = app.focus;
    list_pane(
        frame,
        columns[0],
        "Artists",
        artists,
        &mut app.artist_sel,
        Pane::Artists,
        focus == Pane::Artists,
        theme,
        hits,
    );
    list_pane(
        frame,
        columns[1],
        "Albums",
        albums,
        &mut app.artist_album_sel,
        Pane::ArtistAlbums,
        focus == Pane::ArtistAlbums,
        theme,
        hits,
    );
    list_pane(
        frame,
        columns[2],
        "Tracks",
        songs,
        &mut app.artist_song_sel,
        Pane::ArtistSongs,
        focus == Pane::ArtistSongs,
        theme,
        hits,
    );
}

/// Two-column albums browser.
pub fn draw_albums(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme, hits: &mut Hits) {
    let columns =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).split(area);

    let albums: Vec<String> = app
        .albums
        .iter()
        .map(|a| format!("{} — {}", a.artist.as_deref().unwrap_or("Unknown"), a.name))
        .collect();
    let songs: Vec<String> = app.album_songs.iter().map(track_label).collect();

    let focus = app.focus;
    list_pane(
        frame,
        columns[0],
        "Albums",
        albums,
        &mut app.album_sel,
        Pane::Albums,
        focus == Pane::Albums,
        theme,
        hits,
    );
    list_pane(
        frame,
        columns[1],
        "Tracks",
        songs,
        &mut app.album_song_sel,
        Pane::AlbumSongs,
        focus == Pane::AlbumSongs,
        theme,
        hits,
    );
}

/// Two-column playlists browser.
pub fn draw_playlists(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    theme: &Theme,
    hits: &mut Hits,
) {
    let columns =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)]).split(area);

    let playlists: Vec<String> = app
        .playlists
        .iter()
        .map(|p| format!("{} ({})", p.name, p.song_count))
        .collect();
    let songs: Vec<String> = app.playlist_songs.iter().map(track_label).collect();

    let focus = app.focus;
    list_pane(
        frame,
        columns[0],
        "Playlists",
        playlists,
        &mut app.playlist_sel,
        Pane::Playlists,
        focus == Pane::Playlists,
        theme,
        hits,
    );
    list_pane(
        frame,
        columns[1],
        "Tracks",
        songs,
        &mut app.playlist_song_sel,
        Pane::PlaylistSongs,
        focus == Pane::PlaylistSongs,
        theme,
        hits,
    );
}

/// In a flat list there is no album context, so name the artist and album.
fn flat_track_label(song: &crate::subsonic::models::Song, app: &App) -> String {
    let duration = format_duration(std::time::Duration::from_secs(song.duration as u64));
    let star = if song.is_starred() {
        format!("{} ", app.config.glyphs.icon(Icon::Star))
    } else {
        String::new()
    };
    let rating = match app.rating_stars(song) {
        stars if stars.is_empty() => String::new(),
        stars => format!("  {stars}"),
    };
    format!(
        "{star}{}  ·  {}  ·  {}  [{duration}]{rating}",
        song.title,
        song.artist_or_unknown(),
        song.album_or_unknown()
    )
}

fn track_label(song: &crate::subsonic::models::Song) -> String {
    let duration = format_duration(std::time::Duration::from_secs(song.duration as u64));
    match song.track {
        Some(track) => format!("{track:>2}. {}  [{duration}]", song.title),
        None => format!("{}  [{duration}]", song.title),
    }
}
