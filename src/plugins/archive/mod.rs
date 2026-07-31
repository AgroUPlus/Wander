pub mod api;
pub mod downloader;
pub mod ui;

#[allow(unused_imports)]
pub use api::{ArchiveCollection, ArchiveFile, ArchiveItem};
#[allow(unused_imports)]
pub use downloader::download_archive_item;
#[allow(unused_imports)]
pub use ui::draw;

use crate::app::Selection;

#[derive(Debug, Default, Clone)]
pub struct ArchivePluginState {
    pub query: String,
    pub query_input: crate::ui::widgets::TextInput,
    pub editing_query: bool,
    pub searching: bool,
    pub working: bool,
    pub results: Vec<ArchiveItem>,
    pub selection: Selection,
    /// Track lists, keyed by item identifier.
    ///
    /// The search endpoint cannot report an item's length — that only comes
    /// from its metadata — so the highlighted row's metadata is fetched in the
    /// background to fill the Length column. The same entry is then reused
    /// when the item is played, so pressing Enter costs no extra round trip.
    /// `None` records an item whose metadata could not be read, which stops it
    /// being requested forever.
    pub files: std::collections::HashMap<String, Option<Vec<ArchiveFile>>>,
    /// Identifiers with a metadata request already in flight.
    pub pending: std::collections::HashSet<String>,
}

impl ArchivePluginState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Total runtime of an item, once its metadata has arrived.
    pub fn total_duration(&self, identifier: &str) -> Option<u64> {
        let files = self.files.get(identifier)?.as_ref()?;
        let total: u64 = files.iter().map(|file| file.duration).sum();
        (total > 0).then_some(total)
    }

    pub fn selected_item(&self) -> Option<&ArchiveItem> {
        if self.results.is_empty() {
            None
        } else {
            let idx = self.selection.index.min(self.results.len() - 1);
            self.results.get(idx)
        }
    }
}
