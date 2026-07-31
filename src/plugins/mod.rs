pub mod archive;
pub mod jamendo;

#[cfg(feature = "nyaa")]
pub mod nyaa;

use std::path::Path;

/// What a plugin could read out of an audio file it fetched.
///
/// Online plugins hand the player files they did not index, so without this
/// every track would arrive with a filename for a title and a length of zero —
/// and a zero length means "unknown" to the player, which leaves the seek bar
/// dead and the total time reading 0:00.
#[derive(Debug, Default, Clone)]
pub struct ProbedTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track: Option<u32>,
    pub year: Option<u32>,
    /// Seconds, or 0 when the file could not be read.
    pub duration: u32,
    pub bit_rate: u32,
}

/// Read length and tags from a file on disk.
///
/// Returns defaults rather than failing: a file lofty cannot parse may still
/// be perfectly playable, and losing the whole track over a missing tag would
/// be a worse outcome than showing an unknown length.
pub fn probe_audio(path: &Path) -> ProbedTags {
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::prelude::{Accessor, ItemKey};

    let Ok(tagged) = lofty::read_from_path(path) else {
        return ProbedTags::default();
    };

    let properties = tagged.properties();
    let mut probed = ProbedTags {
        duration: properties.duration().as_secs() as u32,
        bit_rate: properties.audio_bitrate().unwrap_or(0),
        ..ProbedTags::default()
    };

    if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        probed.title = tag.title().map(|t| t.to_string());
        probed.artist = tag.artist().map(|a| a.to_string());
        probed.album = tag.album().map(|a| a.to_string());
        probed.track = tag.track();
        probed.year = tag
            .get_string(ItemKey::Year)
            .or_else(|| tag.get_string(ItemKey::RecordingDate))
            .and_then(|raw| raw.get(..4).and_then(|y| y.parse().ok()));
    }

    probed
}

/// Sanitize string for safe use in file/directory names.
pub fn sanitize_filename(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | '?' | '%' | '*' | ':' | '|' | '"' | '<' | '>' => '_',
            c => c,
        })
        .collect();

    let mut cleaned = sanitized
        .replace("..", "_")
        .trim_matches(|c| c == '.' || c == ' ')
        .to_string();

    if cleaned.is_empty() {
        cleaned = "download".to_string();
    }

    if cleaned.len() > 200 {
        let mut end = 200;
        while end > 0 && !cleaned.is_char_boundary(end) {
            end -= 1;
        }
        cleaned.truncate(end);
        cleaned = cleaned.trim_end().to_string();
    }

    cleaned
}
