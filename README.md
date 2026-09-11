<p align="center">
  <img src="docs/assets/WanderLogo.png" width="96" height="96" alt="Wander logo" />
</p>

<h1 align="center">Wander</h1>

<p align="center">
  Terminal music player (TUI) streaming from Navidrome / Subsonic and local libraries
</p>

<p align="center">
  <a href="https://github.com/Kolbxyz/wander">GitHub</a> ·
  <a href="LICENSE">License</a> ·
  <a href="#quick-start">Quick Start</a> ·
  <a href="#configuration">Configuration</a> ·
  <a href="#keybindings">Keybindings</a>
</p>

---

**Wander** is a fast, keyboard- and mouse-driven terminal music player (TUI) built in Rust. It streams from [Navidrome](https://www.navidrome.org/) and [Subsonic](http://www.subsonic.org/pages/api.jsp)-compatible music servers, plays local audio files directly from your hard drive, or combines both — offering **one unified library and queue**.

- **Unified Local & Remote Library** — Mix local MP3/FLAC/Opus files with streamed Navidrome tracks seamlessly in the same queue without playback gaps.
- **Native Audio Engine** — Low-latency output via `cpal` and `symphonia` with zero-copy ring buffers and native `libopus` decoding.
- **Terminal High-Res Visuals** — Native cover art rendering (Kitty, Sixel, iTerm2, half-blocks) and calibrated reactive visualisers (aurora, embers, bloom, oscilloscope, waterfall).
- **Synced Lyrics & Radio** — Real-time auto-scrolling lyrics, dynamic similarity queueing, MPRIS v2 controls, and Discord Rich Presence.

> *Named for what it is for: wandering through your music collection rather than just searching it.*

---

## Screenshots

<p align="center">
  <img width="48%" alt="Wander Library and Player View" src="https://github.com/user-attachments/assets/fa2726d5-1eea-41a5-bc82-6cf3fecf3165" />
  <img width="48%" alt="Wander Fullscreen Focus and Visualiser View" src="https://github.com/user-attachments/assets/592c14b0-25c6-494d-ad24-a87513a83b44" />
</p>

---

## Interface Overview

Wander organizes playback, management, and discovery across dedicated views:

| View | Description |
|---|---|
| **`1` Home** | Listening statistics, top played artists/tracks, and one-press smart mixes. |
| **`2` Library** | Fast fuzzy search across artists, albums, tracks, and genres (local & remote). |
| **`3` Queue** | Interactive queue manager with drag-and-drop mouse support, shuffle, and repeat modes. |
| **`⚡` Operations** | Dynamic tab tracking background downloads, library rescans, ETA gauges, and logs. |
| **`4` Settings** | In-app visual editor for server connections, music paths, theme colors, and UI layout. |
| **`F` Focus Mode** | Full-screen presentation mode with large cover art, lyrics, and real-time spectrum visualiser. |

---

## Quick Start

### Installation

Install Wander to `~/.local/bin` along with its desktop entry and icons:

```bash
./install.sh
```

Or build manually from source using Cargo:

```bash
cargo build --release
cp target/release/wander ~/.local/bin/
```

### First Run & Setup

Launch the setup wizard from your terminal:

```bash
wander --quickstart
```

1. Enter your **Navidrome server URL**, **username**, or **local music directories**.
2. Store your credentials securely inside your system keyring (Secret Service, Keychain):
   ```bash
   wander --set-password
   ```

> **Upgrading from naviplay?** Configuration directories, cached audio data, and keyring credentials are automatically migrated on first launch.

---

## Keybindings

Press `?` or `Ctrl+h` inside Wander at any time to open the live keybinding cheat sheet.

### Playback & Controls

| Key | Action |
|---|---|
| `Space` | Play / Pause |
| `n` / `p` | Next / Previous track |
| `f` / `b` | Seek forward / backward (5s) |
| `+` / `-` | Increase / Decrease volume |
| `*` | Star / Unstar current track |
| `.` / `,` | Raise / Lower track rating |
| `z` / `r` | Toggle Shuffle / Repeat mode |
| `x` | Toggle **Radio Mode** (auto-queues similar songs) |

### Navigation & Views

| Key | Action |
|---|---|
| `1` – `4` | Switch directly to Tab 1–4 |
| `[` / `]` or `Tab` / `Shift+Tab` | Switch to Previous / Next tab |
| `Alt+←` / `Alt+→` | Resize split panes |
| `h` / `l` or `←` / `→` | Change pane focus / adjust settings values |
| `Ctrl+←` / `Ctrl+→` | Cycle pane focus across all tabs (including Up Next) |
| `Backspace` | Return to previously active tab |
| `Enter` | Play selection |
| `a` | Append selection to queue |
| `Ctrl+z` | Undo last queue change |
| `/` or `Ctrl+p` | Command palette & fuzzy search |
| `F` | Toggle **Focus Mode** |
| `Q` | Toggle Queue side pane |
| `v` / `V` | Toggle visualiser / cycle style (aurora, ember, bloom, scope, waterfall) |
| `Y` / `T` | Cycle lyric variant / Translate lyrics |

*Full mouse support is enabled across all tabs: click to navigate, double-click to play, drag sliders, and scroll lyrics.*

---

## Configuration

Configuration lives at `~/.config/wander/config.toml` and can be configured through the built-in **Settings** view or edited directly:

```toml
[server]
url = "https://navidrome.example.com"
username = "your_username"
# password is stored in the OS keyring (use: wander --set-password)
# format = "raw"  # Options: "raw", "mp3", "opus"

[local]
paths = ["~/Music", "/media/audio"]
scan_on_start = false
playlist_dir = "~/Music/Playlists"

[general]
buffer_seconds = 5.0
glyphs = "nerd"    # Options: "nerd", "unicode", "ascii"

[lyrics]
translate_url = ""
translate_api_key = ""
translate_to = "en"

[discord]
enabled = false
client_id = ""
cover_art = true

[theme]
background = "#1e1e2e"
foreground = "#cdd6f4"
border = "#45475a"
border_focused = "#89b4fa"
accent = "#cba6f7"
highlight_bg = "#313244"
highlight_fg = "#f5e0dc"
current_track = "#a6e3a1"
dim = "#6c7086"
progress = "#89b4fa"
error = "#f38ba8"
viz_low = "#89b4fa"
viz_high = "#f5e0dc"
```

### Share links

Wander can rewrite share URLs through a custom `/listen` forwarder (such as an Agro instance):

```toml
[share]
domain = "frwd.top"
hosts = ["music.example.com"]
```

Incoming links resolve to `https://your-domain/listen?u=...` and avoid exposing internal server domains. Unmatched hosts are shared untouched.

---

## Architecture

Wander decouples user interaction, networking, and audio playback across three dedicated threads:

```
┌─────────────────┐       Action        ┌──────────────────┐     Ring Buffer     ┌─────────────────────┐
│    UI Thread    │ ──────────────────> │   Tokio Async    │ ──────────────────> │   Realtime Audio    │
│    (Ratatui)    │ <────────────────── │     Runtime      │ <────────────────── │    Thread (cpal)    │
└─────────────────┘        State        └──────────────────┘     Sample Clock    └─────────────────────┘
```

- **UI Thread** — Dedicated Ratatui rendering engine decoupled from I/O pauses.
- **Tokio Runtime** — Handles Subsonic/Navidrome HTTP requests, local disk scanning, and background stream decoding.
- **Realtime Audio Thread** — Low-latency `cpal` callback reading directly from allocation-free ring buffers.

---

## Diagnostics & CLI

```bash
# Verify native pipeline decoding for an audio file
wander --decode-check /path/to/file.opus

# Test local library indexing and check tag parsing
wander --scan-check ~/Music

# Securely set or update keyring server password
wander --set-password
```

---

## Development

```bash
# Run test suite
cargo test

# Validate formatting and lints
cargo fmt --check
cargo clippy --all-targets
```

---

## License

Distributed under the **MIT License**. See [`LICENSE`](LICENSE) for details.
