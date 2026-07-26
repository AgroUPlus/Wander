pub mod cover;
pub mod glyphs;
pub mod help;
pub mod home;
pub mod library;
pub mod lyrics;
pub mod overlay;
pub mod player_bar;
pub mod queue;
pub mod settings;
pub mod tabs;
pub mod visualiser;
pub mod widgets;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::{App, Pane, Tab};
use cover::CoverRenderer;

/// Narrowest a lyrics column may be before it stops being worth showing.
const MIN_LYRICS_WIDTH: u16 = 24;
/// And the artwork's own floor, so neither pane collapses.
const MIN_COVER_WIDTH: u16 = 12;

/// A clickable area of the screen, recorded while drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Tab(usize),
    /// One of the Library tab's view buttons.
    LibraryMode(usize),
    PlayPause,
    Repeat,
    Shuffle,
    Seek,
    Volume,
    CurrentArtist,
    CurrentAlbum,
    /// The spectrum panel; clicking it steps to the next drawing style.
    Visualiser,
    /// The cover art pane; clicking it opens the project's GitHub page.
    Cover,
    /// A row in a list or table. `index` is the row's position in its list.
    Row {
        pane: Pane,
        index: usize,
    },
}

/// Maps screen coordinates back to what was drawn there.
///
/// Rebuilt every frame during `draw`, so it can never disagree with what the
/// user is actually looking at — which is the failure mode of hard-coding
/// hit areas separately from layout.
#[derive(Debug, Default)]
pub struct Hits {
    regions: Vec<(Rect, Region)>,
}

impl Hits {
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    pub fn push(&mut self, area: Rect, region: Region) {
        self.regions.push((area, region));
    }

    /// The region at a point. Later entries win, so panes drawn on top of
    /// others (overlays) take precedence.
    pub fn at(&self, x: u16, y: u16) -> Option<Region> {
        self.regions
            .iter()
            .rev()
            .find(|(area, _)| {
                x >= area.x && x < area.x + area.width && y >= area.y && y < area.y + area.height
            })
            .map(|(_, region)| *region)
    }

    /// The on-screen rect of a region, needed to turn a click into a ratio.
    pub fn rect_of(&self, region: Region) -> Option<Rect> {
        self.regions
            .iter()
            .find(|(_, r)| *r == region)
            .map(|(area, _)| *area)
    }
}

/// Draw one frame. Widgets read from `App` and never perform I/O.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    frame: &mut Frame,
    app: &mut App,
    covers: &mut CoverRenderer,
    spectrum: &mut crate::player::spectrum::Spectrum,
    viz: &mut visualiser::Visualiser,
    hits: &mut Hits,
) {
    hits.clear();
    let theme = app.theme.clone();
    let area = frame.area();

    // Paint the theme's background once, underneath everything. Without this,
    // any cell a widget does not explicitly style keeps the terminal's own
    // colours, which is why a theme used to only "half" apply.
    frame.render_widget(Block::default().style(theme.base()), area);

    let rows = Layout::vertical([
        Constraint::Length(3), // tabs
        Constraint::Min(5),    // body
        Constraint::Length(player_bar::HEIGHT),
        Constraint::Length(1), // status line
    ])
    .split(area);

    if app.focus_mode {
        // Nothing but the music: cover, lyrics, visualiser, transport.
        let rows = Layout::vertical([
            Constraint::Min(5),
            Constraint::Length(player_bar::HEIGHT),
            Constraint::Length(1),
        ])
        .split(area);
        draw_focus(frame, rows[0], app, covers, spectrum, viz, &theme, hits);
        player_bar::draw(frame, rows[1], app, &theme, hits);
        frame.render_widget(
            Paragraph::new(
                "c cover  •  y lyrics  •  Q queue  •  v visualiser  •  F or Esc to leave",
            )
            .style(theme.dim())
            .centered(),
            rows[2],
        );
        if let Some(active) = app.overlay.as_ref() {
            overlay::draw(frame, area, active, &theme, app.config.glyphs);
        }
        return;
    }

    tabs::draw(frame, rows[0], app, &theme, hits);
    draw_body(frame, rows[1], app, covers, spectrum, viz, &theme, hits);
    player_bar::draw(frame, rows[2], app, &theme, hits);
    draw_status(frame, rows[3], app, &theme);

    // Overlays sit above everything, including the help sheet.
    if app.show_help {
        help::draw(frame, area, app, &theme);
    }
    if let Some(active) = app.overlay.as_ref() {
        overlay::draw(frame, area, active, &theme, app.config.glyphs);
    }
}

