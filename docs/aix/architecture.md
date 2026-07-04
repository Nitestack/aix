# aix — architecture

## Purpose

`aix` is a standalone Rust CLI that resolves named profiles from a config file
and exports the corresponding environment variables so that downstream AI tools
(`claude`, `pi`, etc.) can reach the gateway without per-tool configuration.
It replaces inline Nix shell scripts and must work on any OS — Nix is a
deployment mechanism, not a runtime dependency.

---

## Module boundaries

### `src/cli.rs` — CLI parsing

Owns the CLI surface: flags, subcommands, value types.  
**Rule:** contains no business logic. Parsing only.

- Global flags: `--profile`, `--config`
- Subcommands: `profiles`, `env`, `shell`, `exec`, `config`, `<tool>` (external catch-all)
- Format enum (`EnvFormat`): sh / json / nu / fish / powershell / cmd

### `src/config.rs` — Config loading and validation

Owns the config file data model and all file I/O related to config.  
**Rule:** no knowledge of secrets resolution or process launching.

| Function | Responsibility |
|---|---|
| `find_config_path()` | Discover config file: explicit path → XDG dirs |
| `load()` | Parse by extension: TOML · YAML · JSON · JSON5 |
| `validate()` | Check invariants: default_profile exists, no duplicate labels |
| `load_env_files()` | Load `.env` files listed under `env_files`; real env wins |
| `sorted_profiles()` | Return `(name, label)` pairs sorted by name |

Config types: `Config`, `Endpoint`, `Profile`, `Provider`, `Gateway`.  
`Provider` and `Gateway` are metadata only — they do not affect runtime behavior.  
`ApiFormat` exists as an internal code enum used by `collect_vars()` but is not a config-file field.

### `src/secrets.rs` — Secret resolution

Owns the `SecretSource` enum and `SecretString` wrapper.  
**Rule:** resolved values must never appear in `Debug`, `Display`, or error messages.

| Secret source | Specified as (TOML) |
|---|---|
| `SecretSource::Direct` | `api_key = "literal"` |
| `SecretSource::Env` | `api_key = { env = "VAR" }` |
| `SecretSource::File` | `api_key = { file = "/path" }` (tilde expanded) |
| `SecretSource::Command` | `api_key = { command = "op read ..." }` (stdout) |

`SecretString` zeroes its memory on drop. Its `Debug` and `Display` impls
emit `[secret]` — never the actual value.

### `src/commands/env.rs` — Profile resolution and env rendering

The `env` subcommand is the core of the tool.  
**Rule:** collect_vars output order is deterministic; format functions are pure.

- `resolve_profile()`: positional arg → config `default_profile` → interactive TUI (TTY only)
- `collect_vars()`: builds the ordered `(name, value)` list for a given `ApiFormat`
- `format_vars()`: dispatches to one of six format functions (sh / json / nu / fish / powershell / cmd)

The `env` subcommand hardcodes `ApiFormat::Both` and always emits all five variables:

| Variable | Value |
|---|---|
| `AIX_PROFILE` | Selected profile name |
| `ANTHROPIC_API_KEY` | Resolved API key |
| `ANTHROPIC_BASE_URL` | Gateway base URL |
| `OPENAI_API_KEY` | Resolved API key |
| `OPENAI_BASE_URL` | Gateway base URL with `/v1` appended |

`collect_vars()` accepts an `ApiFormat` argument so named-tool dispatch can emit a subset:
Anthropic → 3 vars (`AIX_PROFILE` + `ANTHROPIC_*`), OpenAi → 3 vars (`AIX_PROFILE` + `OPENAI_*`).
`AIX_API_KEY` and `AIX_BASE_URL` are never emitted.

### `src/commands/launch.rs` — Process launching

Shared by `exec`, `shell`, `claude`, `pi`.  
**Rule:** sets env vars, then replaces the current process (Unix: `execvp`, Windows: child + wait).  
No knowledge of which AI tool is being launched beyond its argv.

### Named-tool dispatch (`aix <tool>`) — in `src/main.rs`

`aix <tool>` is handled by the `Command::Tool` external subcommand in `main.rs`.  
**Rule:** no tool-specific env var logic outside this dispatch point.

