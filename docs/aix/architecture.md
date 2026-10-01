# aix — architecture

## Purpose

`aix` is a standalone Rust CLI that resolves named profiles and secret sources
from configuration. It launches downstream tools, offers OpenAI-compatible and
LiteLLM gateway capabilities, provides one-shot inference, and can record
metadata-only provenance around arbitrary child processes. Nix is a deployment
mechanism and config generator, not a runtime dependency.

---

## Module boundaries

### `src/cli.rs` — CLI parsing

Owns the CLI surface: flags, subcommands, value types.  
**Rule:** contains no business logic. Parsing only.

- Global flags: `--profile`, `--config`, `--json`
- Subcommands: `init`, `use`, `current`, `profiles`, `env`, `shell`, `exec`, `config`, `spend`, `status`, `models`, `usage`, `ask`, `prompt`, `run`, `runs`, `cache`, `<tool>` (external catch-all)
- Format enum (`EnvFormat`): sh / json / nu / fish / powershell / cmd

### `src/config.rs` — Config loading and validation

Owns the config file data model and all file I/O related to config.  
**Rule:** no knowledge of secrets resolution or process launching.

| Function | Responsibility |
|---|---|
| `find_config_path()` | Discover config file: explicit path → XDG dirs |
| `load()` | Parse by extension: TOML · YAML · JSON · JSON5 |
| `validate()` | Check profile labels, model settings, prompt presets, tool names/commands/env names, and related config invariants |
| `load_env_files()` | Load `.env` files listed under `env_files`; real env wins |
| `sorted_profiles()` | Return `(name, label)` pairs sorted by name |

Config types: `Config`, `Endpoint`, `Profile`, `Provider`, `Gateway`, `CacheConfig`, `ModelConfig`, `PromptPreset`, and `Tool`.
A profile can override `endpoint.base_url` with `base_url` and append custom, secret-backed variables with `env`; those custom values can override generated variables.
`gateway` and `provider` are optional metadata for most commands, but `aix spend` and `aix usage` accept only an unset gateway or `litellm`.
`models` contains defaults and aliases. `tools.<name>` contains optional `command`, required `api_format`, and optional secret-backed `env` launch settings. `ApiFormat` is serialized as `anthropic`, `openai`, or `both` for tool entries.

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

### `src/gateway/` — Gateway capability clients

Gateway HTTP responsibilities are split between a shared transport and
capability-specific clients:

- `transport.rs` owns the normalized bare gateway URL, API key, and bounded
  `reqwest::Client`; it provides bearer-authenticated JSON request helpers and
  common HTTP error handling.
- `openai.rs` exposes OpenAI-compatible `/v1/*` endpoints.
- `litellm.rs` owns LiteLLM management endpoints such as `/key/info` and
  `/key/list`. Management data is sanitized before it is returned to command
  code, and `aix spend` and `aix status` use this client. Historical activity
  requests use the same client; `aix usage` normalizes the response into
  aix-owned daily and model aggregates and discards API-key breakdowns.
- `aix models` discovers model IDs and `aix ask` / `aix prompt` send one-shot text requests
  through the OpenAI-compatible client. `aix status` uses its models endpoint
  for a live authenticated connectivity probe.

Keep endpoint-specific behavior in the corresponding capability client rather
than growing a single catch-all gateway client.

### `src/inference.rs` — Shared one-shot inference service

Owns the common `ask`/`prompt` input collection, model and profile resolution,
env-file loading, gateway request, response parsing, and output behavior.
Command modules provide the instruction and options; they do not duplicate HTTP
or context handling.

Prompt presets are generic user configuration. They add no template language,
implicit filesystem/Git discovery, shell execution, or domain-specific dispatch.

### `src/commands/env.rs` — Credential collection and env rendering

Owns reusable profile credential collection and `aix env` output formatting.
**Rule:** credential output order is deterministic; format functions are pure.

- `resolve_profile()`: effective explicit profile → config `default_profile` → interactive TUI (TTY only); `app.rs` gives a subcommand positional profile precedence over global `--profile`/`AIX_PROFILE`
- `collect_vars()`: builds the ordered `(name, value)` list for a given `ApiFormat`
- `format_vars()`: dispatches to one of six format functions (sh / json / nu / fish / powershell / cmd)

The `env` subcommand hardcodes `ApiFormat::Both` and always emits all seven variables:

| Variable | Value |
|---|---|
| `AIX_PROFILE` | Selected profile name |
| `ANTHROPIC_API_KEY` | Resolved API key |
| `ANTHROPIC_BASE_URL` | Gateway base URL |
| `OPENAI_API_KEY` | Resolved API key |
| `OPENAI_BASE_URL` | Gateway base URL with `/v1` appended |
| `LITELLM_API_KEY` | Resolved API key |
| `LITELLM_BASE_URL` | Gateway base URL with `/v1` appended |

`collect_vars()` accepts an `ApiFormat` argument so named-tool dispatch can emit a subset:
Anthropic → 5 vars (`AIX_PROFILE` + `ANTHROPIC_*` + `LITELLM_*`), OpenAi → 5 vars (`AIX_PROFILE` + `OPENAI_*` + `LITELLM_*`).
`AIX_API_KEY` and `AIX_BASE_URL` are not generated by aix; profile or tool env may set them explicitly.

### `src/commands/launch.rs` — Process launching

