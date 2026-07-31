//! The Home tab: listening statistics, and one-press mixes.
//!
//! Stats come from the local play log (`crate::history`) because Subsonic only
//! exposes aggregate play counts. Mixes are seeds for radio mode rather than
//! fixed playlists, so they never run out.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Sparkline};

use super::glyphs::{GlyphSet, Icon};
use super::widgets::{gradient, truncate};
use super::{Hits, Region};
use crate::app::{App, Pane};
use crate::history::Stats;
use crate::theme::Theme;

/// How a mix chooses its seed tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MixKind {
    /// Most-played genres from the local history.
    Genre(String),
    /// Tracks the user has never played.
    Discover,
    /// Starred tracks.
    Favorites,
    /// Anything, weighted by what is already familiar.
    Surprise,
}

#[derive(Debug, Clone)]
pub struct Mix {
    pub name: String,
    pub kind: MixKind,
}

/// The mixes on offer, derived from the history so they follow actual taste.
///
/// Always includes the three non-genre mixes, so a brand-new install still has
/// something to press.
pub fn mixes(app: &App) -> Vec<Mix> {
    let mut genres = crate::history::mix_genres(&app.stats, 4);
    if genres.is_empty() {
        genres = app.library_genres.iter().take(4).cloned().collect();
    }
    let mut mixes: Vec<Mix> = genres
        .into_iter()
        .map(|genre| Mix {
            name: format!("{} Mix", title_case(&genre)),
            kind: MixKind::Genre(genre),
        })
        .collect();
    mixes.push(Mix {
        name: "Discover".to_string(),
        kind: MixKind::Discover,
    });
    mixes.push(Mix {
        name: "Favorites".to_string(),
        kind: MixKind::Favorites,
    });
    mixes.push(Mix {
        name: "Surprise Me".to_string(),
        kind: MixKind::Surprise,
    });
    mixes
}

/// Selectable item count for the Home pane.
pub fn mix_count(app: &App) -> usize {
    mixes(app).len()
}

fn title_case(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

pub fn draw(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme, hits: &mut Hits) {
    let rows = Layout::vertical([
        Constraint::Length(4), // mixes (2 lines of content + borders)
        Constraint::Length(9), // summary + top lists
        Constraint::Min(6),    // charts
    ])
    .split(area);

    // Includes the track playing right now, so the numbers move as you listen.
    let stats = app.live_stats();

    draw_mixes(frame, rows[0], app, theme, hits);
    draw_stats(frame, rows[1], &stats, theme, app.config.glyphs);
    draw_charts(frame, rows[2], &stats, theme);
}

/// The sparkline, heatmap and 24-hour clock, side by side.
fn draw_charts(frame: &mut Frame, area: Rect, stats: &Stats, theme: &Theme) {
    if area.height < 4 {
        return;
    }
    let columns = Layout::horizontal([
        Constraint::Percentage(34),
        Constraint::Percentage(33),
        Constraint::Percentage(33),
    ])
    .split(area);

    draw_sparkline(frame, columns[0], stats, theme);
    draw_heatmap(frame, columns[1], stats, theme);
    draw_clock(frame, columns[2], stats, theme);
}

/// Minutes per day over the last two weeks.
fn draw_sparkline(frame: &mut Frame, area: Rect, stats: &Stats, theme: &Theme) {
    let outer = block("Last 14 days", false, theme);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    if inner.height == 0 {
        return;
    }

    let minutes: Vec<u64> = stats.by_day.iter().map(|secs| secs / 60).collect();
    let split = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);

    frame.render_widget(
        Sparkline::default()
            .data(&minutes)
            .style(Style::default().fg(theme.accent.0)),
        split[0],
    );

    // Second week against the first, which is the comparison that tells you
    // something a bare total does not.
    let half = minutes.len() / 2;
    let (earlier, recent): (u64, u64) =
        (minutes[..half].iter().sum(), minutes[half..].iter().sum());
    let (glyph, style) = match recent.cmp(&earlier) {
        std::cmp::Ordering::Greater => ("▲", theme.playing()),
        std::cmp::Ordering::Less => ("▼", theme.dim()),
        std::cmp::Ordering::Equal => ("=", theme.dim()),
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{recent} min this week  "), theme.base()),
            Span::styled(format!("{glyph} vs {earlier}"), style),
        ])),
        split[1],
    );
}

