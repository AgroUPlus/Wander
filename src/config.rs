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
    pub tray: TrayConfig,
    pub local: LocalConfig,
    pub lyrics: LyricsConfig,
    pub plugins: PluginsConfig,
    pub agro: AgroConfig,
    pub sync: SyncConfig,
    pub share: ShareConfig,
    /// Key overrides, e.g. `"ctrl+p" = "open_palette"`. `"none"` unbinds a key.
    /// Anything not listed keeps its default binding.
    #[serde(default)]
    pub keys: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgroConfig {
    pub enabled: bool,
    #[serde(alias = "url")]
    pub server: String,
    pub username: String,
    #[serde(alias = "api_key", alias = "token")]
    pub passphrase: String,
    /// The device credential bought with the passphrase, cached between runs.
    ///
    /// Separate from [`passphrase`] because they are different kinds of secret: the passphrase is
    /// the account, this is one device. Without somewhere to keep it, every `AgroClient` — and one
    /// is built per operation — had to buy a fresh token from `/api/v1/login`, leaving a row in
    /// the server's app-password list each time. Tokens piled up faster than anyone could read
    /// them, all wearing the same label.
    ///
    /// Written by the client itself, not by hand. Deleting it costs nothing: the next request
    /// simply buys another.
    #[serde(default)]
    pub device_token: String,
    pub device_id: String,
    pub device_name: Option<String>,
    pub sync_settings: bool,
    /// Read listening statistics from Agro instead of from this machine's own play log.
    ///
    /// Off by default, because the local log is what this install already has and switching a
    /// device to the fleet's totals should be a decision, not something that happens on upgrade.
    /// Plays are reported to Agro either way — the flag only decides where the Home tab reads.
    pub central_stats: bool,
    /// X25519 identity keys for E2EE track drops and communications.
    #[serde(default)]
    pub identity_private_key: Option<String>,
    #[serde(default)]
    pub identity_public_key: Option<String>,
}

/// Where share links go out.
///
/// A domain of your own, serving the `/listen` forwarder, so a link works for whoever receives it
/// rather than only for someone using the same backend. Blank means share the server's own link,
/// which is what happens with none of this configured.
///
/// A paired Agro server may publish this for the whole fleet — see `Agro::share_settings` — and
/// when it does, its value wins over what is here. Agro is optional in both directions: without
/// it, this field still works on its own.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ShareConfig {
    /// Bare host, e.g. `frwd.top`.
    pub domain: String,
    /// Extra hosts the forwarder will accept, beyond YouTube's and your music server's.
    pub hosts: Vec<String>,
}

/// Device sync and archiving settings for Agro.
///
/// Separate from [`AgroConfig`] because it is a separate decision: pairing with Agro was about
/// playback handoff, and sync/archiving is configured on its own.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncConfig {
    /// P2P device sync: hash and report local holdings for direct device-to-device transfers.
    /// Zero server storage used. Default: true.
    #[serde(alias = "report_holdings")]
    pub p2p_sync: bool,
    /// Upload local audio files to the Agro / Navidrome server storage.
    /// Admin-only. Default: false.
    #[serde(alias = "enabled")]
    pub server_archive: bool,
    /// Files hashed per pass.
    pub hash_batch: usize,
    /// Files uploaded per pass (when server_archive is enabled).
    pub upload_batch: usize,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            p2p_sync: true,
            server_archive: false,
            hash_batch: 200,
            upload_batch: 25,
        }
    }
}

fn default_device_id() -> String {
    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "desktop".to_string());
    format!("wander-{}", hostname)
}

impl AgroConfig {
    /// Retrieves or generates X25519 identity keys for E2EE drops and relay streaming.
    pub fn get_or_create_identity_keys(&mut self) -> (x25519_dalek::StaticSecret, x25519_dalek::PublicKey) {
        use base64::prelude::*;
        if let (Some(priv_b64), Some(pub_b64)) = (&self.identity_private_key, &self.identity_public_key) {
            if let (Ok(priv_bytes), Ok(pub_bytes)) = (
                BASE64_STANDARD.decode(priv_b64.trim()),
                BASE64_STANDARD.decode(pub_b64.trim()),
            ) {
                if priv_bytes.len() == 32 && pub_bytes.len() == 32 {
                    let mut priv_arr = [0u8; 32];
                    let mut pub_arr = [0u8; 32];
                    priv_arr.copy_from_slice(&priv_bytes);
                    pub_arr.copy_from_slice(&pub_bytes);
                    let secret = x25519_dalek::StaticSecret::from(priv_arr);
                    let public = x25519_dalek::PublicKey::from(pub_arr);
                    return (secret, public);
                }
            }
        }

        // Generate new keypair using OS CSPRNG
        let secret = x25519_dalek::StaticSecret::random_from_rng(chacha20poly1305::aead::OsRng);
        let public = x25519_dalek::PublicKey::from(&secret);

        self.identity_private_key = Some(BASE64_STANDARD.encode(secret.to_bytes()));
        self.identity_public_key = Some(BASE64_STANDARD.encode(public.as_bytes()));

        (secret, public)
    }

