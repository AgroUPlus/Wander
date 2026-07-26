use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use super::glyphs::Icon;
use super::widgets::{Slider, truncate};
use super::{Hits, Region};
use crate::app::{App, format_duration};
use crate::player::queue::Repeat;
use crate::theme::Theme;

/// Total height of the player bar, borders included.
pub const HEIGHT: u16 = 4;

/// Both rows are laid out on the same grid, so the title starts exactly above
/// the elapsed time and the volume percentage ends exactly above the total
/// time. Widening one column without the other breaks that alignment.
///
/// Left gutter: the transport glyph on the track row, blank on the seek row.
const GUTTER: u16 = 2;
/// Right-hand readout columns: total time, and the volume percentage.
const READOUT: u16 = 8;

/// The single source of playback information.
///
/// M1 split this across a top header and a bottom seek bar, which showed the
/// elapsed/total time twice. Everything lives here now.
pub fn draw(frame: &mut Frame, area: Rect, app: &App, theme: &Theme, hits: &mut Hits) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border(false));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(inner);

    draw_track_row(frame, rows[0], app, theme, hits);
    draw_seek_row(frame, rows[1], app, theme, hits);
}

/// Where both sliders stop, leaving room for their readout.
///
/// Computed rather than left to the layout solver: with `Constraint`s the two
/// rows have different fixed content before the readout, so at narrow widths
/// the solver squeezes them differently and the columns drift apart. Plain
/// arithmetic keeps them aligned at every width.
fn slider_end(area: Rect) -> u16 {
    area.width.saturating_sub(READOUT)
}

fn slice(area: Rect, start: u16, end: u16) -> Rect {
    let start = start.min(area.width);
    let end = end.clamp(start, area.width);
    Rect {
        x: area.x + start,
        y: area.y,
        width: end - start,
        height: area.height,
    }
}

/// Columns of the track row: glyph, identity, repeat/shuffle, volume icon,
/// volume slider, percentage.
fn track_columns(area: Rect) -> [Rect; 6] {
    let end = slider_end(area);
    // The volume block claims a fixed slice at the end of the row; the track
    // identity takes whatever is left, and truncates when that is little.
    let volume_width = 14u16.min(end.saturating_sub(GUTTER));
    let volume_start = end.saturating_sub(volume_width);
    let modes_start = volume_start.saturating_sub(6);

    [
        slice(area, 0, GUTTER),
        slice(area, GUTTER, modes_start),
        slice(area, modes_start, volume_start),
        slice(area, volume_start, volume_start + 2),
        slice(area, volume_start + 2, end),
        slice(area, end, area.width),
    ]
}

/// How many characters each part of the track identity may use.
#[derive(Debug, PartialEq)]
struct IdentityWidths {
    title: usize,
    artist: usize,
    album: usize,
}

/// Separators between the three parts: `"  "` and `" · "`.
const IDENTITY_SEPARATORS: usize = 2 + 3;
/// Artist and album stop growing here even when the row is enormous, so the
/// eye still lands on the title first.
const ARTIST_CAP: usize = 25;
const ALBUM_CAP: usize = 25;
/// Floors kept while clawing back space, so a squeezed row degrades into
/// something still readable rather than into stray ellipses.
const MIN_TITLE: usize = 10;
const MIN_ARTIST: usize = 8;
const MIN_ALBUM: usize = 6;

