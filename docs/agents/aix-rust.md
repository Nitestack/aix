# aix Rust CLI conventions

Apply this guidance to changes in `src/**`, `tests/**`, `Cargo.toml`, and `Cargo.lock`.

## Project intent

The Rust CLI replaces ad-hoc shell scripts that called a LiteLLM-compatible AI gateway API.
It must be self-contained and runnable outside Nix.

## Structure

- `src/main.rs` — module list and the tokio entry point; nothing else
- `src/cli.rs` — clap definitions (global flags, subcommands)
- `src/app.rs` — error-report setup, dispatch to command modules, exit codes
- `src/config.rs` — config loading (TOML/YAML/JSON/JSON5), profile and endpoint resolution
- `src/secrets.rs` — resolve literal, env, file, and command secret sources
- `src/error.rs` — `AixError` (thiserror) and exit-code mapping
- `src/output.rs` — shared `--json` envelope and human output helpers
- `src/gateway/` — shared HTTP transport and gateway capability clients (models, budget)
- `src/auth/` — ChatGPT OAuth protocol and the owner-only auth store
- `src/local_gateway/` — per-launch loopback gateways (API-key, ChatGPT Responses); the OpenCode bridge is wired in `commands/launch/opencode.rs`
- `src/usage_observer/`, `src/usage_event.rs`, `src/usage_store.rs` — token-usage parsing and the local usage-event store
- `src/run_history.rs` — metadata-only managed-run records
- `src/cache.rs` — local response cache for gateway reads
- `src/inference.rs` — shared request path for `aix ask` and `aix prompt`
- `src/herdr.rs` — herdr pane labels
- `src/commands/` — one module (or directory) per subcommand; `launch` is shared by `aix <tool>` and `aix run`

See `ARCHITECTURE.md` at the repo root for boundaries and invariants.
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
- Do not require a live gateway in CI; use `wiremock` for HTTP.
- Checks that need real tools or accounts go in `tests/manual/` as written procedures.

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
