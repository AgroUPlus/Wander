use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

pub mod actions;
pub(crate) mod defaults;
#[cfg(test)]
mod tests;

pub use actions::*;
pub(crate) use defaults::*;

pub struct Keymap {
    bindings: HashMap<(KeyCode, KeyModifiers), Action>,
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
