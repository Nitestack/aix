# Current `aix` Behavior — Rust CLI

This document describes the behavior of the `aix` Rust CLI as of the current codebase.
The old Nix shell-script implementation has been superseded; see `docs/aix/migration.md` for the transition notes.

---

## Commands

| Invocation | Behavior |
|---|---|
| `aix profiles` | List available profiles (name + label) |
| `aix profiles --json` | Emit the shared JSON envelope with profile names and labels (no secrets) |
| `aix init <SHELL>` | Print shell integration for sh, bash, zsh, fish, nu, or PowerShell |
| `aix use PROFILE [--shell SHELL]` | Emit a shell assignment to select a profile without resolving secrets |
| `aix use --clear [--shell SHELL]` | Emit a shell command to unset `AIX_PROFILE` |
| `aix current [--format short] [--json]` | Show `AIX_PROFILE`, then `default_profile`, or `none`; never prompt |
| `aix env [PROFILE] [--format FORMAT]` | Print API-key profile vars (all 7); reject ChatGPT profiles without exporting OAuth tokens |
| `aix shell [PROFILE] [--dry-run]` | Launch the detected shell with API-key profile env; reject ChatGPT profiles |
| `aix exec [PROFILE] [--dry-run] -- CMD...` | Run a command with API-key profile env; reject ChatGPT profiles |
| `aix <tool> [PROFILE] [--dry-run] [-- args...]` | Run a configured tool; ChatGPT profiles require an explicit auth binding, while API-key profiles retain the legacy fallback |
| `aix config path` | Print the resolved config file path |
| `aix config validate` | Validate the config file and exit |
| `aix auth login [PROFILE]` | Sign in to a ChatGPT-authenticated profile using the system browser |
| `aix auth status [PROFILE] [--json]` | Show local auth state without network access or token values |
| `aix auth logout [PROFILE]` | Revoke the ChatGPT refresh token when possible and always clear local tokens |
| `aix spend [PROFILE] [--json] [--no-cache]` | Show LiteLLM spend and budget information |
| `aix status [PROFILE] [--json] [--refresh]` | Probe gateway connectivity and report profile and spend status |
| `aix models [PROFILE] [--filter TEXT] [--json]` | Discover sorted model IDs from `/v1/models` |
| `aix usage [PROFILE] [--since Nd | --start DATE --end DATE] [--model MODEL] [--json]` | Show historical LiteLLM spend, token, request, and model usage |
| `aix ask [--model MODEL] [--system TEXT] [--file PATH]... [PROMPT]` | Send one instruction and explicit context to the gateway |
| `aix prompt NAME [--model MODEL] [--file PATH]...` | Run a configured reusable prompt preset |
| `aix prompt --list [--json]` | List preset names and model metadata without printing preset bodies |
| `aix run [OPTIONS] -- CMD...` | Run a child with a run ID and durable, metadata-only local history |
| `aix runs [--limit N] [--json]` | List newest run records without contacting the gateway |
| `aix runs show RUN_ID [--json]` | Show one complete non-secret run record |
| `aix cache clear` | Delete all cached gateway-response files |

---

## Profile resolution

Profiles are defined in the TOML config file (see `docs/aix/example-config.toml`).

For commands with a positional profile, **selection order** is:

1. Positional profile argument on the subcommand
2. Global `--profile NAME` (defaults from the `AIX_PROFILE` environment variable)
3. `default_profile` key in the config file
4. Interactive picker via `inquire::Select` — only when both stdin and stdout are a TTY

If none applies and stdin/stdout are not both TTYs, profile-resolving commands exit
with an error. `aix ask` and `aix prompt` use the global selection only (no positional profile).
`aix current` never prompts: it reports a valid `AIX_PROFILE`, then a valid
`default_profile`, otherwise `none`. `aix use` validates profile existence without
selecting a profile or resolving credentials.

Prompt presets are user-defined entries under `[prompts.<name>]`, with a required
`prompt` instruction and optional `system` and `model` fields. Model selection
is CLI override → preset model → profile default → global default. Preset
execution shares `aix ask`'s inference and input pipeline; it reads only stdin
and files explicitly supplied with `--file`. Listing is sorted and does not
expose prompt or system text. There are no built-in presets or project/Git
discovery behaviors.

### ChatGPT-authenticated profiles

