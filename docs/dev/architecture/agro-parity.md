# Agro Parity & Credential Storage

## 1. Agro Protocol Synchronization

- Coordinate wire formats and protocol versions with `Agro` and `Wanda` specifications.
- Integration endpoints and models are implemented in `src/integrations/agro*.rs`.
- Wire schemas, event payloads, and presence updates must remain strictly in step with the Agro daemon.

## 2. Secure Credential Storage

- Passwords and sensitive Agro tokens are stored in the system OS keyring via Secret Service (Linux) or Keychain (macOS).
- Sensitive tokens must never be written to plaintext configuration files, debug dumps, or console logs.
