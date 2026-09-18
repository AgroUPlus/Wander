# Three-Thread Execution Engine

Wander achieves glitch-free playback and responsive terminal interaction via three decoupled execution contexts:

```
┌─────────────────┐       Action        ┌──────────────────┐     Ring Buffer     ┌─────────────────────┐
│    UI Thread    │ ──────────────────> │   Tokio Async    │ ──────────────────> │   Realtime Audio    │
│    (Ratatui)    │ <────────────────── │     Runtime      │ <────────────────── │    Thread (cpal)    │
└─────────────────┘        State        └──────────────────┘     Sample Clock    └─────────────────────┘
```

## 1. UI Thread (Ratatui)
- Dedicated rendering engine decoupled from network or disk pauses.
- Handles keyboard events, mouse clicks, view routing, split panes, and spectrum visualizers.
- Receives state updates from the async runtime and emits user actions.

## 2. Tokio Async Runtime
- Handles all asynchronous operations:
  - Subsonic / Navidrome HTTP queries.
  - Local directory indexing and metadata parsing.
  - Agro WebSocket synchronization, presence tracking, and synchronized playback (jams).
  - Background audio decoding and buffer feeding.

## 3. Realtime Audio Thread (`cpal`)
- Low-latency audio callback reading directly from allocation-free ring buffers (`rtrb`).
- Decodes streams using `symphonia` and native `libopus`.
- **CRITICAL INVARIANT**: Never allocate heap memory, acquire blocking mutexes, or make system calls in the audio callback.