Shared by `exec`, `shell`, `run`, and external named-tool dispatch.
**Rule:** resolve config and credentials, compose generated/profile/tool env in
that order, launch a child, and preserve its exit behavior. Configured tool
names select an executable and API format; unconfigured named tools retain the
legacy `claude` → Anthropic / other → OpenAI fallback. An unconfigured `aix run`
command receives both formats.

### Named-tool dispatch (`aix <tool>`) — in `src/app.rs`

`aix <tool>` is handled by the `Command::Tool` external subcommand in `app.rs`.
The launcher checks `tools.<name>` first, then applies compatibility fallback.
It does not embed harness-specific behavior beyond the legacy `claude` default.

- `aix <tool>` configured in `[tools]` → configured command, format, and env
- unconfigured `aix claude` → `ApiFormat::Anthropic`
- other unconfigured `aix <tool>` → `ApiFormat::OpenAi`

Usage: `aix <tool> [PROFILE] [--dry-run] [-- TOOL_ARGS...]`

### Managed run history — `src/commands/run.rs`, `runs.rs`, and `src/run_history.rs`

`aix run` wraps an arbitrary child with a UUID and a versioned local record.
The child receives informational `AIX_RUN_*` metadata; standard streams remain
inherited. `RunStore` owns state-directory selection, atomic record writes,
listing, and record lookup. It never captures command arguments or child
content. `aix exec` remains stateless.

### `src/error.rs` — Error types

All application errors. Uses `thiserror`.  
**Rule:** error messages must name identifiers (env var names, file paths, command strings) — never resolved secret values.

### Nix integration (`nix/home-manager.nix`)

Generates the TOML config file read by the CLI, including profile/model settings,
generic `programs.aix.prompts` presets, and `programs.aix.tools` launch wiring.
**Rule:** this layer may only generate config. No runtime dependency on Nix from Rust.

---

## Invariants

These invariants must hold across all future changes.

1. **Rust core does not know Nix paths.**  
   No `/run/secrets`, `/nix/store`, or NixOS-specific paths hardcoded in Rust. The CLI
   accepts secrets through config fields backed by literal values, env vars, files, or commands.

2. **Nix generates config; Rust reads it.**  
   Business logic lives in Rust. Nix produces the config file consumed by the CLI.

3. **Secret values must never be logged or printed unless intentionally requested.**  
   `SecretString::Debug` and `SecretString::Display` emit `[secret]`.  
   `SecretSource::Direct::Debug` emits `[redacted]`.  
   Error messages include the source identifier (env var name, file path, command string)
   but not the resolved value.

4. **Output formats must be tested.**  
    Every format function (sh, json, nu, fish, powershell, cmd) has unit tests for:
   - correct syntax for the target shell
   - proper escaping of special characters (quotes, backslashes, newlines)

5. **Gateway and tool configuration stays declarative.**
   Gateway endpoints and metadata belong in profiles/config. Tool-specific
   executable, credential format, and extra env belong in `[tools.<name>]`;
   launch logic stays generic and does not encode harness workflows.

6. **Config format is stable.**  
   TOML, YAML, JSON, and JSON5 config files must parse to identical in-memory
   representations. Any structural change to config types such as `Config`,
   `Endpoint`, `Profile`, `ModelConfig`, or `Tool` requires updating all four
   format fixtures in tests.

---

## How to add a new gateway or provider

No Rust changes are required for most provider additions.

1. Set `base_url` to the gateway URL via env var, file, or command.
2. Add a named profile under `[profiles.<name>]` with the appropriate `api_key`.
3. Optionally set `provider` and `gateway` as metadata. Set `gateway = "litellm"` (or leave it unset) when the profile will be used with `aix spend` or `aix usage`; other gateway values are rejected by those commands.

`aix env` and `aix exec` always emit Anthropic, OpenAI, and LiteLLM credential sets.
Configured tools select a format in `[tools.<name>]`; unconfigured named tools
retain the `claude`/OpenAI compatibility defaults. Tool env overrides profile
env, which overrides generated credentials.

If a new credential format is needed, update `ApiFormat`, its config
deserialization and Home Manager enum, `collect_vars()`, and the corresponding
tests. Do **not** add a new subcommand or provider-specific branch to process
launching.

---

## Do not

| Rule | Reason |
|---|---|
| Do not hardcode `/run/secrets` or any Nix path in Rust | The CLI must work without Nix at runtime |
| Do not hardcode profile names (e.g. `"swtb"`, `"work"`) in Rust | Profile names are user config, not code |
| Do not hand-roll JSON | Use `serde_json`; malformed JSON in env output breaks downstream consumers |
| Do not auto-load `.env` files by default | The user must opt in via `env_files` in config; silent auto-loading causes surprising precedence behaviour |
| Do not print secret values | Any log, debug output, or error message that contains a resolved secret is a security defect |
| Do not add harness-specific workflow logic to launch commands | `[tools]` describes executable, credential format, and env only; routing and records stay generic |
| Do not add a new subcommand for a new AI tool unless it needs special argv construction | Use `exec` instead |

---

## Testing requirements

Before adding a feature, verify that:

- Config parsing tests cover all four formats (TOML, YAML, JSON, JSON5) and assert
  identical effective behaviour.
- Every new `ApiFormat` variant has a `collect_vars` unit test.
- Every new format function has escape-sequence unit tests (currently six: sh, json, nu, fish, powershell, cmd).
- Secret values never appear in non-env output (`exec --dry-run`, `profiles`, error messages).
