# Wander 🎵

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Language: Rust](https://img.shields.io/badge/Rust-2024_Edition-orange.svg)](https://www.rust-lang.org/)
[![API: Subsonic](https://img.shields.io/badge/API-Subsonic%20%2F%20Navidrome-green.svg)](https://www.navidrome.org/)
[![TUI: Ratatui](https://img.shields.io/badge/TUI-Ratatui-purple.svg)](https://ratatui.rs/)

**Wander** is a fast, keyboard- and mouse-driven terminal music player (TUI) built in Rust. It streams from [Navidrome](https://www.navidrome.org/) and [Subsonic](http://www.subsonic.org/pages/api.jsp)-compatible music servers, plays local audio files directly from your hard drive, or combines both — offering **one unified library and queue**.

> *Named for what it is for: wandering through your music collection rather than just searching it.*

---

## ⚡ Highlights

- **Unified Local & Remote Library**: Mix local MP3/FLAC/Opus files with streamed Navidrome tracks seamlessly in the same queue without playback gaps.
- **Native Audio Engine**: High-performance audio output via `cpal` and `symphonia` with zero-copy ring buffers. Native `libopus` decoding ensures Opus tracks stream bit-perfect without server transcoding.
- **Terminal High-Res Visuals**: High-resolution cover art (Kitty, Sixel, iTerm2, and half-block fallbacks) alongside an in-terminal spectrum visualiser.
- **Synced & Unsynced Lyrics**: Real-time timed lyrics with smooth auto-scrolling, or manual navigation for unsynced track lyrics.
- **Endless Radio Mode**: Automatically queues contextually relevant tracks using Navidrome similarity API, genre matching, and local listening history.
- **System & Desktop Integration**: Native MPRIS v2 interface (`playerctl`, desktop shell bars) and Discord Rich Presence with MusicBrainz cover artwork resolution.
- **Full Customizability**: Flexible key rebinding system and complete color palette customization.

---

## 📸 Interface Overview

Wander features four main interactive tabs and a full-screen focus view:

| View | Description |
| :--- | :--- |
| **`1` Home** | Listening statistics, top played artists/tracks, and one-press smart mixes. |
| **`2` Library** | Fast fuzzy search across artists, albums, tracks, and genres (local & remote). |
| **`3` Queue** | Interactive queue manager with drag-and-drop mouse support, shuffle, and repeat modes. |
| **`4` Settings** | In-app visual editor for server connections, music paths, theme colors, and UI layout. |
| **`F` Focus Mode** | Full-screen presentation mode featuring large cover art, lyrics, and real-time spectrum visualiser. |

---

## 🚀 Quick Start

### Installation

Run the provided installation script to build and install Wander to `~/.local/bin`, along with its desktop entry and icon:

```bash
./install.sh
```

Alternatively, build manually using Cargo:

```bash
cargo build --release
cp target/release/wander ~/.local/bin/
```

### First Run & Setup

Simply launch `wander` from your shell or application launcher:

```bash
wander
```

1. On initial startup, Wander guides you through configuring your **Navidrome server URL**, **username**, and/or **local music directories**.
2. Passwords are securely stored in your operating system's keyring (e.g. Secret Service, Keychain) and never saved in plain text:
   ```bash
   wander --set-password
   ```

> [!NOTE]  
> Upgrading from `naviplay`? Configuration directories, cached audio data, and keyring credentials are automatically migrated on first launch.

---

## ⌨️ Keybindings

Press `?` or `Ctrl+h` inside Wander at any time to display the live keybinding cheat sheet.

### Playback & Volume

| Key | Action |
| :--- | :--- |
| `Space` | Play / Pause |
| `n` / `p` | Next / Previous track |
| `f` / `b` | Seek forward / backward (5s) |
| `+` / `-` | Increase / Decrease volume |
| `*` | Star / Unstar current track |
| `.` / `,` | Raise / Lower track rating |
| `z` / `r` | Toggle Shuffle / Repeat mode |
| `x` | Toggle **Radio Mode** (auto-queues similar songs) |

### Navigation & Queue

| Key | Action |
| :--- | :--- |
| `1` – `4` | Switch directly to Tab 1–4 |
| `[` / `]` or `Alt+←` / `Alt+→` | Switch to Previous / Next tab |
| `Backspace` | Return to previously focused tab |
| `Enter` | Play highlighted track / selection |
| `a` | Append selection to queue |
| `Ctrl+z` | Undo last queue mutation |
| `/` or `Ctrl+p` | Open Command Palette & Fuzzy Launcher |

### UI & Layout

| Key | Action |
| :--- | :--- |
| `F` | Toggle **Focus Mode** (Full-screen Cover + Lyrics + Visualiser) |
| `Q` | Toggle Queue Side Pane |
| `c` | Toggle Cover Art Display |
| `v` | Toggle Spectrum Visualiser |
| `?` or `Ctrl+h` | Open Live Help / Keymap Cheat Sheet |

> [!TIP]  
> **Mouse Controls**: Mouse interaction is fully supported across all tabs! Click tabs and lists, double-click rows to play, drag progress/volume sliders, and scroll lyrics.

---

## ⚙️ Configuration

Configuration files are located at `~/.config/wander/config.toml`. All settings can be edited interactively from the **Settings** tab inside the app or modified manually.

```toml
[server]
url = "https://navidrome.example.com"
username = "your_username"
# password is stored in the OS keyring (use: wander --set-password)
# enabled = true
# format = "raw"  # Force transcoding: "raw", "mp3", or "opus"

[local]
# Local directories to index (leave empty for server-only mode)
paths = ["~/Music", "/media/audio"]
scan_on_start = false
playlist_dir = "~/Music/Playlists"

[general]
buffer_seconds = 5.0
glyphs = "nerd"    # Icon set: "nerd", "unicode", or "ascii"

[discord]
enabled = false
client_id = ""     # Optional custom Discord App ID
cover_art = true   # Fetch public Cover Art Archive covers for Discord Rich Presence

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

### Customizing Keybindings

Override default keys by adding a `[keys]` table to `config.toml`:

```toml
[keys]
"ctrl+p" = "open_palette"
"alt+f"  = "toggle_focus_mode"
"g"      = "none"  # Unbind default key
"F5"     = "refresh"
```

---

## 🏗️ Architecture

Wander uses a 3-thread decoupled architecture connected by high-performance channels, ensuring smooth UI rendering and stutter-free audio output:

```
┌─────────────────┐       Action       ┌──────────────────┐      Ring Buffer     ┌─────────────────────┐
│    UI Thread    │ ─────────────────> │  Tokio Async     │ ───────────────────> │  Realtime Audio     │
│   (Ratatui)     │ <───────────────── │  Runtime         │ <─────────────────── │  Thread (cpal)      │
└─────────────────┘       State        └──────────────────┘     Sample Clock     └─────────────────────┘
```

- **UI Thread**: Renders the interface at high framerates using `ratatui` without blocking on disk or network I/O.
- **Tokio Runtime**: Handles background HTTP requests, Subsonic API communications, local directory scanning, and audio decoding.
- **Audio Output Thread**: Executes a realtime `cpal` callback reading directly from lock-free ring buffers (zero allocations, zero locks).

---

## 🛠️ Diagnostics & CLI Utilities

Wander includes built-in CLI tools for inspecting audio files and tag parsing:

```bash
# Verify native pipeline decoding for a specific file
wander --decode-check /path/to/file.opus

# Test local library scanner output and inspect metadata tags
wander --scan-check ~/Music

# Prompt securely for server password and save to keyring
wander --set-password
```

---

## 🛠️ Development & Testing

```bash
# Run unit tests
cargo test

# Check code formatting & lints
cargo fmt --check
cargo clippy --all-targets
```

---

## 📜 License

Distributed under the **MIT License**. See [`LICENSE`](LICENSE) for details.
