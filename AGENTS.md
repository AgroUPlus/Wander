# Wander

Terminal music player (TUI) in Rust unifying **Navidrome / Subsonic**, **local files**, and **Agro** sync. Keyboard and mouse driven, built on Ratatui, Tokio, and cpal.

## 1. Non-Negotiable Hard Rules

| Rule | Why |
| --- | --- |
| **Audio loop allocation-free** | Never allocate, lock mutexes, or make syscalls in `cpal` callback (`rtrb`). |
| **300 lines max per file** | Split when file passes 250 lines; 1 concept per file. |
| **No speculative fallbacks** | Surface explicit errors; never invent placeholder metadata. |
| **Secrets in OS Keyring** | Sensitive tokens live in Secret Service / Keychain; never plaintext. |
| **Agro wire parity** | Coordinate wire format and schema versions with Agro and Wanda specs. |
| **No dead code** | If a helper has no caller, it does not get written. |
| **Research before implementation** | Follow mandatory 4-step research workflow before adding dependencies. |
| **No AI attribution in git** | Comply with `CLA.md` Section 8; never add `Co-Authored-By` AI tags. |

## 2. Documentation Directory Map

Detailed developer guides and architectural specifications in `docs/dev/`:

| Topic | Pointer / Specification |
| --- | --- |
| **Coding Style & Rules** | [`docs/dev/process/coding-style.md`](docs/dev/process/coding-style.md) |
| **Mandatory Research Workflow** | [`docs/dev/process/research-workflow.md`](docs/dev/process/research-workflow.md) |
| **Git & Authorship Policy** | [`docs/dev/process/git-and-authorship.md`](docs/dev/process/git-and-authorship.md) |
| **Three-Thread Architecture** | [`docs/dev/architecture/three-thread-engine.md`](docs/dev/architecture/three-thread-engine.md) |
| **Agro Protocol Parity** | [`docs/dev/architecture/agro-parity.md`](docs/dev/architecture/agro-parity.md) |
| **Commands & Testing** | [`docs/dev/tools/commands-and-testing.md`](docs/dev/tools/commands-and-testing.md) |

## 3. Quick Commands

```bash
cargo build --release                    # Compile release binary
cargo test                               # Run test suite
cargo clippy --all-targets               # Lint checks
cargo fmt --check                        # Format checks
```