    /// Whether this device has anything it can authenticate with.
    ///
    /// Either credential will do, and asking about the passphrase alone is wrong: after the first
    /// login — or after a pairing URI is read — the passphrase is *gone*, deliberately, and the
    /// device token is the only thing left. Every "is Agro set up?" check has to accept that, or a
    /// correctly paired device silently reports nothing at all.
    pub fn has_credential(&self) -> bool {
        !self.device_token.trim().is_empty() || !self.passphrase.trim().is_empty()
    }

    /// Ready to talk to a server: switched on, told where, and holding a credential.
    pub fn is_ready(&self) -> bool {
        self.enabled && !self.server.trim().is_empty() && self.has_credential()
    }

    /// Accepts a whole pairing URI where a credential is expected.
    ///
    /// What the dashboard puts on screen is `agro://connect?username=…&token=…&server=…` — that is
    /// the string being copied, and pasting all of it into `passphrase` is the obvious mistake to
    /// make. It fails silently and identically to a wrong credential: the URI goes out as a bearer
    /// token, the server refuses it, and nothing says why. Reading the token out of it is cheaper
    /// than explaining the difference, and the URI carries the username and server too, so a
    /// single paste is enough to pair.
    fn absorb_pairing_uri(&mut self) {
        let raw = self.passphrase.trim().to_string();
        if !raw.starts_with("agro://") {
            return;
        }
        let Some(query) = raw.split_once('?').map(|(_, q)| q) else {
            return;
        };
        for (key, value) in query.split('&').filter_map(|pair| pair.split_once('=')) {
            let decoded = percent_decode(value);
            if decoded.is_empty() {
                continue;
            }
            match key {
                "token" => {
                    self.device_token = decoded;
                    self.passphrase = String::new();
                }
                "username" => self.username = decoded,
                "server" => self.server = decoded.trim_end_matches('/').to_string(),
                _ => {}
            }
        }
    }
}

/// Just enough percent-decoding for the one field that carries it, the server URL.
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&raw[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).trim().to_string()
}

impl Default for AgroConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server: "https://agro.kolbxyz.xyz".to_string(),
            username: "alpha".to_string(),
            passphrase: String::new(),
            device_token: String::new(),
            device_id: default_device_id(),
            device_name: None,
            sync_settings: true,
            central_stats: false,
            identity_private_key: None,
            identity_public_key: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginsConfig {
    #[cfg(feature = "nyaa")]
    pub nyaa: NyaaConfig,
    pub archive: ArchiveConfig,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            #[cfg(feature = "nyaa")]
            nyaa: NyaaConfig::default(),
            archive: ArchiveConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnlinePrimaryAction {
    Stream,
    Download,
}

impl OnlinePrimaryAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Stream => "Stream (Play Now)",
            Self::Download => "Download to local library",
        }
    }
}

#[cfg(feature = "nyaa")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NyaaConfig {
    pub enabled: bool,
    pub download_dir: Option<PathBuf>,
    pub category: String,
    pub primary_action: OnlinePrimaryAction,
}

#[cfg(feature = "nyaa")]
impl Default for NyaaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            download_dir: None,
            category: "2_0".to_string(),
            primary_action: OnlinePrimaryAction::Stream,
        }
    }
}

/// Internet Archive plugin: legal streaming and downloads of the audio
/// collections archive.org hosts (live music, netlabels, 78 RPM transfers).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ArchiveConfig {
    pub enabled: bool,
    pub download_dir: Option<PathBuf>,
    /// Collection code from `ArchiveCollection::code`, e.g. `etree`.
    pub collection: String,
    pub primary_action: OnlinePrimaryAction,
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            download_dir: None,
            collection: "audio".to_string(),
            primary_action: OnlinePrimaryAction::Stream,
        }
    }
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