Profiles may use either the legacy `api_key` field or
`auth = { type = "chatgpt" }`; exactly one is required. ChatGPT profiles do not
need a gateway endpoint and may not set a profile-specific `base_url`. Login
uses a loopback callback, OAuth state, PKCE, and verified ID-token claims. The
issued client registration and OAuth tokens are stored per profile under the
platform application-data directory (`AIX_AUTH_DIR` overrides the location).
Unix storage uses mode `0700` for the directory and `0600` for files; tokens are
not encrypted at rest.

`aix auth status` is offline and reports only non-secret state. `aix auth
logout` clears local tokens even if remote revocation is unavailable. A ChatGPT
profile can launch only a configured tool with `[tools.<name>.chatgpt]`. Existing
bindings remain direct by default, except ChatGPT-authenticated OpenCode retains
its existing local bridge. Codex app-server can opt into `transport =
"local_gateway"`; compatible runs receive only a random per-launch local
credential, and aix gets the real access token from `AuthService` for each
upstream inference request. These gateway-backed processes can cross token
refresh without restarting. Other tools remain on direct token handoff and may
need a restart after their access token expires. `aix env`, `shell`, `exec`,
`ask`, and unconfigured tools do not receive OAuth tokens. LiteLLM leases and
run policies remain API-key-only. No child receives a refresh token, ID token,
or issued client ID, and run history remains metadata-only.

For a ChatGPT-authenticated `opencode` tool, aix launches OpenCode v2 with a
process-local `aix-chatgpt` Responses provider configured through
`OPENCODE_CONFIG_CONTENT`. The provider targets the shared loopback-only,
ephemeral Responses bridge with a random local bearer token. The bridge accepts only
`POST /v1/responses` and `GET /v1/models`, forwards only to the public
`https://api.openai.com/v1/` API, streams SSE, and applies the current SIWC
preview request constraints while recording metadata-only local usage. Codex
app-server uses one-shot provider overrides for the loopback URL and local
credential. Neither integration edits the tool's config, auth, session, or
project files. Dry-run starts no listener and reads/refreshes no credentials. See
[ChatGPT Responses local gateway](chatgpt-local-gateway.md) for the Codex
configuration and compatibility check.

---

## Environment variables emitted

For API-key profiles, `aix env` and `aix exec` always emit **all seven**
variables (`ApiFormat::Both`). ChatGPT profiles fail with an explicit
unsupported-auth error rather than exporting OAuth credentials:

| Variable | Value |
|---|---|
| `AIX_PROFILE` | Selected profile name |
| `ANTHROPIC_API_KEY` | Resolved API key |
| `ANTHROPIC_BASE_URL` | Gateway base URL |
| `OPENAI_API_KEY` | Resolved API key |
| `OPENAI_BASE_URL` | Gateway base URL with `/v1` appended |
| `LITELLM_API_KEY` | Resolved API key |
| `LITELLM_BASE_URL` | Gateway base URL with `/v1` appended |

For API-key profiles, named-tool dispatch includes `AIX_PROFILE` and the
LiteLLM-named credential pair. A configured `[tools.<name>]` entry selects
`api_format` and may add tool env; otherwise the historical `claude`/OpenAI
fallback remains. ChatGPT-bound launches do not generate any API-key variables.

| Command | Format | Variables emitted |
|---|---|---|
| Configured `aix <tool>` with API-key profile | `tools.<name>.api_format` | `AIX_PROFILE`, selected credential variables, then profile env and tool env |
| Unconfigured `aix claude` | Anthropic | `AIX_PROFILE`, `ANTHROPIC_*`, `LITELLM_*` |
| Other unconfigured `aix <tool>` | OpenAI | `AIX_PROFILE`, `OPENAI_*`, `LITELLM_*` |
| Unconfigured `aix run -- CMD` | Both | All seven generated credential variables, then profile env |
| Configured direct tool with ChatGPT profile | `[tools.<name>.chatgpt]` (default transport) | `AIX_PROFILE`, profile/tool env, then only the declared access-token variable |
| Configured `opencode` tool with ChatGPT profile | `[tools.opencode.chatgpt]` | `AIX_PROFILE`, profile/tool env, process-local config, and a random local bridge token; no ChatGPT token |
| Configured Codex app-server with ChatGPT local gateway | `[tools.codex.chatgpt]` with `transport = "local_gateway"` | `AIX_PROFILE`, profile/tool env, dynamic provider overrides, and only a random local bearer in the declared token variable |
| Tool lacking a ChatGPT binding | — | Rejected for ChatGPT profile; no API-key fallback or token handoff |