/// Focus mode: cover on the left, lyrics on the right, visualiser beneath.
/// If cover is hidden, visualiser takes the cover room.
///
/// Reuses the ordinary pane renderers rather than introducing a second layout
/// path, so anything fixed in one is fixed in both.
#[allow(clippy::too_many_arguments)]
fn draw_focus(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    covers: &mut CoverRenderer,
    spectrum: &mut crate::player::spectrum::Spectrum,
    viz: &mut visualiser::Visualiser,
    theme: &crate::theme::Theme,
    hits: &mut Hits,
) {
    let (_, queue_percent, viz_height) = app.tween_panes();
    let show_viz_bottom =
        app.show_cover_pane && app.show_visualiser && area.height > viz_height + 8;
    let show_viz_in_middle = !app.show_cover_pane && app.show_visualiser;

    let mut constraints = vec![Constraint::Length(3), Constraint::Min(5)];
    if show_viz_bottom {
        constraints.push(Constraint::Length(viz_height));
    }
    let rows = Layout::vertical(constraints).split(area);

    draw_focus_header(frame, rows[0], app, theme);

    // Up Next is togglable here too, so focus mode can still be used to steer
    // what plays rather than only to watch it.
    let (middle, queue_area) = if app.show_queue_pane {
        let split =
            Layout::horizontal([Constraint::Min(20), Constraint::Percentage(queue_percent)])
                .split(rows[1]);
        (split[0], Some(split[1]))
    } else {
        (rows[1], None)
    };

    if app.show_cover_pane {
        // Lyrics have their own toggle here (`y` / `L`); focus mode is a reading
        // view, so they are on by default.
        let cover_width = focus_cover_width(
            middle.width,
            covers.width_for_height(middle.height),
            app.show_focus_lyrics,
        );
        match cover_width {
            Some(cover_width) => {
                let columns = Layout::horizontal([
                    Constraint::Length(cover_width),
                    Constraint::Min(MIN_LYRICS_WIDTH),
                ])
                .split(middle);
                hits.push(columns[0], Region::Cover);
                covers.draw(frame, columns[0], app, theme);
                lyrics::draw(frame, columns[1], app, theme, hits);
            }
            None => {
                hits.push(middle, Region::Cover);
                covers.draw(frame, middle, app, theme);
            }
        }
    } else if show_viz_in_middle {
        // Cover is hidden; visualizer takes the cover room!
        if app.show_focus_lyrics && middle.width >= MIN_LYRICS_WIDTH + MIN_COVER_WIDTH {
            let viz_width =
                focus_cover_width(middle.width, covers.width_for_height(middle.height), true)
                    .unwrap_or(middle.width / 2);
            let columns = Layout::horizontal([
                Constraint::Length(viz_width),
                Constraint::Min(MIN_LYRICS_WIDTH),
            ])
            .split(middle);
            hits.push(columns[0], Region::Visualiser);
            viz.draw(frame, columns[0], spectrum, app.viz_mode, theme);
            lyrics::draw(frame, columns[1], app, theme, hits);
        } else {
            hits.push(middle, Region::Visualiser);
            viz.draw(frame, middle, spectrum, app.viz_mode, theme);
        }
    } else if app.show_focus_lyrics {
        lyrics::draw(frame, middle, app, theme, hits);
    }

    if let Some(queue_area) = queue_area {
        queue::draw(frame, queue_area, app, theme, hits, Pane::Queue, false);
    }
    if show_viz_bottom {
        hits.push(rows[2], Region::Visualiser);
        viz.draw(frame, rows[2], spectrum, app.viz_mode, theme);
    }
}

/// How wide the cover pane should be in focus mode, or `None` to give it the
/// whole area because the lyrics are off or there is no room for them.
///
/// The artwork takes every column its height allows, capped so the lyrics keep
/// a readable column — otherwise turning them on appeared to do nothing, which
/// is how they went missing twice.
fn focus_cover_width(available: u16, ideal: u16, show_lyrics: bool) -> Option<u16> {
    if !show_lyrics || available < MIN_LYRICS_WIDTH + MIN_COVER_WIDTH {
        return None;
    }
    Some(
        ideal
            .min(available.saturating_sub(MIN_LYRICS_WIDTH))
            .max(MIN_COVER_WIDTH),
    )
}

/// The track, spelled out large, because focus mode is meant to be readable
/// from across the room rather than scanned.
fn draw_focus_header(frame: &mut Frame, area: Rect, app: &App, theme: &crate::theme::Theme) {
    let status = app.player.status();
    let width = area.width.saturating_sub(4) as usize;

    let lines = match status.current.as_ref() {
        Some(song) => vec![
            Line::from(Span::styled(
                widgets::truncate(&song.title, width),
                theme.playing(),
            ))
            .centered(),
            Line::from(vec![
                Span::styled(
                    widgets::truncate(song.artist_or_unknown(), width / 2),
                    theme.title(),
                ),
                Span::styled("  ·  ", theme.dim()),
                Span::styled(
                    widgets::truncate(song.album_or_unknown(), width / 2),
                    theme.dim(),
                ),
            ])
            .centered(),
        ],
        None => vec![Line::from(Span::styled("Nothing playing", theme.dim())).centered()],
    };

    frame.render_widget(Paragraph::new(lines), area);
}

