# Wander Developer Documentation Index

This directory contains developer documentation and engineering guides for Wander (terminal music player in Rust), organized following Linux-kernel style modular documentation principles.

## Structure

```
docs/dev/
├── process/
│   ├── coding-style.md             # Rust conventions, error handling & 300-line limit
│   ├── research-workflow.md        # Mandatory 4-step research workflow
│   └── git-and-authorship.md       # Clean commits, CLA Section 8, no AI attribution
├── architecture/
│   ├── three-thread-engine.md      # UI Thread (Ratatui), Tokio runtime, and cpal audio loop
│   └── agro-parity.md              # Wire synchronization with Agro sync daemon
└── tools/
    └── commands-and-testing.md     # Cargo commands, clippy, tests, and formatting
```

## Guiding Principles

1. **Lightweight Root Pointer**: Root `AGENTS.md` and `CLAUDE.md` act as fast navigation indexes and invariant guards; deep implementation details live here.
2. **Allocation-Free Realtime Audio**: Never allocate, lock mutexes, or make syscalls inside the audio callback.
3. **Hardware & Terminal Performance**: Decoupled rendering loop prevents I/O pauses from stuttering TUI interaction.