/// Eight weeks of daily listening as an intensity grid.
fn draw_heatmap(frame: &mut Frame, area: Rect, stats: &Stats, theme: &Theme) {
    let outer = block("Last 8 weeks", false, theme);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    if inner.height < 7 || inner.width < 8 {
        return;
    }

    let peak = stats.heatmap.iter().copied().max().unwrap_or(0).max(1);
    let weeks = stats.heatmap.len() / 7;

    // Seven rows (one per weekday slot), one column per week.
    let mut lines = Vec::with_capacity(7);
    for day in 0..7 {
        let mut spans = Vec::with_capacity(weeks);
        for week in 0..weeks {
            let secs = stats.heatmap[week * 7 + day];
            let span = if secs == 0 {
                Span::styled("·", theme.dim())
            } else {
                let t = (secs as f64 / peak as f64).clamp(0.0, 1.0);
                Span::styled(
                    "█",
                    Style::default().fg(gradient(theme.viz_low.0, theme.viz_high.0, t)),
                )
            };
            spans.push(span);
            spans.push(Span::raw(" "));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(Span::styled(
        format!("peak {} min/day", peak / 60),
        theme.dim(),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Which hours of the day get listened to.
fn draw_clock(frame: &mut Frame, area: Rect, stats: &Stats, theme: &Theme) {
    let outer = block("By hour", false, theme);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    if inner.height < 3 {
        return;
    }

    let peak = stats.by_hour.iter().copied().max().unwrap_or(0).max(1);
    // Vertical eighths so a short hour still reads as more than nothing.
    const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let bars: String = stats
        .by_hour
        .iter()
        .map(|secs| {
            let level = (*secs as f64 / peak as f64 * 8.0).round() as usize;
            BLOCKS[level.min(8)]
        })
        .collect();

    let busiest = stats
        .by_hour
        .iter()
        .enumerate()
        .max_by_key(|(_, secs)| **secs)
        .map(|(hour, _)| hour)
        .unwrap_or(0);

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(bars, Style::default().fg(theme.accent.0))),
            Line::from(Span::styled("0        8       16      23", theme.dim())),
            Line::from(Span::styled(
                if stats.plays_total == 0 {
                    "No plays yet".to_string()
                } else {
                    format!("busiest around {busiest:02}:00 UTC")
                },
                theme.dim(),
            )),
        ]),
        inner,
    );
}

fn block(title: &str, focused: bool, theme: &Theme) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border(focused))
        .style(theme.base())
        .title(format!(" {title} "))
        .title_style(if focused { theme.title() } else { theme.dim() })
}

fn draw_mixes(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme, hits: &mut Hits) {
    let focused = app.focus == Pane::Home;
    let mixes = mixes(app);
    app.home_sel.clamp(mixes.len());

    let outer = block("Made for you", focused, theme);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    if inner.width == 0 || mixes.is_empty() {
        return;
    }

    // One equal-width card per mix, so the row fills the pane at any size.
    let share = 100 / mixes.len() as u16;
    let columns = Layout::horizontal(
        mixes
            .iter()
            .map(|_| Constraint::Percentage(share))
            .collect::<Vec<_>>(),
    )
    .split(inner);

    for (index, (mix, column)) in mixes.iter().zip(columns.iter()).enumerate() {
        let selected = index == app.home_sel.index;
        let style = if selected && focused {
            theme.selected()
        } else if selected {
            theme.playing()
        } else {
            theme.base()
        };
        hits.push(
            *column,
            Region::Row {
                pane: Pane::Home,
                index,
            },
        );

        let glyph = app.config.glyphs.icon(match mix.kind {
            MixKind::Genre(_) => Icon::Song,
            MixKind::Discover => Icon::Discover,
            MixKind::Favorites => Icon::Favorite,
            MixKind::Surprise => Icon::Surprise,
        });
        let width = column.width.saturating_sub(2) as usize;
        let lines = vec![
            Line::from(Span::styled(format!(" {glyph}"), theme.title())),
            Line::from(Span::styled(
                format!(" {}", truncate(&mix.name, width)),
                style,
            )),
        ];
        frame.render_widget(Paragraph::new(lines), *column);
    }
}