/// The status-bar icon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TrayConfig {
    /// On by default, unlike Discord Rich Presence: this sends nothing anywhere, needs no account
    /// or client id, and on a machine with no status-bar host it simply does not appear. There is
    /// nothing for a user to opt into, only something to switch off if they would rather not have
    /// the icon.
    pub enabled: bool,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
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
            tray: TrayConfig::default(),
            theme: Theme::default(),
            theme_preset: Some("Tokyo Night".to_string()),
            queue_columns: Column::defaults(),
            buffer_seconds: 5.0,
            volume_log: true,
            glyphs: crate::ui::glyphs::GlyphSet::default(),
            discord: DiscordConfig::default(),
            local: LocalConfig::default(),
            lyrics: LyricsConfig::default(),
            plugins: PluginsConfig::default(),
            agro: AgroConfig::default(),
            sync: SyncConfig::default(),
            share: ShareConfig::default(),
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
    /// Fetch missing lyrics online via LRCLIB when not in local tags/server.
    pub fetch_online: bool,
    /// Base URL of LRCLIB API. Defaults to `https://lrclib.net`.
    pub lrclib_url: String,
}

impl Default for LyricsConfig {
    fn default() -> Self {
        Self {
            translate_url: String::new(),
            translate_api_key: String::new(),
            translate_to: "en".to_string(),
            fetch_online: true,
            lrclib_url: "https://lrclib.net".to_string(),
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
    /// Where the track comes from: local file, server, or online plugin.
    Source,
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
            Self::Source => "Src",
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
            Self {
                kind: ColumnKind::Source,
                width: 5,
            },
        ]
    }
}

/// Takes the config away from the group and the world.
///
/// `password` and `agro.passphrase` are stored here in plaintext whenever the keyring is not in
/// use — [`Config::password`] *prefers* the keyring but falls back to this file, so it cannot be
/// assumed empty. A default umask leaves it world-readable, which on a shared machine hands every
/// other account the Navidrome password and the Agro pairing passphrase.
///
/// Best effort: a failure is not fatal. Refusing to start over a chmod would break the program on
/// filesystems that have no modes to set.
#[cfg(unix)]
fn restrict_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    let mode = metadata.permissions().mode() & 0o777;
    if mode == 0o600 {
        return;
    }
    if let Err(error) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        eprintln!(
            "wander: could not restrict permissions on {}: {error}",
            path.display()
        );
    }
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &std::path::Path) {}

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
        // Older configs were written before the mode was enforced, and a hand-edited one picks up
        // whatever the user's umask gives it. Narrowed on the way in rather than only on the way
        // out, so a file that has been sitting readable does not stay that way until the next save.
        restrict_permissions(&path);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading config at {}", path.display()))?;
        let mut config: Self = toml::from_str(&text)
            .with_context(|| format!("parsing config at {}", path.display()))?;
        config.agro.absorb_pairing_uri();
        Ok(config)
    }

    /// Save the config to disk.
    ///
    /// Written to a sibling temp file and renamed over the original, because every settings toggle
    /// calls this: writing in place means a crash, a full disk or a kill at the wrong moment
    /// leaves a half-written config, and the next launch starts with defaults or an error. The
    /// rename is atomic within a directory, so the file on disk is always one whole config or the
    /// other.
    ///
    /// Note this rewrites the file from the struct, so comments and key order in a hand-edited
    /// `config.toml` do not survive a save. That is inherent to round-tripping through `toml`.
    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let text = toml::to_string_pretty(self).context("serializing config")?;

        let temp = path.with_extension("toml.new");
        std::fs::write(&temp, text)
            .with_context(|| format!("writing config at {}", temp.display()))?;
        // Narrowed before the rename, not after: `rename` carries the mode with it, so setting it
        // here means the file is never observable at a wider mode, even briefly.
        restrict_permissions(&temp);
        std::fs::rename(&temp, &path)
            .with_context(|| format!("replacing config at {}", path.display()))
    }

    /// Resolve the password, preferring the OS keyring over the config file.
    pub fn password(&self) -> Result<String> {
        if !self.server.password.is_empty() {
            return Ok(self.server.password.clone());
        }
        crate::paths::keyring_password(&self.server.username)
    }
}

#[cfg(all(test, unix))]
mod permission_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_widely_readable_config_is_narrowed() {
        let dir = std::env::temp_dir().join(format!("wander-perm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        restrict_permissions(&path);

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "config is {mode:o}, not 0600");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        restrict_permissions(std::path::Path::new("/nonexistent/wander/config.toml"));
    }
}

#[cfg(test)]
mod pairing_uri_tests {
    use super::*;

    /// The string on the dashboard is a URI, and that is what gets pasted.
    #[test]
    fn a_pairing_uri_is_read_for_its_parts() {
        let mut agro = AgroConfig {
            passphrase:
                "agro://connect?username=alpha&token=abc123&server=https%3A%2F%2Fagro.example.com"
                    .to_string(),
            ..AgroConfig::default()
        };
        agro.absorb_pairing_uri();

        assert_eq!(agro.device_token, "abc123");
        assert_eq!(agro.username, "alpha");
        assert_eq!(agro.server, "https://agro.example.com");
        // The URI is not a passphrase and must not be left sitting in the passphrase field, where
        // it would be sent to /api/v1/login on the next 401.
        assert!(agro.passphrase.is_empty());
    }