- `aix claude` → `ApiFormat::Anthropic` (emits `ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL`)
- `aix <anything-else>` → `ApiFormat::OpenAi` (emits `OPENAI_API_KEY`, `OPENAI_BASE_URL` + `/v1`)

Usage: `aix <tool> [PROFILE] [--dry-run] [-- TOOL_ARGS...]`

### `src/error.rs` — Error types

All application errors. Uses `thiserror`.  
**Rule:** error messages must name identifiers (env var names, file paths, command strings) — never resolved secret values.

### Nix integration (`modules/home/aix.nix`)

Generates the TOML config file read by the CLI.  
**Rule:** this layer may only generate config. No runtime dependency on Nix from Rust.

---

## Invariants

These invariants must hold across all future changes.

1. **Rust core does not know Nix paths.**  
   No `/run/secrets`, `/nix/store`, or NixOS-specific paths hardcoded in Rust. The CLI
   accepts secrets via env var, config file field, or `--secret-cmd`.

2. **Nix generates config; Rust reads it.**  
   Business logic lives in Rust. Nix produces the config file consumed by the CLI.

3. **Secret values must never be logged or printed unless intentionally requested.**  
   `SecretString::Debug` and `SecretString::Display` emit `[secret]`.  
   `SecretSource::Direct::Debug` emits `[redacted]`.  
   Error messages include the source identifier (env var name, file path, command string)
   but not the resolved value.

4. **Output formats must be tested.**  
   Every format function (sh, json, nu, fish, powershell) has unit tests for:
   - correct syntax for the target shell
   - proper escaping of special characters (quotes, backslashes, newlines)

5. **New providers and gateways are config-level additions only.**  
   Adding a new gateway or provider does not require any change to `launch.rs`,
   `exec.rs`, `claude.rs`, or `pi.rs`. The profile contains all needed values;
   routing uses profiles resolved at runtime.

6. **Config format is stable.**  
   TOML, YAML, JSON, and JSON5 config files must parse to identical in-memory
   representations. Any structural change to `Config`/`Endpoint`/`Profile` requires
   updating all four format examples in tests.

---

## How to add a new gateway or provider

No Rust changes are required for most provider additions.

1. In the user's config, set `provider` and `gateway` to document the new gateway
   (metadata only — does not affect runtime behavior).
2. Set `base_url` to the gateway URL via env var, file, or command.
3. Add a named profile under `[profiles.<name>]` with the appropriate `api_key`.

`aix env` and `aix exec` always emit both Anthropic and OpenAI credential sets.
`aix claude` emits Anthropic-only; `aix <other-tool>` emits OpenAI-only.

If the new gateway requires env var names beyond the current two sets,
add a variant to `ApiFormat` in `src/config.rs` and a corresponding branch in
`collect_vars()`. Do **not** add a new subcommand or a conditional branch in
`launch.rs`/`exec.rs`.

---

## Do not

| Rule | Reason |
|---|---|
| Do not hardcode `/run/secrets` or any Nix path in Rust | The CLI must work without Nix at runtime |
| Do not hardcode profile names (e.g. `"swtb"`, `"work"`) in Rust | Profile names are user config, not code |
| Do not hand-roll JSON | Use `serde_json`; malformed JSON in env output breaks downstream consumers |
| Do not auto-load `.env` files by default | The user must opt in via `env_files` in config; silent auto-loading causes surprising precedence behaviour |
| Do not print secret values | Any log, debug output, or error message that contains a resolved secret is a security defect |
| Do not add provider-specific logic to subcommands | Format selection for named tools (`claude` → Anthropic, others → OpenAI) lives only in `main.rs`; `exec.rs` and `launch.rs` are format-agnostic |
| Do not add a new subcommand for a new AI tool unless it needs special argv construction | Use `exec` instead |

---

## Testing requirements

Before adding a feature, verify that:

- Config parsing tests cover all four formats (TOML, YAML, JSON, JSON5) and assert
  identical effective behaviour.
- Every new `ApiFormat` variant has a `collect_vars` unit test.
- Every new format function has escape-sequence unit tests (currently six: sh, json, nu, fish, powershell, cmd).
- Secret values never appear in non-env output (`exec --dry-run`, `profiles`, error messages).
