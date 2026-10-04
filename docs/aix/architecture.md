# aix — architecture

## Purpose

`aix` is a standalone Rust CLI that resolves named profiles and secret sources
from configuration. It launches downstream tools, offers OpenAI-compatible and
LiteLLM gateway capabilities, provides one-shot inference, manages ChatGPT OAuth
sign-in state, and can record metadata-only provenance around arbitrary child
processes. Nix is a deployment mechanism and config generator, not a runtime
dependency.

---

## Module boundaries

### `src/cli.rs` — CLI parsing

Owns the CLI surface: flags, subcommands, value types.  
**Rule:** contains no business logic. Parsing only.

- Global flags: `--profile`, `--config`, `--json`
- Subcommands: `init`, `use`, `current`, `profiles`, `env`, `shell`, `exec`, `config`, `auth`, `spend`, `status`, `models`, `usage`, `ask`, `prompt`, `run`, `runs`, `cache`, `<tool>` (external catch-all)
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

Config types: `Config`, `Endpoint`, `Profile`, `ProfileAuth`, `Provider`, `Gateway`, `CacheConfig`, `ModelConfig`, `PromptPreset`, and `Tool`. A profile has exactly one auth source: a legacy `api_key` or `auth = { type = "chatgpt" }`. ChatGPT profiles may omit the shared endpoint and cannot set a profile `base_url`.
A profile can override `endpoint.base_url` with `base_url` and append custom, secret-backed variables with `env`; those custom values can override generated variables.
`gateway` and `provider` are optional metadata for most commands, but `aix spend` and `aix usage` accept only an unset gateway or `litellm`.
`models` contains defaults and aliases. `tools.<name>` contains optional `command`, required `api_format`, and optional secret-backed `env` launch settings. Its optional `chatgpt` block declares the access-token environment name, prepended arguments, and inherited variables to clear. `ApiFormat` is serialized as `anthropic`, `openai`, or `both` for tool entries.

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

### `src/auth/` and `src/commands/auth.rs` — ChatGPT auth lifecycle

`AuthService` owns sign-in, offline status, logout, and the access-token refresh
path. `protocol.rs` builds the PKCE authorization request, validates the loopback
callback, exchanges authorization codes, verifies ID-token signatures and
claims from OpenID metadata/JWKS, discovers revocation, and refreshes tokens.
`store.rs` writes versioned, profile-scoped records atomically and uses file
locks to serialize login/logout/refresh across processes. `launch.rs` asks this
service for a usable access token only for an explicitly ChatGPT-bound tool.
Ordinary bound tools receive that access token at launch; OpenCode's dedicated
bridge asks `AuthService` for a token per upstream request instead.

The store lives under the platform local application-data directory (or
`AIX_AUTH_DIR`). On Unix, the directory and files are restricted to the owner
(0700/0600). OAuth tokens are persisted as local JSON and are not encrypted at
rest. The status command reads only local state and never returns token values.
The ordinary launch boundary exposes only the short-lived access token, under
the configured `access_token_env`; refresh tokens, ID tokens, client IDs, and
tokens for other profiles remain in the auth service/store. The OpenCode child
instead receives only a random bridge bearer token; the ChatGPT access token is
added to a request inside aix and is never placed in the child environment. No
OAuth token is exported through `env`, `shell`, generic `exec`, inference, or an
unconfigured tool.
`aix run` uses the same explicit binding, but lease and run-policy paths reject
ChatGPT profiles before reaching LiteLLM.

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

For API-key profiles, `env` hardcodes `ApiFormat::Both` and emits all seven
variables. ChatGPT profiles are rejected rather than exporting OAuth tokens:

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
**Rule:** resolve config and credentials, compose the appropriate environment,
launch a child, and preserve its exit behavior. API-key launches keep the
generated/profile/tool precedence and legacy named-tool fallbacks. ChatGPT
launches require `tools.<name>.chatgpt`, clear standard inherited API-key
variables and configured `clear_env`, then apply profile env and tool env. Most
bound tools receive the selected access token last; ChatGPT-authenticated
OpenCode instead uses the `opencode_sidecar` lifecycle described below.
ChatGPT prepended arguments come before caller arguments. Dry-run reports
arguments and variable names without starting the bridge or loading/refreshing
OAuth credentials. An unconfigured API-key `aix run` command receives both
credential formats.

### `src/commands/launch/opencode.rs` — OpenCode SIWC bridge

This narrow adapter is selected only for the logical `opencode` tool with a
ChatGPT profile. It binds an ephemeral listener to `127.0.0.1`, generates a
per-launch bridge secret, and supplies process-local OpenCode v2 configuration
for a dedicated `aix-chatgpt` Responses provider. It accepts only authenticated
`POST /v1/responses` and `GET /v1/models` requests and fixes the upstream base to
`https://api.openai.com/v1/`. The real access token is fetched from
`AuthService` for every upstream request; request/response streaming and the
current SIWC request restrictions are handled here rather than in generic
launch code. Dropping the sidecar cancels the listener and in-flight requests.
It writes no OpenCode configuration, credential, session, or project files and
does not implement a general proxy or model-catalog synchronizer.

### Named-tool dispatch (`aix <tool>`) — in `src/app.rs`

`aix <tool>` is handled by the `Command::Tool` external subcommand in `app.rs`.
The launcher checks `tools.<name>` first, then applies compatibility fallback.
The only harness-specific behavior is the narrow OpenCode SIWC sidecar above.

- API-key profile + configured `[tools.<name>]` → configured command, format, and env
- API-key profile + unconfigured `aix claude` → `ApiFormat::Anthropic`
- API-key profile + other unconfigured `aix <tool>` → `ApiFormat::OpenAi`
- ChatGPT profile + configured non-OpenCode tool → explicit access-token handoff
- ChatGPT profile + configured `opencode` tool → local bridge credential only
- ChatGPT profile + missing binding → capability error; no API-key fallback

Usage: `aix <tool> [PROFILE] [--dry-run] [-- TOOL_ARGS...]`

### Managed run history — `src/commands/run.rs`, `runs.rs`, and `src/run_history.rs`

`aix run` wraps an arbitrary child with a UUID and a versioned local record.
The child receives informational `AIX_RUN_*` metadata; standard streams remain
inherited. A configured tool's prepend arguments and ChatGPT binding are also
used by plain managed runs. OAuth tokens are never recorded. `RunStore` owns
state-directory selection, atomic record writes, listing, and record lookup. It
never captures command arguments or child content. `aix exec` remains stateless.

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
    but not the resolved value. ChatGPT OAuth tokens are intentionally persisted
    in the owner-only auth store, but never printed or exported.

4. **Output formats must be tested.**  
    Every format function (sh, json, nu, fish, powershell, cmd) has unit tests for:
   - correct syntax for the target shell
   - proper escaping of special characters (quotes, backslashes, newlines)

5. **Gateway and tool configuration stays declarative.**
   Gateway endpoints and metadata belong in profiles/config. Tool-specific
   executable, credential format, and extra env belong in `[tools.<name>]`;
   launch logic stays generic except for the narrowly scoped OpenCode SIWC
   sidecar, which must not grow into a general harness proxy framework.

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

For API-key profiles, `aix env` and `aix exec` emit Anthropic, OpenAI, and
LiteLLM credential sets. ChatGPT profiles are rejected by those commands rather
than exporting OAuth tokens. Configured tools select a format in `[tools.<name>]`
for API-key profiles; unconfigured named tools retain the `claude`/OpenAI
compatibility defaults. ChatGPT launches require an explicit auth binding.
Tool env overrides profile env, which overrides generated credentials for
API-key launches.

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