/// Divide the identity column between title, artist and album.
///
/// The title gets whatever the others do not need, rather than a fixed share:
/// a short artist and album should leave the title the rest of the row, and the
/// old half-the-width rule cropped titles with the space sitting empty beside
/// them. When it genuinely does not fit, space is taken back from the album
/// first and the title last, since the title is what identifies the track.
fn identity_widths(
    available: usize,
    title: usize,
    artist: usize,
    album: usize,
    stars: usize,
) -> IdentityWidths {
    // Stars sit after the album with their own two-space gap.
    let overhead = IDENTITY_SEPARATORS + if stars > 0 { stars + 2 } else { 0 };
    let content = available.saturating_sub(overhead);

    let mut widths = IdentityWidths {
        title,
        artist: artist.min(ARTIST_CAP),
        album: album.min(ALBUM_CAP),
    };

    let wanted = widths.title + widths.artist + widths.album;
    if wanted <= content {
        return widths;
    }

    // Shrink towards the floors, least important first.
    let mut excess = wanted - content;
    let mut shrink = |field: &mut usize, floor: usize| {
        let give = (*field).saturating_sub(floor).min(excess);
        *field -= give;
        excess -= give;
    };
    shrink(&mut widths.album, MIN_ALBUM);
    shrink(&mut widths.artist, MIN_ARTIST);
    shrink(&mut widths.title, MIN_TITLE);

    if excess == 0 {
        return widths;
    }

    // Narrower than every floor combined: hand out what there is in priority
    // order and let the tail drop off entirely.
    let mut left = content;
    for field in [&mut widths.title, &mut widths.artist, &mut widths.album] {
        let take = (*field).min(left);
        *field = take;
        left -= take;
    }
    widths
}

/// Columns of the seek row: gutter, elapsed, slider, total.
fn seek_columns(area: Rect) -> [Rect; 4] {
    let end = slider_end(area);
    let elapsed_end = (GUTTER + READOUT).min(end);
    [
        slice(area, 0, GUTTER),
        slice(area, GUTTER, elapsed_end),
        slice(area, elapsed_end, end),
        slice(area, end, area.width),
    ]
}

