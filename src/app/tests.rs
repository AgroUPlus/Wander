use super::features::*;
use super::layout::*;
use super::navigation::*;
use super::storage::*;
use super::types::*;
use super::*;
use crate::subsonic::models::Song;
use std::time::Duration;

use super::*;

fn shape() -> FrameShape {
    FrameShape {
        overlay: None,
        show_help: false,
        focus_mode: false,
        panes: [true, true, false, true, true],
        panes_sizes: [30, 30, 8],
        cover: 0,
        tab: 0,
        viz_mode: crate::ui::visualiser::VizMode::Aurora,
    }
}

/// Each of these is a case where a popup or a moving pane can leave debris
/// over the artwork, so each must be visible as a change of shape.
#[test]
fn everything_that_can_cover_the_artwork_changes_the_frame_shape() {
    let base = shape();
    assert_eq!(base, shape(), "an unchanged frame must not force a repaint");

    let opened = FrameShape {
        overlay: Some(0),
        ..base
    };
    assert_ne!(base, opened, "opening a popup");
    assert_ne!(
        opened,
        FrameShape {
            overlay: Some(1),
            ..base
        },
        "swapping one popup for another"
    );
    assert_ne!(
        base,
        FrameShape {
            show_help: true,
            ..base
        },
        "opening the help sheet"
    );
    assert_ne!(
        base,
        FrameShape {
            panes_sizes: [31, 30, 8],
            ..base
        },
        "a pane mid-glide"
    );
    assert_ne!(base, FrameShape { cover: 1, ..base }, "new artwork");
    for changed in [
        FrameShape {
            focus_mode: true,
            ..base
        },
        FrameShape {
            panes: [false, true, false, true, true],
            ..base
        },
        FrameShape { tab: 1, ..base },
    ] {
        assert_ne!(base, changed, "{changed:?} should force a repaint");
    }
}

/// The Up Next pane used to be reachable only by clicking it: it was drawn
/// beside every tab but was in no tab's focus order.
#[test]
fn the_side_queue_is_the_last_stop_when_cycling_right() {
    for tab in [Tab::Home, Tab::Library, Tab::Settings] {
        let panes = focus_order(tab, LibraryMode::Artists, true);
        assert_eq!(
            panes.last(),
            Some(&Pane::Queue),
            "{tab:?} should end at the Up Next pane"
        );
        assert!(
            panes.len() > 1,
            "{tab:?} should have somewhere to come back to"
        );
    }
}

#[test]
fn a_hidden_side_queue_is_not_focusable() {
    for tab in [Tab::Home, Tab::Library, Tab::Settings] {
        assert!(
            !focus_order(tab, LibraryMode::Artists, false).contains(&Pane::Queue),
            "{tab:?} must not focus a pane that is not drawn"
        );
    }
}

/// The Queue tab draws the queue as its content, so the side pane is
/// suppressed there and must not appear twice in the focus order.
#[test]
fn the_queue_tab_lists_the_queue_once() {
    let panes = focus_order(Tab::Queue, LibraryMode::Artists, false);
    assert_eq!(panes, vec![Pane::Queue]);
}

/// Every Library view keeps its own panes and gains the queue at the end.
#[test]
fn library_views_keep_their_panes_and_gain_the_queue() {
    let modes = [
        LibraryMode::Artists,
        LibraryMode::Albums,
        LibraryMode::Tracks,
        LibraryMode::Playlists,
        LibraryMode::Favorites,
    ];
    for mode in modes {
        let without = focus_order(Tab::Library, mode, false);
        let with = focus_order(Tab::Library, mode, true);
        assert_eq!(without, mode.panes().to_vec());
        assert_eq!(with[..without.len()], without[..]);
        assert_eq!(with.last(), Some(&Pane::Queue));
    }
}

#[test]
fn formats_short_and_long_durations() {
    assert_eq!(format_duration(Duration::from_secs(0)), "0:00");
    assert_eq!(format_duration(Duration::from_secs(75)), "1:15");
    assert_eq!(format_duration(Duration::from_secs(255)), "4:15");
    assert_eq!(format_duration(Duration::from_secs(3661)), "1:01:01");
}

#[test]
fn selection_stays_within_bounds() {
    let mut sel = Selection::default();
    sel.move_by(5, 3);
    assert_eq!(sel.index, 2, "clamps to the last item");
    sel.move_by(-10, 3);
    assert_eq!(sel.index, 0, "clamps to the first item");
}

#[test]
fn selection_on_an_empty_list_stays_at_zero() {
    let mut sel = Selection::default();
    sel.move_by(3, 0);
    assert_eq!(sel.index, 0);
}