For API-key configured tools, the precedence is generated credentials → profile
env → tool env. For ChatGPT tools, aix removes inherited standard API-key
variables and `clear_env`, then applies profile env → tool env. Ordinary tools
receive the selected access token last. Gateway-backed tools clear their
configured token variable and standard API-key variables, then receive only a
local bridge token and process-local provider configuration. `aix run` uses the
configured tool's `command`, prepended arguments, and auth binding. `AIX_API_KEY`
and `AIX_BASE_URL` are not generated by aix; custom env entries may set them
explicitly.

---

## Output formats (`aix env --format`)

| Format | Syntax |
|---|---|
| `sh` (default) | `export VAR='value'` |
| `json` | JSON object `{"VAR": "value"}` |
| `nu` | `$env.VAR = "value"` |
| `fish` | `set -x VAR 'value'` |
| `powershell` | `$env:VAR = 'value'` |
| `cmd` | `set "VAR=value"` |

---

## Secret sources

All secret-backed fields (`api_key`, `base_url`, profile `env`, and tool `env` values) accept one of these forms. Profile `label` supports the same dynamic forms. `tools.<name>.chatgpt.access_token_env` is a variable name, not a configured token value. Ordinary tools receive the selected profile's access token there; for `opencode`, `AIX_OPENCODE_BRIDGE_TOKEN` names the local bridge credential instead.

| TOML form | Resolved from |
|---|---|
| `"literal"` | Literal string (avoid for secrets — appears in config file) |
| `{ env = "VAR" }` | Environment variable |
| `{ file = "/run/secrets/..." }` | File contents (trailing newline stripped) |
| `{ command = "op read ..." }` | stdout of a shell command (trailing newline stripped) |

A profile can set `base_url` to override the shared endpoint and an `[profiles.<name>.env]` table to append custom variables. `[tools.<name>]` entries may set `command`, `api_format` (`anthropic`, `openai`, or `both`), and `env`. Variable names must be valid shell environment identifiers. Launch precedence is generated credentials, profile env, then configured tool env.

---

## Config file discovery

1. `--config PATH` flag (or `AIX_CONFIG` env var)
2. XDG config dir: `~/.config/aix/aix.{toml,yaml,yml,json,json5}` (first found)

TOML, YAML, JSON, and JSON5 are all supported.

---

## Shell detection (`aix shell`)

Shell detected in order:

1. `$SHELL` env var (if non-empty)
2. `nu` if `$NU_VERSION` is set and `nu` is on `$PATH` (Unix only)
3. `sh` fallback (Unix) / `pwsh` or `cmd` (Windows)

`aix init <SHELL>` generates wrappers for `sh`, `bash`, `zsh`, `fish`, `nu`, and
PowerShell. After initialization, only the selected profile name is stored in
the parent shell's `AIX_PROFILE`; credentials remain process-local. `aix current`
is non-interactive and does not resolve secrets.

---

## Managed run records

`aix run` generates a UUID run ID and passes it to the child as `AIX_RUN_ID`.
Optional name, workflow, task ID, and tags are passed as `AIX_RUN_NAME`,
`AIX_WORKFLOW`, `AIX_TASK_ID`, and JSON-array `AIX_RUN_TAGS`. `aix runs` lists
records newest-first without network access; `aix runs show RUN_ID` displays one
record. `AIX_STATE_DIR` overrides the platform state directory.

Records contain the profile, logical tool and executable names, timing, exit
status, and caller-supplied metadata. They never contain command arguments,
prompts, stdin/stdout/stderr, API keys, base URLs, OAuth tokens, or secret env
values. Writes are atomic. `aix exec` remains unrecorded.

---

## JSON output

Commands with JSON support return the versioned aix-owned envelope
`{ "schema_version": 1, "command": "...", "data": ... }`. The global
`--json` flag is supported by `current`, `profiles`, `spend`, `status`, `models`,
`usage`, `ask`, `prompt`, `runs`, and `auth status`. Prompt listing returns sorted preset
summaries (`name`, `model`, and `has_system`); execution returns the resolved
model, assistant content, and usage. `runs show` reports its canonical command name as
`runs show`. Failed JSON commands leave stdout empty; diagnostics go to stderr.