/// Transport glyph, track identity, playback modes, and volume.
fn draw_track_row(frame: &mut Frame, area: Rect, app: &App, theme: &Theme, hits: &mut Hits) {
    let status = app.player.status();

    let columns = track_columns(area);

    // Transport state, also a click target for play/pause. Playback is already
    // obvious from the moving seek bar, so only the states that are not carry a
    // glyph — a permanent play arrow is noise.
    let glyphs = app.config.glyphs;
    let glyph = if status.buffering {
        glyphs.icon(Icon::Buffering)
    } else if status.current.is_none() {
        glyphs.icon(Icon::Stopped)
    } else if app.player.is_paused() {
        glyphs.icon(Icon::Paused)
    } else {
        " "
    };
    hits.push(columns[0], Region::PlayPause);
    frame.render_widget(Paragraph::new(glyph).style(theme.title()), columns[0]);

    // Track identity, or an error if something went wrong.
    match (status.error.as_deref(), status.current.as_ref()) {
        (Some(error), _) => {
            let line = Line::from(Span::styled(
                truncate(error, columns[1].width as usize),
                theme.error(),
            ));
            frame.render_widget(Paragraph::new(line), columns[1]);
        }
        (None, Some(song)) => {
            let stars = app.rating_stars(song);
            let widths = identity_widths(
                columns[1].width as usize,
                song.title.chars().count(),
                song.artist_or_unknown().chars().count(),
                song.album_or_unknown().chars().count(),
                stars.chars().count(),
            );
            let title = truncate(&song.title, widths.title);
            let artist = truncate(song.artist_or_unknown(), widths.artist);
            let album = truncate(song.album_or_unknown(), widths.album);

            let artist_rect = Rect {
                x: columns[1].x + title.chars().count() as u16 + 2,
                y: columns[1].y,
                width: artist.chars().count() as u16,
                height: 1,
            };
            let album_rect = Rect {
                x: artist_rect.x + artist_rect.width + 3,
                y: columns[1].y,
                width: album.chars().count() as u16,
                height: 1,
            };

            hits.push(artist_rect, Region::CurrentArtist);
            hits.push(album_rect, Region::CurrentAlbum);

            let mut spans = vec![
                Span::styled(title, theme.playing()),
                Span::styled("  ", theme.dim()),
                Span::styled(artist, theme.title()),
                Span::styled(" · ", theme.dim()),
                Span::styled(album, theme.dim()),
            ];
            // Only shown once rated, so an unrated library stays uncluttered.
            if !stars.is_empty() {
                spans.push(Span::styled("  ", theme.dim()));
                spans.push(Span::styled(stars, theme.title()));
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), columns[1]);
        }
        (None, None) => {
            frame.render_widget(
                Paragraph::new("Nothing playing").style(theme.dim()),
                columns[1],
            );
        }
    };

    // Repeat and shuffle indicators: lit when active, dimmed when not.
    let (repeat, shuffle) = {
        let queue = app.player.queue.lock().unwrap();
        (queue.repeat, queue.shuffle)
    };
    let repeat_glyph = match repeat {
        Repeat::Off => glyphs.icon(Icon::RepeatOff),
        Repeat::All => glyphs.icon(Icon::RepeatAll),
        Repeat::One => glyphs.icon(Icon::RepeatOne),
    };
    // Two separate buttons in one column: registering the whole column as
    // `Repeat` meant clicking the shuffle glyph cycled repeat instead.
    let modes = Layout::horizontal([Constraint::Length(3), Constraint::Min(1)]).split(columns[2]);
    hits.push(modes[0], Region::Repeat);
    hits.push(modes[1], Region::Shuffle);

    frame.render_widget(
        Paragraph::new(Span::styled(
            repeat_glyph,
            if repeat == Repeat::Off {
                theme.dim()
            } else {
                theme.title()
            },
        )),
        modes[0],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            glyphs.icon(Icon::Shuffle),
            if shuffle { theme.title() } else { theme.dim() },
        )),
        modes[1],
    );

    // Volume: icon, slider, percentage.
    let volume = if app.active_drag == Some(Region::Volume) {
        app.drag_ratio as f32
    } else {
        app.player.volume()
    };
    let icon = if volume <= 0.001 {
        glyphs.icon(Icon::VolumeMuted)
    } else if volume < 0.5 {
        glyphs.icon(Icon::VolumeLow)
    } else {
        glyphs.icon(Icon::VolumeHigh)
    };
    // The percentage sits at the very end of the row, exactly like the total
    // time on the seek row, so both sliders stop at the same screen column and
    // both readouts start at the same one.
    frame.render_widget(Paragraph::new(icon).style(theme.dim()), columns[3]);
    hits.push(columns[4], Region::Volume);
    frame.render_widget(
        Paragraph::new(
            Slider::new(
                volume as f64,
                theme.accent.0,
                theme.accent.0,
                theme.border.0,
            )
            .knob(None)
            .render(columns[4].width),
        ),
        columns[4],
    );
    frame.render_widget(
        Paragraph::new(format!(" {}%", (volume * 100.0).round() as u32)).style(theme.dim()),
        columns[5],
    );
}