#[test]
fn clamp_pulls_selection_back_when_the_list_shrinks() {
    let mut sel = Selection {
        index: 9,
        offset: 0,
    };
    sel.clamp(4);
    assert_eq!(sel.index, 3);
    sel.clamp(0);
    assert_eq!(sel.index, 0);
}

#[test]
fn every_library_mode_declares_at_least_one_pane() {
    for mode in LibraryMode::ALL {
        assert!(!mode.panes().is_empty(), "{} has no panes", mode.title());
    }
}

/// Lyrics lost their own tab, so focus mode is the only place they can hold
/// the cursor — which is what makes hand-scrolling unsynced lyrics
/// reachable. If they ever reappear in a browsable pane list, revisit
/// `set_focus_mode`, which currently assumes it is the sole route.
#[test]
fn lyrics_are_not_a_browsable_pane() {
    for mode in LibraryMode::ALL {
        assert!(
            !mode.panes().contains(&Pane::Lyrics),
            "{} lists the lyrics pane",
            mode.title()
        );
    }
}

#[test]
fn a_rating_set_this_session_wins_over_the_servers_copy() {
    assert_eq!(merge_rating(Some(4), Some(2)), 4);
    assert_eq!(
        merge_rating(None, Some(2)),
        2,
        "server value when unset here"
    );
    assert_eq!(merge_rating(Some(0), Some(5)), 0, "clearing must stick");
    assert_eq!(merge_rating(None, None), 0);
}

#[test]
fn a_nonsense_rating_from_the_server_cannot_overflow_the_stars() {
    // Stars are rendered by repeating a glyph `rating` times.
    assert_eq!(merge_rating(None, Some(99)), 5);
}

/// The side panes are drawn on the right, so widening them moves the
/// divider left. Pressing `M-→` must therefore *narrow* them, or the
/// boundary travels opposite to the arrow.
#[test]
fn the_divider_follows_the_arrow_key() {
    let start = 25;
    // M-← widens the side panes, pushing the divider left.
    assert!(step_percent(start, 1) > start);
    // M-→ narrows them, letting the content take the space back.
    assert!(step_percent(start, -1) < start);
}

#[test]
fn resizing_stops_at_the_bounds() {
    let mut narrow = PANE_MIN_PERCENT;
    for _ in 0..20 {
        narrow = step_percent(narrow, -1);
    }
    assert_eq!(narrow, PANE_MIN_PERCENT, "a pane must not vanish");

    let mut wide = PANE_MAX_PERCENT;
    for _ in 0..20 {
        wide = step_percent(wide, 1);
    }
    assert_eq!(wide, PANE_MAX_PERCENT, "a pane must not swallow the screen");
}

#[test]
fn base64_matches_the_reference_encoding() {
    // Padding is the part that is easy to get wrong, so cover all three
    // input lengths mod 3.
    assert_eq!(base64(b"a"), "YQ==");
    assert_eq!(base64(b"ab"), "YWI=");
    assert_eq!(base64(b"abc"), "YWJj");
    assert_eq!(
        base64(b"https://music.example.com/share/AbC1"),
        "aHR0cHM6Ly9tdXNpYy5leGFtcGxlLmNvbS9zaGFyZS9BYkMx"
    );
}

#[test]
fn hit_map_returns_the_topmost_region() {
    let mut hits = Hits::default();
    let area = ratatui::layout::Rect::new(0, 0, 10, 2);
    hits.push(area, Region::Seek);
    hits.push(area, Region::Volume);
    // Later pushes win, so overlays take precedence over what is beneath.
    assert_eq!(hits.at(5, 1), Some(Region::Volume));
    assert_eq!(hits.at(50, 50), None);
}

#[test]
fn hit_map_finds_a_regions_rect() {
    let mut hits = Hits::default();
    let area = ratatui::layout::Rect::new(3, 4, 10, 1);
    hits.push(area, Region::Seek);
    assert_eq!(hits.rect_of(Region::Seek), Some(area));
    assert_eq!(hits.rect_of(Region::Volume), None);
}

#[test]
fn visualiser_height_resizes_and_clamps() {
    let mut height = 8u16;
    height = (height as i16 + 2).clamp(3, 40) as u16;
    assert_eq!(height, 10);
    height = (height as i16 - 2).clamp(3, 40) as u16;
    assert_eq!(height, 8);
}

#[test]
fn cover_click_registers_region() {
    let mut hits = Hits::default();
    let area = ratatui::layout::Rect::new(0, 0, 20, 20);
    hits.push(area, Region::Cover);
    assert_eq!(hits.at(10, 10), Some(Region::Cover));
    assert_eq!(hits.rect_of(Region::Cover), Some(area));
}
