pub mod ui;

pub mod api;

#[allow(unused_imports)]
pub use api::{JamendoFormat, JamendoTrack};

use crate::app::Selection;

#[derive(Debug, Default, Clone)]
pub struct JamendoPluginState {
    pub query: String,
    pub query_input: crate::ui::widgets::TextInput,
    pub editing_query: bool,
    pub searching: bool,
    pub working: bool,
    pub results: Vec<JamendoTrack>,
    pub selection: Selection,
}

impl JamendoPluginState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn selected_track(&self) -> Option<&JamendoTrack> {
        if self.results.is_empty() {
            None
        } else {
            let idx = self.selection.index.min(self.results.len() - 1);
            self.results.get(idx)
        }
    }
}
