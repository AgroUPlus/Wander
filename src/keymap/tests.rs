use super::actions::*;
use super::defaults::*;
use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
