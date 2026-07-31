use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

const USER_AGENT: &str = "wander-tui/0.1 (https://archive.org; music player)";

/// One audio item (an "item" in Internet Archive terms — usually a whole
/// album, concert or record) returned by the search endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveItem {
    pub identifier: String,
    pub title: String,
    pub creator: String,
    pub year: String,
    pub downloads: u64,
    /// Collection the item lives in, e.g. `etree` for the Live Music Archive.
    pub collection: String,
}

impl ArchiveItem {
    #[allow(dead_code)]
    pub fn details_url(&self) -> String {
        format!("https://archive.org/details/{}", self.identifier)
    }
}

/// A single playable file inside an item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveFile {
    /// Path of the file within the item, e.g. `gd77-05-08d1t01.flac`.
    pub name: String,
    pub title: String,
    /// Archive's format label, e.g. `Flac`, `VBR MP3`, `Ogg Vorbis`.
    pub format: String,
    pub track: Option<u32>,
    /// Length in whole seconds, when the item reports one.
    pub duration: u64,
    pub size: u64,
}

impl ArchiveFile {
    /// Direct HTTPS URL the player can stream from — Archive serves plain
    /// range-capable downloads, so no local copy is needed to play a track.
    pub fn stream_url(&self, identifier: &str) -> String {
        let encoded: Vec<String> = self
            .name
            .split('/')
            .map(|segment| urlencoding::encode(segment).into_owned())
            .collect();
        format!(
            "https://archive.org/download/{}/{}",
            urlencoding::encode(identifier),
            encoded.join("/")
        )
    }

    pub fn suffix(&self) -> Option<String> {
        self.name
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_lowercase())
    }
}

/// Audio formats we can hand to the decoder, best first. Preference order is
/// the point of the list: FLAC wins whenever an item offers it.
const FORMAT_RANK: [&str; 7] = ["flac", "wav", "aiff", "m4a", "ogg", "opus", "mp3"];

fn format_rank(file: &ArchiveFile) -> Option<usize> {
    let ext = file.suffix()?;
    FORMAT_RANK.iter().position(|candidate| *candidate == ext)
}

/// Which slice of Archive's audio to search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArchiveCollection {
    AllAudio,
    LiveMusic,
    Netlabels,
    Album78s,
}

impl ArchiveCollection {
    pub const ALL: [ArchiveCollection; 4] = [
        ArchiveCollection::AllAudio,
        ArchiveCollection::LiveMusic,
        ArchiveCollection::Netlabels,
        ArchiveCollection::Album78s,
    ];

    /// Value stored in the config file.
    pub fn code(self) -> &'static str {
        match self {
            ArchiveCollection::AllAudio => "audio",
            ArchiveCollection::LiveMusic => "etree",
            ArchiveCollection::Netlabels => "netlabels",
            ArchiveCollection::Album78s => "78rpm",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ArchiveCollection::AllAudio => "All Audio",
            ArchiveCollection::LiveMusic => "Live Music",
            ArchiveCollection::Netlabels => "Netlabels",
            ArchiveCollection::Album78s => "78 RPM & Cylinders",
        }
    }

    pub fn from_code(code: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|c| c.code() == code)
            .unwrap_or(ArchiveCollection::AllAudio)
    }

    /// Lucene clause appended to the user's query.
    ///
    /// Exactly one clause, never two: archive.org's search returns zero hits
    /// for a query that ANDs two field clauses together (`mediatype:(audio)
    /// AND collection:(etree)` matches nothing), even though either alone
    /// works. The named collections are audio-only anyway, so the mediatype
    /// filter is only needed for the unrestricted search.
    fn filter(self) -> String {
        match self {
            ArchiveCollection::AllAudio => "mediatype:(audio)".to_string(),
            other => format!("collection:({})", other.code()),
        }
    }
}

pub async fn search_archive(
    client: &Client,
    query: &str,
    collection: ArchiveCollection,
) -> Result<Vec<ArchiveItem>> {
    let full_query = format!("({}) AND {}", query.trim(), collection.filter());
    let url = format!(
        "https://archive.org/advancedsearch.php?q={}&fl%5B%5D=identifier&fl%5B%5D=title&fl%5B%5D=creator&fl%5B%5D=year&fl%5B%5D=downloads&fl%5B%5D=collection&sort%5B%5D=downloads+desc&rows=75&page=1&output=json",
        urlencoding::encode(&full_query)
    );

    let response = client
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .context("Failed to reach archive.org search")?;

    if !response.status().is_success() {
        anyhow::bail!("archive.org returned status code {}", response.status());
    }

    let body: serde_json::Value = response
        .json()
        .await
        .context("Failed to parse archive.org search response")?;

    parse_search_response(&body)
}