#[allow(clippy::too_many_arguments)]
fn draw_body(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    covers: &mut CoverRenderer,
    spectrum: &mut crate::player::spectrum::Spectrum,
    viz: &mut visualiser::Visualiser,
    theme: &crate::theme::Theme,
    hits: &mut Hits,
) {
    let side_queue = app.side_queue_visible();
    let side_lyrics = app.show_lyrics_pane;

    // Eased sizes, so resizing glides rather than stepping.
    let (cover_percent, queue_percent, viz_height) = app.tween_panes();

    let (body_area, mode_bar_area) = if app.tab == Tab::Library {
        let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).split(area);
        (rows[1], Some(rows[0]))
    } else {
        (area, None)
    };

    if let Some(mode_area) = mode_bar_area {
        library::draw_mode_selector(frame, mode_area, app, theme, hits);
    }

    let mut constraints = Vec::new();
    constraints.push(Constraint::Min(20));
    if app.show_cover_pane || side_lyrics || app.show_visualiser {
        constraints.push(Constraint::Percentage(cover_percent));
    }
    if side_queue {
        constraints.push(Constraint::Percentage(queue_percent));
    }
    let columns = Layout::horizontal(constraints).split(body_area);

    let mut next = 1;
    let content = columns[0];
    let side_area = if app.show_cover_pane || side_lyrics || app.show_visualiser {
        let a = columns[next];
        next += 1;
        Some(a)
    } else {
        None
    };
    let queue_area = if side_queue {
        Some(columns[next])
    } else {
        None
    };

    match app.tab {
        Tab::Home => home::draw(frame, content, app, theme, hits),
        Tab::Queue => queue::draw(frame, content, app, theme, hits, Pane::Queue, true),
        Tab::Library => library::draw_library(frame, content, app, theme, hits),
        Tab::Settings => settings::draw(frame, content, app, theme, hits),
    }

    if let Some(side) = side_area {
        // The side column stacks whichever of cover / lyrics / visualiser are
        // enabled. The visualiser takes a fixed height; the rest share what is
        // left, so turning one off simply gives its space to the others.
        let show_viz = app.show_visualiser && side.height > viz_height + 4;
        let flexible = app.show_cover_pane as u16 + side_lyrics as u16;

        let mut constraints = Vec::new();
        for _ in 0..flexible {
            constraints.push(Constraint::Ratio(1, flexible.max(1) as u32));
        }
        if show_viz {
            constraints.push(Constraint::Length(viz_height));
        }
        let sections = Layout::vertical(constraints).split(side);

        let mut next = 0;
        if app.show_cover_pane {
            hits.push(sections[next], Region::Cover);
            covers.draw(frame, sections[next], app, theme);
            next += 1;
        }
        if side_lyrics {
            lyrics::draw(frame, sections[next], app, theme, hits);
            next += 1;
        }
        if show_viz {
            hits.push(sections[next], Region::Visualiser);
            viz.draw(frame, sections[next], spectrum, app.viz_mode, theme);
        }
    }
    if let Some(queue_area) = queue_area {
        queue::draw(frame, queue_area, app, theme, hits, Pane::Queue, false);
    }
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App, theme: &crate::theme::Theme) {
    let text = app.status_message.clone().unwrap_or_else(|| {
        let radio_active = app.player.queue.lock().unwrap().radio;
        let radio_badge = if radio_active { "  •  Auto-Mix ON" } else { "" };
        format!(
            "space play/pause  •  n/p skip  •  a queue  •  / go to anything  •  m library view  •  F focus  •  C-h keys{radio_badge}"
        )
    });
    frame.render_widget(Paragraph::new(text).style(theme.dim()).centered(), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_mode_shows_lyrics_by_default_at_any_usable_width() {
        for width in [40u16, 60, 100, 200, 400] {
            // A tall terminal makes the ideal cover wider than the pane, which
            // is exactly the case that used to hide the lyrics entirely.
            let split = focus_cover_width(width, 9999, true);
            let cover = split.expect("lyrics should be visible at width {width}");
            assert!(
                width - cover >= MIN_LYRICS_WIDTH,
                "lyrics got {} columns at width {width}",
                width - cover
            );
            assert!(cover >= MIN_COVER_WIDTH, "cover collapsed at width {width}");
        }
    }

    #[test]
    fn turning_lyrics_off_gives_the_artwork_everything() {
        assert_eq!(focus_cover_width(120, 60, false), None);
    }

    #[test]
    fn a_pane_too_narrow_for_both_shows_only_the_artwork() {
        assert_eq!(focus_cover_width(20, 10, true), None);
    }

    #[test]
    fn a_modest_cover_does_not_steal_the_lyrics_column() {
        // Ideal fits comfortably: both panes get what they asked for.
        assert_eq!(focus_cover_width(120, 60, true), Some(60));
    }
}
