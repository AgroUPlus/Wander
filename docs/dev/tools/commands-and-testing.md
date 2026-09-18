# Cargo Commands & Verification

## 1. Primary Commands

```bash
cargo build --release                    # Compile optimized release binary
cargo test                               # Execute unit and integration tests
cargo clippy --all-targets               # Check clippy linter warnings
cargo fmt --check                        # Validate code formatting
```

## 2. Verification Workflow for AI Agents

- Use real-time language server diagnostics (`rust-analyzer`) or `cargo check` before running comprehensive test suites.
- Run `cargo test`, `cargo clippy --all-targets`, and `cargo fmt --check` before concluding work.
