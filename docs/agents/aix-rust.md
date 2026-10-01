# aix Rust CLI conventions

Apply this guidance to changes in `src/**`, `tests/**`, `Cargo.toml`, and `Cargo.lock`.

## Project intent

The Rust CLI replaces ad-hoc shell scripts that called a LiteLLM-compatible AI gateway API.
It must be self-contained and runnable outside Nix.

## Structure

- `src/main.rs` — entry point, arg parsing only
- `src/config.rs` — profile/endpoint resolution
- `src/gateway/` — shared HTTP transport and gateway capability clients
- `src/secrets.rs` — resolve configured literal, environment, file, or command sources
- `src/commands/` — one module per subcommand

Keep modules small. No god files.

## Config model

Profiles are named sets of `(label, api_key)`. Gateway config (`base_url`) lives in the `[endpoint]` block.
For profile-resolving commands, a positional profile overrides global `--profile`/`AIX_PROFILE`,
which overrides `default_profile`; interactive selection is the TTY-only final fallback.
Never embed a URL, key, or profile name as a compile-time constant in command logic.

`api_format` is a launch setting only: it may appear under `[tools.<name>]`, not on a profile or
endpoint. `aix env`/`aix exec` always emit all seven credential vars (Anthropic, OpenAI, and
LiteLLM-named sets). Both `aix <tool>` and `aix run -- <tool>` use configured launch wiring when
the logical tool name matches `[tools.<name>]`. An unconfigured `aix run` command receives all
seven vars. The legacy unconfigured `aix claude` form emits Anthropic + LiteLLM-named vars; other
unconfigured named tools emit OpenAI + LiteLLM-named vars. `LITELLM_API_KEY`/`LITELLM_BASE_URL` are
always emitted — they alias the same credential as the OpenAI-named vars.

Tool entries are generic launch wiring: `command` is optional and defaults to the tool name,
`api_format` is `anthropic`, `openai`, or `both`, and optional `env` entries use the same secret
sources as profile env. Launch env precedence is generated credentials, profile env, then tool env.

Example config shape (TOML):
```toml
[endpoint]
base_url = { env = "AIX_BASE_URL" }

[profiles.work]
label   = "Work"
api_key = { env = "AIX_WORK_KEY" }

[profiles.personal]
label   = "Personal"
api_key = { file = "/run/secrets/aix/personal" }
```

## Secret sources

The CLI resolves each configured secret source independently: a literal string,
`{ env = "VAR" }`, `{ file = "/path" }`, or `{ command = "..." }`. Config path
selection uses `--config` or `AIX_CONFIG`; values such as `AIX_API_KEY` are not
special implicit overrides. Never require Nix secret tooling at runtime.

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
