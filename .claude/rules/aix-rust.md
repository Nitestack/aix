---
paths:
  - "tools/aix/**"
  - "crates/aix/**"
  - "Cargo.toml"
  - "Cargo.lock"
---

# aix Rust CLI — conventions

## Project intent

The Rust CLI replaces ad-hoc shell scripts that called a LiteLLM-compatible AI gateway API.
It must be self-contained and runnable outside Nix.

## Structure

- `src/main.rs` — entry point, arg parsing only
- `src/config.rs` — profile/endpoint resolution
- `src/client.rs` — HTTP client, streaming, retry
- `src/secrets.rs` — secret resolution (env → file → store flag)
- `src/commands/` — one module per subcommand

Keep modules small. No god files.

## Config model

Profiles are named sets of `(base_url, model, default_params)`.
The active profile is chosen by `--profile` flag or `AIX_PROFILE` env var.
Never embed a URL or model name as a compile-time constant in command logic.

Example config shape (TOML):
```toml
[profiles.default]
base_url = "https://ai.example.com/v1"
model    = "gpt-4o"

[profiles.fast]
base_url = "https://ai.example.com/v1"
model    = "gpt-4o-mini"
```

## Secret resolution order

1. Env var (e.g. `AIX_API_KEY`)
2. Config file field `api_key` (path from `--config` or `AIX_CONFIG`)
3. External store via `--secret-cmd <cmd>` (stdout of the command is the key)

The CLI must work if only step 1 is satisfied. Never require Nix secret tooling at runtime.

## Error handling

Use `color-eyre` for application-level errors in `main()` — it produces beautiful terminal output with span traces. Use `thiserror` for library-facing error types in `error.rs`. Do not use `anyhow`.
Emit structured errors to stderr; streaming output to stdout only.

## Testing

- Unit tests for config parsing and secret resolution.
- Integration tests (behind `#[cfg(test)]` feature or separate binary) may hit a mock server.
- Do not require a live AI Hub endpoint in CI.

## Cross-platform

- No `std::os::unix` imports in non-platform-gated code.
- Paths via `std::path::PathBuf`, never string concatenation.
- Avoid crates with heavy C build dependencies unless strictly necessary.

## Formatting / linting

Run before any commit:
```sh
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```