pub fn parse_search_response(body: &serde_json::Value) -> Result<Vec<ArchiveItem>> {
    let docs = body
        .get("response")
        .and_then(|r| r.get("docs"))
        .and_then(|d| d.as_array())
        .context("archive.org search response had no result documents")?;

    let mut items = Vec::with_capacity(docs.len());
    for doc in docs {
        let Some(identifier) = doc.get("identifier").and_then(|v| v.as_str()) else {
            continue;
        };
        items.push(ArchiveItem {
            identifier: identifier.to_string(),
            title: doc
                .get("title")
                .map(flatten_field)
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| identifier.to_string()),
            creator: doc.get("creator").map(flatten_field).unwrap_or_default(),
            year: doc.get("year").map(flatten_field).unwrap_or_default(),
            downloads: doc
                .get("downloads")
                .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
                .unwrap_or(0),
            collection: doc.get("collection").map(flatten_field).unwrap_or_default(),
        });
    }

    Ok(items)
}

/// Archive metadata fields are inconsistently either a string or an array of
/// strings, so both shapes collapse to one display string.
fn flatten_field(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(values) => values
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// Fetch an item's playable files, best format first.
///
/// Archive usually stores several derivatives of the same recording (FLAC plus
/// an MP3 and an Ogg transcode). Returning all of them would queue every track
/// three times, so only the best available format is kept.
pub async fn item_files(client: &Client, identifier: &str) -> Result<Vec<ArchiveFile>> {
    let url = format!(
        "https://archive.org/metadata/{}",
        urlencoding::encode(identifier)
    );

    let response = client
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .with_context(|| format!("Failed to fetch archive.org metadata for {identifier}"))?;

    if !response.status().is_success() {
        anyhow::bail!(
            "archive.org metadata returned status code {}",
            response.status()
        );
    }

    let body: serde_json::Value = response
        .json()
        .await
        .context("Failed to parse archive.org metadata response")?;

    let files = parse_metadata_files(&body);
    if files.is_empty() {
        anyhow::bail!("No playable audio files in this item");
    }
    Ok(files)
}

pub fn parse_metadata_files(body: &serde_json::Value) -> Vec<ArchiveFile> {
    let Some(entries) = body.get("files").and_then(|f| f.as_array()) else {
        return Vec::new();
    };

    // Archive's derivatives share a stem (`t01.flac`, `t01.mp3`), and only
    // some of them carry a `length`. Collecting the known lengths first lets a
    // file that omits its own borrow from a sibling, which is the difference
    // between a real duration and a dead seek bar.
    let mut length_by_stem: std::collections::HashMap<String, u64> =
        std::collections::HashMap::new();
    for entry in entries {
        let (Some(name), Some(length)) = (
            entry.get("name").and_then(|v| v.as_str()),
            entry.get("length").and_then(|v| v.as_str()).map(parse_length),
        ) else {
            continue;
        };
        if length > 0 {
            length_by_stem.entry(file_stem(name).to_string()).or_insert(length);
        }
    }

    let mut audio: Vec<ArchiveFile> = Vec::new();
    for entry in entries {
        let Some(name) = entry.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let file = ArchiveFile {
            name: name.to_string(),
            title: entry
                .get("title")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    name.rsplit('/')
                        .next()
                        .unwrap_or(name)
                        .rsplit_once('.')
                        .map(|(stem, _)| stem.to_string())
                        .unwrap_or_else(|| name.to_string())
                }),
            format: entry
                .get("format")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            track: entry
                .get("track")
                .and_then(|v| v.as_str().and_then(|s| s.split('/').next()?.parse().ok()))
                .or_else(|| entry.get("track").and_then(|v| v.as_u64()).map(|n| n as u32)),
            duration: entry
                .get("length")
                .and_then(|v| v.as_str())
                .map(parse_length)
                .filter(|seconds| *seconds > 0)
                .or_else(|| length_by_stem.get(file_stem(name)).copied())
                .unwrap_or(0),
            size: entry
                .get("size")
                .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
                .unwrap_or(0),
        };

        if format_rank(&file).is_some() {
            audio.push(file);
        }
    }

    // One entry per track, at the best format that track offers.
    //
    // Picking a single best format for the whole item drops tracks: items
    // routinely have FLAC for some tracks and only MP3 for others, and every
    // MP3-only track then disappeared from the queue entirely. Ranking within
    // each track keeps the album complete while still preferring lossless
    // wherever it exists.
    let mut best_per_track: std::collections::HashMap<String, ArchiveFile> =
        std::collections::HashMap::new();
    for file in audio {
        let stem = file_stem(&file.name).to_string();
        match best_per_track.get(&stem) {
            Some(existing) if format_rank(existing) <= format_rank(&file) => {}
            _ => {
                best_per_track.insert(stem, file);
            }
        }
    }

    // Sorted on one key for every file. Comparing tagged files by track number
    // but falling back to the name when either side is untagged is not a total
    // order, and Rust's sort detects that and panics — Archive items commonly
    // tag only some of their files, so that was reachable.
    let mut audio: Vec<ArchiveFile> = best_per_track.into_values().collect();
    audio.sort_by(|a, b| {
        (a.track.unwrap_or(u32::MAX), &a.name).cmp(&(b.track.unwrap_or(u32::MAX), &b.name))
    });
    audio
}