/// Elapsed time, seek slider, total time — shown exactly once.
fn draw_seek_row(frame: &mut Frame, area: Rect, app: &App, theme: &Theme, hits: &mut Hits) {
    let status = app.player.status();
    let total = status.current.as_ref().map(|s| s.duration).unwrap_or(0);

    let (elapsed, ratio) = if app.active_drag == Some(Region::Seek) {
        let secs = total as f64 * app.drag_ratio;
        (std::time::Duration::from_secs_f64(secs), app.drag_ratio)
    } else {
        let el = app.player.elapsed();
        let r = if total > 0 {
            (el.as_secs_f64() / total as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (el, r)
    };

    // Mirrors the track row: a GUTTER-wide blank so the elapsed time starts on
    // the same column as the title, and a READOUT-wide tail for the total.
    let columns = seek_columns(area);

    frame.render_widget(
        Paragraph::new(format_duration(elapsed)).style(theme.dim()),
        columns[1],
    );

    hits.push(columns[2], Region::Seek);
    let slider = Slider::new(
        ratio,
        theme.progress.0,
        theme.current_track.0,
        theme.border.0,
    );
    // No padding: the slider fills its column exactly, so it ends on the same
    // screen column as the volume slider above it. `App::update_drag_ratio`
    // assumes this — a padded slider would make clicks land off-target.
    frame.render_widget(Paragraph::new(slider.render(columns[2].width)), columns[2]);

    frame.render_widget(
        Paragraph::new(format!(
            " {}",
            format_duration(std::time::Duration::from_secs(total as u64))
        ))
        .style(theme.dim()),
        columns[3],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this replaced: the title was given half the column no matter
    /// what, so it was cropped with the rest of the row sitting empty.
    #[test]
    fn a_short_artist_and_album_leave_the_whole_row_to_the_title() {
        let title = 60;
        let widths = identity_widths(100, title, 3, 2, 0);
        assert_eq!(
            widths.title, title,
            "the title should not be cropped at all"
        );
        assert_eq!(widths.artist, 3);
        assert_eq!(widths.album, 2);
    }

    #[test]
    fn nothing_is_truncated_when_everything_fits() {
        let widths = identity_widths(120, 30, 12, 14, 0);
        assert_eq!(
            widths,
            IdentityWidths {
                title: 30,
                artist: 12,
                album: 14
            }
        );
    }

    #[test]
    fn the_parts_always_fit_the_column_at_every_width() {
        for available in 0usize..200 {
            for stars in [0usize, 5] {
                let w = identity_widths(available, 80, 40, 40, stars);
                let overhead = IDENTITY_SEPARATORS + if stars > 0 { stars + 2 } else { 0 };
                let used = w.title + w.artist + w.album + overhead;
                assert!(
                    used <= available.max(overhead),
                    "overflowed at {available} (stars {stars}): {w:?} used {used}"
                );
            }
        }
    }

    #[test]
    fn a_squeezed_row_takes_from_the_album_before_the_title() {
        // Everything wants more than there is; the title must survive best.
        let wide = identity_widths(200, 50, 30, 30, 0);
        let narrow = identity_widths(60, 50, 30, 30, 0);
        assert!(narrow.album < wide.album, "album should give way first");
        assert!(narrow.title >= MIN_TITLE, "the title must stay readable");
        assert!(narrow.title > narrow.album);
    }

    #[test]
    fn stars_are_reserved_so_they_are_never_pushed_off_the_row() {
        let without = identity_widths(80, 60, 20, 20, 0);
        let with = identity_widths(80, 60, 20, 20, 5);
        let total = |w: &IdentityWidths| w.title + w.artist + w.album;
        assert_eq!(
            total(&without) - total(&with),
            7,
            "5 stars plus a 2-space gap"
        );
    }

    /// The two rows must read as one grid: sliders ending together, readouts
    /// starting together. This is easy to break by nudging one constraint.
    #[test]
    fn both_sliders_end_on_the_same_column_at_every_width() {
        for width in [20u16, 30, 40, 60, 80, 120, 200] {
            let area = Rect::new(0, 0, width, 1);
            let track = track_columns(area);
            let seek = seek_columns(area);

            let volume_end = track[4].x + track[4].width;
            let seek_end = seek[2].x + seek[2].width;
            assert_eq!(volume_end, seek_end, "sliders disagree at width {width}");
            assert_eq!(
                track[5].x, seek[3].x,
                "percentage and total time disagree at width {width}"
            );
        }
    }

    /// Repeat and shuffle share a column but are separate buttons; a single
    /// hit region over both made the shuffle glyph toggle repeat.
    #[test]
    fn repeat_and_shuffle_occupy_disjoint_hit_areas() {
        let area = Rect::new(0, 0, 100, 1);
        let modes = track_columns(area)[2];
        let split = Layout::horizontal([Constraint::Length(3), Constraint::Min(1)]).split(modes);

        assert!(split[0].width > 0 && split[1].width > 0);
        assert_eq!(
            split[0].x + split[0].width,
            split[1].x,
            "no gap, no overlap"
        );
        assert_eq!(split[1].x + split[1].width, modes.x + modes.width);
    }

    #[test]
    fn the_title_starts_above_the_elapsed_time() {
        let area = Rect::new(0, 0, 100, 1);
        assert_eq!(track_columns(area)[1].x, seek_columns(area)[1].x);
    }
}