    /// A URI without a server keeps whatever was already configured.
    #[test]
    fn a_uri_without_a_server_leaves_the_configured_one() {
        let mut agro = AgroConfig {
            server: "https://agro.example.com".to_string(),
            passphrase: "agro://connect?username=beta&token=xyz".to_string(),
            ..AgroConfig::default()
        };
        agro.absorb_pairing_uri();
        assert_eq!(agro.server, "https://agro.example.com");
        assert_eq!(agro.device_token, "xyz");
    }

    /// An ordinary passphrase is left exactly as it is.
    #[test]
    fn a_real_passphrase_is_untouched() {
        let mut agro = AgroConfig {
            passphrase: "sonar-ocean-glacier-eagle".to_string(),
            ..AgroConfig::default()
        };
        agro.absorb_pairing_uri();
        assert_eq!(agro.passphrase, "sonar-ocean-glacier-eagle");
        assert!(agro.device_token.is_empty());
    }
}

#[cfg(test)]
mod credential_tests {
    use super::*;

    /// A device paired from a QR has a token and no passphrase, and that is fully configured.
    ///
    /// Every "is Agro set up?" check used to ask about the passphrase alone. Reading a pairing URI
    /// moves the token out of that field, so a correctly paired device answered "no credential"
    /// everywhere and its whole reporting task refused to start.
    #[test]
    fn a_device_token_alone_counts_as_paired() {
        let agro = AgroConfig {
            enabled: true,
            server: "https://agro.example.com".into(),
            passphrase: String::new(),
            device_token: "a-device-token".into(),
            ..AgroConfig::default()
        };
        assert!(agro.has_credential(), "a device token is a credential");
        assert!(
            agro.is_ready(),
            "a paired device was treated as unconfigured"
        );
    }

    #[test]
    fn a_passphrase_alone_still_counts() {
        let agro = AgroConfig {
            enabled: true,
            server: "https://agro.example.com".into(),
            passphrase: "four-word-pass-phrase".into(),
            device_token: String::new(),
            ..AgroConfig::default()
        };
        assert!(agro.is_ready());
    }

    #[test]
    fn nothing_configured_is_not_ready() {
        let agro = AgroConfig {
            enabled: true,
            server: "https://agro.example.com".into(),
            passphrase: String::new(),
            device_token: String::new(),
            ..AgroConfig::default()
        };
        assert!(!agro.has_credential());
        assert!(!agro.is_ready());
    }

    /// Reading a pairing URI must leave the config in a state the rest of the app accepts.
    #[test]
    fn a_pairing_uri_leaves_a_ready_config() {
        let mut agro = AgroConfig {
            enabled: true,
            passphrase:
                "agro://connect?username=alpha&token=abc123&server=https%3A%2F%2Fagro.example.com"
                    .into(),
            ..AgroConfig::default()
        };
        agro.absorb_pairing_uri();
        assert!(
            agro.is_ready(),
            "pairing from a URI produced a config nothing would use"
        );
    }
}

#[cfg(test)]
mod tray_config_tests {
    use super::*;

    /// On unless the user says otherwise. The icon costs nothing on a machine with no status bar,
    /// so making people discover a setting before they get it would be the wrong default.
    #[test]
    fn the_tray_is_enabled_by_default() {
        assert!(TrayConfig::default().enabled);
        assert!(Config::default().tray.enabled);
    }

    /// An existing `config.toml` predates this section entirely, and reading one must not reset
    /// the file or fail: `#[serde(default)]` is what fills the gap.
    #[test]
    fn a_config_without_a_tray_section_still_parses() {
        let config: Config = toml::from_str("").expect("an empty config is valid");
        assert!(config.tray.enabled, "a missing section means the default");
    }

    #[test]
    fn the_tray_can_be_switched_off() {
        let config: Config =
            toml::from_str("[tray]\nenabled = false\n").expect("valid config");
        assert!(!config.tray.enabled);
    }

    /// It has to survive a save/load cycle, or switching it off would not stick.
    #[test]
    fn the_setting_round_trips_through_toml() {
        let mut config = Config::default();
        config.tray.enabled = false;
        let text = toml::to_string(&config).expect("serialises");
        let back: Config = toml::from_str(&text).expect("deserialises");
        assert!(!back.tray.enabled);
    }
}