fn draw_stats(frame: &mut Frame, area: Rect, stats: &Stats, theme: &Theme, glyphs: GlyphSet) {
    let outer = block("Your listening", false, theme);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let columns = Layout::horizontal([
        Constraint::Percentage(25),
        Constraint::Percentage(25),
        Constraint::Percentage(25),
        Constraint::Percentage(25),
    ])
    .split(inner);

    let summary = vec![
        Line::from(Span::styled("Last 24 h", theme.dim())),
        Line::from(Span::styled(hours(stats.secs_today), theme.playing())),
        Line::default(),
        Line::from(Span::styled("Last 7 days", theme.dim())),
        Line::from(Span::styled(hours(stats.secs_week), theme.playing())),
        Line::default(),
        Line::from(Span::styled("All time", theme.dim())),
        Line::from(Span::styled(
            format!(
                "{}  ·  {} plays",
                hours(stats.secs_total),
                stats.plays_total
            ),
            theme.base(),
        )),
        Line::default(),
        Line::from(Span::styled(
            format!("{} {} day streak", glyphs.icon(Icon::Streak), stats.streak),
            theme.title(),
        )),
    ];
    frame.render_widget(Paragraph::new(summary), columns[0]);

    frame.render_widget(
        Paragraph::new(top_list(
            "Top artists",
            &stats.top_artists,
            theme,
            columns[1].width,
        )),
        columns[1],
    );
    frame.render_widget(
        Paragraph::new(top_list(
            "Top albums",
            &stats.top_albums,
            theme,
            columns[2].width,
        )),
        columns[2],
    );
    frame.render_widget(
        Paragraph::new(top_list(
            "Top genres",
            &stats.top_genres,
            theme,
            columns[3].width,
        )),
        columns[3],
    );
}

/// A ranked list drawn as proportional bars: relative size is the point, and a
/// bar shows it at a glance where a column of numbers does not.
fn top_list(
    title: &str,
    entries: &[(String, u32)],
    theme: &Theme,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(title.to_string(), theme.dim())),
        Line::default(),
    ];
    if entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "Nothing played yet".to_string(),
            theme.dim(),
        )));
        return lines;
    }

    let peak = entries
        .iter()
        .map(|(_, count)| *count)
        .max()
        .unwrap_or(1)
        .max(1);
    // Name, bar and count share the width; the name gets what is left.
    let bar_width = (width / 4).clamp(4, 12);
    let name_width = width.saturating_sub(bar_width + 6) as usize;

    for (index, (name, count)) in entries.iter().enumerate() {
        let filled = (*count as f32 / peak as f32 * bar_width as f32).round() as usize;
        let colour = gradient(
            theme.viz_high.0,
            theme.viz_low.0,
            index as f64 / entries.len().max(1) as f64,
        );
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<name_width$} ", truncate(name, name_width)),
                theme.base(),
            ),
            Span::styled("█".repeat(filled), Style::default().fg(colour)),
            Span::styled(
                "░".repeat(bar_width as usize - filled),
                Style::default().fg(theme.border.0),
            ),
            Span::styled(format!("{count:>4}"), theme.dim()),
        ]));
    }
    lines
}

fn hours(secs: u64) -> String {
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    if h > 0 {
        format!("{h} h {m:02} min")
    } else {
        format!("{m} min")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_listening_time_readably() {
        assert_eq!(hours(0), "0 min");
        assert_eq!(hours(90), "1 min");
        assert_eq!(hours(3_600), "1 h 00 min");
        assert_eq!(hours(7_950), "2 h 12 min");
    }

    #[test]
    fn a_top_list_reports_emptiness_rather_than_rendering_nothing() {
        let theme = Theme::default();
        let lines = top_list("Top artists", &[], &theme, 30);
        assert_eq!(lines.len(), 3, "title, blank, and the empty notice");
    }

    #[test]
    fn top_list_bars_stay_inside_the_pane_at_any_width() {
        let theme = Theme::default();
        let entries = vec![
            ("a very long artist name indeed".to_string(), 40u32),
            ("b".to_string(), 1),
        ];
        for width in [12u16, 20, 40, 80] {
            for line in top_list("Top artists", &entries, &theme, width)
                .iter()
                .skip(2)
            {
                assert!(
                    line.width() <= width as usize,
                    "line overflowed at width {width}: {}",
                    line.width()
                );
            }
        }
    }
}
