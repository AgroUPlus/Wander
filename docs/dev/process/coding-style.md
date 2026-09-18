# Coding Style & Architecture Rules

## 1. File Length & Modularity

- **Hard cap 300 lines per file.** Split when a file passes 250 lines.
- One concept per file.
- Default to private (`pub(crate)` or unexported). Use `pub` only for genuine cross-crate or public library API.

## 2. Robustness & Error Handling

- **No speculative fallbacks.** If a stream or local file fails to decode, surface an explicit error to the user.
- Never invent placeholder metadata.
- Never swallow errors with blanket ignores.
- **No dead code.** If a helper or feature has no caller, it does not get written.

## 3. Dependencies & Safety

- Keep dependencies minimal and audit audio decoders for memory safety.
- Audio and buffer allocations must happen ahead-of-time outside the realtime processing loop.