/// The part of a file's name that its derivatives share: no directories, no
/// extension. `disc1/t01.flac` and `disc1/t01.mp3` both reduce to `t01`.
fn file_stem(name: &str) -> &str {
    let leaf = name.rsplit('/').next().unwrap_or(name);
    leaf.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(leaf)
}

/// Archive reports lengths either as seconds (`412.35`) or as `mm:ss`/`h:mm:ss`.
fn parse_length(raw: &str) -> u64 {
    if let Ok(seconds) = raw.parse::<f64>() {
        return seconds.max(0.0) as u64;
    }
    let mut total = 0u64;
    for part in raw.split(':') {
        let Ok(value) = part.trim().parse::<f64>() else {
            return 0;
        };
        total = total * 60 + value.max(0.0) as u64;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search_docs() {
        let body: serde_json::Value = serde_json::from_str(
            r#"{"response":{"docs":[
                {"identifier":"gd77-05-08","title":"Grateful Dead Live","creator":["Grateful Dead"],
                 "year":"1977","downloads":"12345","collection":["etree","GratefulDead"]},
                {"title":"no identifier"}
            ]}}"#,
        )
        .unwrap();

        let items = parse_search_response(&body).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].identifier, "gd77-05-08");
        assert_eq!(items[0].creator, "Grateful Dead");
        assert_eq!(items[0].downloads, 12345);
    }

    /// The bug this replaced: one best format was chosen for the whole item,
    /// so every track that lacked it was silently dropped from the queue.
    #[test]
    fn a_track_with_no_flac_still_appears_at_its_best_format() {
        let body: serde_json::Value = serde_json::from_str(
            r#"{"files":[
                {"name":"t01.flac","format":"Flac","track":"1","length":"10"},
                {"name":"t01.mp3","format":"VBR MP3","track":"1","length":"10"},
                {"name":"t02.mp3","format":"VBR MP3","track":"2","length":"20"},
                {"name":"t03.ogg","format":"Ogg Vorbis","track":"3","length":"30"}
            ]}"#,
        )
        .unwrap();

        let files = parse_metadata_files(&body);
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["t01.flac", "t02.mp3", "t03.ogg"],
            "every track survives, each at its own best format"
        );
    }

    #[test]
    fn prefers_flac_and_drops_other_derivatives() {
        let body: serde_json::Value = serde_json::from_str(
            r#"{"files":[
                {"name":"cover.jpg","format":"JPEG"},
                {"name":"t02.flac","format":"Flac","track":"2/10","length":"3:20","size":"200"},
                {"name":"t01.flac","format":"Flac","track":"1/10","length":"210.5","size":"100"},
                {"name":"t01.mp3","format":"VBR MP3","track":"1/10"}
            ]}"#,
        )
        .unwrap();

        let files = parse_metadata_files(&body);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].name, "t01.flac");
        assert_eq!(files[0].duration, 210);
        assert_eq!(files[1].duration, 200);
    }

    /// Real items in the Live Music Archive do this: the FLAC carries no
    /// `length`, but its MP3 twin does.
    #[test]
    fn a_missing_length_is_borrowed_from_a_sibling_derivative() {
        let body: serde_json::Value = serde_json::from_str(
            r#"{"files":[
                {"name":"d1/t01.flac","format":"Flac","track":"1"},
                {"name":"d1/t01.mp3","format":"VBR MP3","track":"1","length":"4:11"}
            ]}"#,
        )
        .unwrap();

        let files = parse_metadata_files(&body);
        assert_eq!(files.len(), 1, "only the best format is kept");
        assert_eq!(files[0].name, "d1/t01.flac");
        assert_eq!(files[0].duration, 251, "4:11 borrowed from the mp3");
    }

    /// Mixed tagged/untagged files used to produce an intransitive ordering,
    /// which Rust's sort turns into a panic.
    #[test]
    fn a_partly_tagged_item_sorts_without_panicking() {
        let mut files = String::from(r#"{"files":["#);
        for i in 0..40 {
            let track = if i % 2 == 0 {
                format!(r#","track":"{}""#, 40 - i)
            } else {
                String::new()
            };
            files.push_str(&format!(
                r#"{{"name":"t{i:02}.flac","format":"Flac","length":"10"{track}}},"#
            ));
        }
        files.pop();
        files.push_str("]}");

        let body: serde_json::Value = serde_json::from_str(&files).unwrap();
        assert_eq!(parse_metadata_files(&body).len(), 40);
    }

    #[test]
    fn falls_back_to_mp3_when_no_lossless() {
        let body: serde_json::Value =
            serde_json::from_str(r#"{"files":[{"name":"a/b c.mp3","format":"VBR MP3"}]}"#).unwrap();
        let files = parse_metadata_files(&body);
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].stream_url("some item"),
            "https://archive.org/download/some%20item/a/b%20c.mp3"
        );
    }
}
