use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::theme::Theme;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub theme: Theme,
    /// Name of the preset `theme` came from, so the settings screen can show it
    /// and cycling can continue from the right place. Identifying the preset by
    /// comparing colours breaks as soon as two presets share an accent.
    /// `None` means the user hand-edited the palette.
    pub theme_preset: Option<String>,
    pub queue_columns: Vec<Column>,
    /// Seconds of audio to buffer ahead of the output device.
    pub buffer_seconds: f32,
    pub volume_log: bool,
    /// Icon set: `nerd` (needs a patched font), `unicode`, or `ascii`.
    #[serde(default)]
    pub glyphs: crate::ui::glyphs::GlyphSet,
    pub discord: DiscordConfig,
    pub local: LocalConfig,
    pub lyrics: LyricsConfig,
    /// Key overrides, e.g. `"ctrl+p" = "open_palette"`. `"none"` unbinds a key.
    /// Anything not listed keeps its default binding.
    #[serde(default)]
    pub keys: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscordConfig {
    pub enabled: bool,
    /// Application ID from <https://discord.com/developers/applications>.
    ///
    /// Rich Presence requires your own application; there is no shared one to
    /// fall back on, so this must be set for `enabled` to do anything.
    pub client_id: String,
    /// Show album art from the public Cover Art Archive when the album has a
    /// MusicBrainz ID. Navidrome's own cover URLs are never sent: they embed
    /// an auth token that would grant Discord access to the library.
    pub cover_art: bool,
}

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            client_id: String::new(),
            cover_art: true,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            theme: Theme::default(),
            theme_preset: Some("Tokyo Night".to_string()),
            queue_columns: Column::defaults(),
            buffer_seconds: 5.0,
            volume_log: true,
            glyphs: crate::ui::glyphs::GlyphSet::default(),
            discord: DiscordConfig::default(),
            local: LocalConfig::default(),
            lyrics: LyricsConfig::default(),
            keys: std::collections::HashMap::new(),
        }
    }
}

/// On-demand lyric translation.
///
/// Off unless `translate_url` is set, and never automatic: pressing the key
/// sends the track's lyrics to whatever endpoint is named here, which is the
/// user's decision to make rather than a default to inherit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LyricsConfig {
    /// A LibreTranslate-compatible `/translate` endpoint, e.g.
    /// `http://localhost:5000/translate`. Empty disables translation.
    pub translate_url: String,
    /// API key, if the endpoint wants one.
    pub translate_api_key: String,
    /// Target language code.
    pub translate_to: String,
}

impl Default for LyricsConfig {
    fn default() -> Self {
        Self {
            translate_url: String::new(),
            translate_api_key: String::new(),
            translate_to: "en".to_string(),
        }
    }
}

impl LyricsConfig {
    pub fn translation_enabled(&self) -> bool {
        !self.translate_url.trim().is_empty()
    }
}

/// A local, on-disk music collection, browsable alongside the server.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalConfig {
    /// Folders to scan. Empty means the local library is off.
    pub paths: Vec<PathBuf>,
    /// Where `.m3u8` playlists are read from and written to. Local playlists
    /// are unavailable until this is set, since there is nowhere to put them.
    pub playlist_dir: Option<PathBuf>,
    /// Rescan at startup. Off by default: the persisted index is usually still
    /// accurate, and a large collection makes startup wait.
    pub scan_on_start: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Whether to use the server at all. Lets a local-only user switch
    /// Navidrome off without throwing away their credentials.
    pub enabled: bool,
    /// Base URL of the Navidrome server, e.g. `https://music.example.com`.
    pub url: String,
    pub username: String,
    /// Optional plaintext password. Prefer leaving this empty and storing the
    /// password in the OS keyring instead; see `Config::password`.
    pub password: String,
    /// Transcode format requested from the server. `raw` means no transcode.
    pub format: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        // Enabled by default so an existing config keeps working after the
        // field was introduced; an empty URL is what turns it off in practice.
        Self {
            enabled: true,
            url: String::new(),
            username: String::new(),
            password: String::new(),
            format: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Column {
    pub kind: ColumnKind,
    /// Width as a percentage of the table width.
    pub width: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColumnKind {
    Artist,
    Title,
    Album,
    Length,
    Track,
    Year,
}

impl ColumnKind {
    pub fn header(self) -> &'static str {
        match self {
            Self::Artist => "Artist",
            Self::Title => "Title",
            Self::Album => "Album",
            Self::Length => "Len",
            Self::Track => "#",
            Self::Year => "Year",
        }
    }
}

impl Column {
    fn defaults() -> Vec<Self> {
        vec![
            Self {
                kind: ColumnKind::Artist,
                width: 25,
            },
            Self {
                kind: ColumnKind::Title,
                width: 35,
            },
            Self {
                kind: ColumnKind::Album,
                width: 30,
            },
            Self {
                kind: ColumnKind::Length,
                width: 10,
            },
        ]
    }
}

impl Config {
    pub fn path() -> Result<PathBuf> {
        Ok(crate::paths::config_dir()?.join("config.toml"))
    }

    /// Load the config, falling back to defaults when the file does not exist.
    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading config at {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing config at {}", path.display()))
    }

    /// Save the config to disk.
    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let text = toml::to_string_pretty(self).context("serializing config")?;
        std::fs::write(&path, text).with_context(|| format!("writing config at {}", path.display()))
    }

    /// Resolve the password, preferring the OS keyring over the config file.
    pub fn password(&self) -> Result<String> {
        if !self.server.password.is_empty() {
            return Ok(self.server.password.clone());
        }
        crate::paths::keyring_password(&self.server.username)
    }
}
