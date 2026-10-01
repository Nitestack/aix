# aix — AI eXecute

> [!NOTE]
> **Not IBM AIX.** This project has no relation to IBM's AIX operating system.

Profile-aware CLI for AI tools and AI gateway utilities. Spend reporting uses LiteLLM's admin API; model discovery works with any OpenAI-compatible gateway, and `aix ask` provides domain-agnostic one-shot inference.

`aix` reads a config file, resolves API keys and gateway URLs from various secret sources, and either exports them as shell variables, runs a command with those variables pre-set, or sends a one-shot text request to the gateway. No credentials are stored in shell history or process lists.

---

## Table of contents

- [How it works](#how-it-works)
- [Installation](#installation)
- [Config path resolution](#config-path-resolution)
- [Config format](#config-format)
- [Secret sources](#secret-sources)
- [Environment variables emitted](#environment-variables-emitted)
- [Shell integration](#shell-integration)
- [Running tools directly](#running-tools-directly)
- [Model discovery](#model-discovery)
- [Spend and budget](#spend-and-budget)
- [Gateway status](#gateway-status)
- [Historical usage](#historical-usage)
- [One-shot inference](#one-shot-inference)
- [JSON output and exit codes](#json-output-and-exit-codes)
- [Cache](#cache)
- [Security notes](#security-notes)

---

## How it works

`aix` manages one or more named *profiles*, each pairing an API key with a gateway URL. It injects credentials into subprocesses and shells, reads spend data from LiteLLM gateways, and discovers model IDs through the standard OpenAI-compatible endpoint.

```
config file → profile selection → secret resolution → env vars → your tool
```

The tool never logs, prints, or stores resolved secrets outside the subprocess environment.

---

## Installation

### Via Nix

Three paths depending on how much you want Nix to manage.

#### home-manager module (recommended for NixOS / nix-darwin)

Add the flake input, import the module, and declare your config declaratively. Nix installs the binary and generates `~/.config/aix/aix.toml` at activation time.

```nix
# flake.nix
inputs.aix.url = "github:Nitestack/aix";
```

```nix
# home-manager config
imports = [ inputs.aix.homeManagerModules.aix ];

programs.aix = {
  enable = true;
  defaultProfile = "work";

  endpoint = {
    # Any secret source: { env = "VAR"; }, { file = "/run/secrets/..."; },
    # { command = "pass show ..."; }, or a plain string (stored in Nix store).
    baseUrl = { file = "/run/secrets/aix/base-url"; };
  };

  profiles.work = {
    label  = "Work";
    apiKey = { file = "/run/secrets/aix/work-key"; };
  };
};
```

Then rebuild:

```bash
nixos-rebuild switch --flake .#your-host
# or, standalone home-manager:
home-manager switch --flake .#your-user
```

#### Overlay (install binary only, manage config yourself)

Apply `overlays.default` to add `pkgs.aix-rs` to your package set, then add it to `home.packages` or `environment.systemPackages`. The overlay only supports the four default systems (`x86_64-linux`, `aarch64-linux`, `x86_64-darwin`, `aarch64-darwin`).

```nix
nixpkgs.overlays = [ inputs.aix.overlays.default ];
home.packages = [ pkgs.aix-rs ];
```

#### One-off / try it out

```bash
nix run github:Nitestack/aix -- --help
```

### Via Cargo

```bash
cargo install --path .
```

Or build without installing:

```bash
cargo build --release
# binary is at target/release/aix
```

### Prebuilt binaries

Prebuilt binaries for Linux (x86_64, aarch64), macOS (arm64, x86_64), and Windows (x86_64) are attached to the rolling `latest` GitHub Release, rebuilt on every push to `main`.

#### Installer scripts (recommended)

The quickest way to install — the script auto-detects your platform, downloads the right binary, and wires up your `PATH`.

**Linux / macOS:**

```sh
curl -fsSL https://github.com/Nitestack/aix/releases/latest/download/install.sh | sh
```

Installs to `~/.local/bin` by default. Adds the directory to your shell rc file (`~/.zshrc`, `~/.bashrc`, `~/.config/fish/config.fish`, or `~/.profile`). Restart your terminal or source the rc file after installing.

**Windows (PowerShell):**

```powershell
irm https://github.com/Nitestack/aix/releases/latest/download/install.ps1 | iex
```

Installs to `%LOCALAPPDATA%\Programs\aix` by default. Adds the directory to the user `Path` environment variable. Restart your terminal after installing.

**Custom install location:**

Set `AIX_INSTALL_DIR` before running either script to override the default destination:

```sh
AIX_INSTALL_DIR=/usr/local/bin curl -fsSL https://github.com/Nitestack/aix/releases/latest/download/install.sh | sh
```

```powershell
$env:AIX_INSTALL_DIR = 'C:\Tools\aix'; irm https://github.com/Nitestack/aix/releases/latest/download/install.ps1 | iex
```

#### Manual download

To place the binary yourself, download the appropriate asset directly from the [latest release](https://github.com/Nitestack/aix/releases/latest):

| Platform | Asset name |
|----------|-----------|
| Linux x86_64 | `aix-x86_64-unknown-linux-gnu` |
| Linux aarch64 | `aix-aarch64-unknown-linux-gnu` |
| macOS arm64 | `aix-aarch64-apple-darwin` |
| macOS x86_64 | `aix-x86_64-apple-darwin` |
| Windows x86_64 | `aix-x86_64-pc-windows-msvc.exe` |

---

## Config path resolution

`aix` locates its config file in the following order:

1. `--config <PATH>` flag (highest priority)
2. `AIX_CONFIG` environment variable
3. Platform config directory, searching for the first of:
   - `aix.toml`
   - `aix.yaml` / `aix.yml`
   - `aix.json` / `aix.json5`

The platform config directory is:

| Platform | Default path |
|----------|-------------|
| Linux    | `~/.config/aix/` |
| macOS    | `~/Library/Application Support/aix/` |
| Windows  | `%APPDATA%\aix\` |

Print the resolved path at any time:

```bash
aix config path
```

Validate the config without running anything:

```bash
aix config validate
```

---

## Config format

Supported formats: TOML, YAML, JSON, JSON5. All examples below use TOML.

### Minimal example (direct key)

```toml
# ~/.config/aix/aix.toml

[endpoint]
base_url = "https://ai-hub.example.com"

[profiles.work]
label = "Work"
api_key = "sk-fake-0000000000000000000000000000000000000000000000"
```

### Full example (all secret types, all options)

```toml
# ~/.config/aix/aix.toml

# Profile used when no --profile flag and stdin/stdout are not both TTYs.
# Remove to always use the interactive picker when both are TTYs.
default_profile = "work"

# Optional: .env files to source before resolving secrets.
# Real environment variables always win over .env values.
# Paths support ~ expansion.
env_files = ["~/.env.aix"]

[endpoint]
# The gateway base URL — accepts any secret source (see below).
base_url = { env = "AIX_BASE_URL" }
# Optional metadata. `aix spend` accepts an unset gateway or "litellm" only.
gateway = "litellm"
# `provider` is optional metadata for other consumers.
provider = "litellm"

# ── Profile: Work ────────────────────────────────────────────────────────────
[profiles.work]
label   = "Work"
# Env var secret: read AIX_WORK_KEY from the process environment.
api_key = { env = "AIX_WORK_KEY" }

# ── Profile: Personal ────────────────────────────────────────────────────────
[profiles.personal]
label   = "Personal"
# File secret: read from a file (Nix secrets, Docker secret, etc.).
# Trailing newline is stripped automatically.
api_key = { file = "/run/secrets/aix/personal" }
# Optional profile-specific gateway, overriding [endpoint].base_url.
base_url = { env = "AIX_PERSONAL_BASE_URL" }

# Additional variables with valid shell-environment names are injected after generated
# variables and may override them.
[profiles.personal.env]
EXAMPLE_FEATURE_FLAG = "enabled"

# ── Profile: Dev ─────────────────────────────────────────────────────────────
[profiles.dev]
label   = "Dev (command-based)"
# Command secret: run a shell command, use stdout as the secret.
# Works with pass, 1Password CLI, Bitwarden CLI, etc.
api_key = { command = "pass show aix/dev-key" }

# ── Profile: Staging (direct key — avoid in production) ──────────────────────
[profiles.staging]
label   = "Staging"
# Direct key: literal string. Fine for throwaway / CI use.
# Never commit real keys.
api_key = "sk-fake-staging-0000000000000000000000"
```

### Model defaults and aliases

Model settings are optional. Defaults are raw model IDs; aliases are local names for raw IDs:

```toml
[models]
default = "gateway/claude-sonnet"

[models.aliases]
fast = "gateway/gpt-fast"
smart = "gateway/claude-opus"

[profiles.work.models]
default = "company/claude-sonnet"

[profiles.work.models.aliases]
fast = "company/fast-model"
```

For an explicit model request, a selected profile alias wins over a top-level alias; otherwise the value is treated as a raw model ID. Without an explicit model, `profiles.<name>.models.default` wins over `models.default`. If neither default exists, the caller must pass `--model` or configure one. Defaults are never looked up as aliases, and aliases do not chain. Alias targets need not appear in a live model list; `aix config validate` checks them offline for non-empty values.

The Home Manager module exposes the same structure:

```nix
programs.aix.models = {
  default = "gateway/claude-sonnet";
  aliases = {
    fast = "gateway/gpt-fast";
    smart = "gateway/claude-opus";
  };
};

programs.aix.profiles.work.models = {
  default = "company/claude-sonnet";
  aliases.fast = "company/fast-model";
};
```

`aix ask` uses this configuration and shared resolver for one-shot inference.

### Cache config

```toml
# Optional: cache aix spend API responses on disk (default: 1-hour TTL, enabled)
[cache]
ttl_secs = 3600  # seconds before a cached response is considered stale; 0 = never expires
disabled = false # set to true to always fetch fresh data
```

---

## Secret sources

Every secret-backed field (`api_key`, `base_url`, and profile `env` values) accepts one of four forms. Profile `label` supports the same dynamic forms:

| Source | Syntax | Notes |
|--------|--------|-------|
| Direct | `"sk-literal-string"` | Plain string, no expansion. Use only for non-sensitive / throwaway values. |
| Env var | `{ env = "VAR_NAME" }` | Read from the process environment at runtime. |
| File | `{ file = "/path/to/file" }` | Read file contents; `~` is expanded; trailing newline stripped. |
| Command | `{ command = "some-cli read secret/path" }` | Run command via shell; stdout becomes the secret; trailing newline stripped. |

Only one source per field is allowed. Mixing sources in the same field is a config validation error.

---

## Environment variables emitted

`aix env` and `aix exec` always inject all seven variables:

| Variable | Value |
|---|---|
| `AIX_PROFILE` | Selected profile name |
| `ANTHROPIC_API_KEY` | Resolved API key |
| `ANTHROPIC_BASE_URL` | Gateway base URL (bare) |
| `OPENAI_API_KEY` | Resolved API key |
| `OPENAI_BASE_URL` | Gateway base URL with `/v1` appended |
| `LITELLM_API_KEY` | Resolved API key |
| `LITELLM_BASE_URL` | Gateway base URL with `/v1` appended |

`LITELLM_API_KEY`/`LITELLM_BASE_URL` are aliases for the same credential and
gateway as `OPENAI_API_KEY`/`OPENAI_BASE_URL` — not a third distinct secret
— for tools that specifically look for a `LITELLM_*`-named variable (e.g.
LiteLLM-aware config formats). They are always present regardless of tool
name or invocation shape.

Named-tool subcommands (`aix <tool>`) emit a subset of the Anthropic/OpenAI
pair based on the tool name, but `AIX_PROFILE`, `LITELLM_API_KEY`, and
`LITELLM_BASE_URL` are always included regardless of tool name:

| Invocation | Anthropic/OpenAI vars set |
|---|---|
| `aix claude ...` | `ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL` |
| `aix <any other tool> ...` | `OPENAI_API_KEY`, `OPENAI_BASE_URL` |

`AIX_API_KEY` and `AIX_BASE_URL` are never emitted. Tools that previously read those variables should switch to the `ANTHROPIC_*` or `OPENAI_*` equivalents.

---

## Shell integration

Use `aix env` to emit export statements and eval them in the current shell.

### POSIX sh / bash / zsh

```sh
# Emit and eval (current shell)
eval "$(aix env work)"

# Explicitly request sh format
eval "$(aix env work --format sh)"

# Use default profile
eval "$(aix env)"
```

### Nushell

```nu
# JSON is the simplest approach — no temp file needed
aix env work --format json | from json | load-env

# Alternatively, use the Nushell format and source a file
# (source requires a literal path, so save first)
aix env work --format nu | save --force /tmp/aix-env.nu
source /tmp/aix-env.nu
```

### Fish

```fish
# Fish format uses set -x KEY 'value' syntax
aix env work --format fish | source
```

> **Note:** `eval (aix env work --format fish)` does **not** work. Fish command substitution splits output on newlines into separate list elements, and `eval` then joins them with spaces — collapsing all the `set -x` lines into a single malformed `set` call. Use `| source` instead.

### PowerShell

```powershell
# PowerShell format uses $env:KEY = 'value' syntax
aix env work --format powershell | Invoke-Expression
```

### Windows Command Prompt (cmd.exe)

```cmd
REM Save to a temp file and call it in the current session
aix env work --format cmd > "%TEMP%\aix-env.cmd" && call "%TEMP%\aix-env.cmd"

REM Use default profile
aix env --format cmd > "%TEMP%\aix-env.cmd" && call "%TEMP%\aix-env.cmd"
```

> **Note:** Values containing `%` are safe — the output doubles them to `%%` so `SET` interprets them correctly. Values containing `"` use a `""` encoding that works on modern Windows 10/11 cmd.exe but is not guaranteed on all NT versions. If your API key or base URL contains a literal double-quote (rare in practice), use `--format powershell` instead.

### Inspect without loading (JSON)

```bash
# Useful for scripting or debugging — outputs a JSON object
aix env work --format json | jq .
```

---

## Running tools directly

### aix exec — one-shot command

Run any command with the profile environment set. Arguments after `--` are passed through unchanged.

```bash
# Run curl with work credentials
aix exec work -- curl -s https://ai-hub.example.com/health

# Use default profile
aix exec -- my-ai-tool --prompt "Hello"

# Dry-run: print what would run without running it (no secrets in output)
aix exec work --dry-run -- my-ai-tool
```

### aix shell — interactive shell session

Opens an interactive shell with the profile environment pre-loaded. Shell detection order: `$SHELL` env var → `NU_VERSION` (Nushell, if on `$PATH`) → `sh` fallback.

```bash
# Work profile
aix shell work

# Default profile
aix shell

# See what shell would be launched
aix shell work --dry-run
```

### Running any AI tool

`aix <tool>` works with any binary on `$PATH` — no configuration needed and no list to maintain. The credential format is chosen automatically:

- `aix claude ...` → injects `ANTHROPIC_*` variables
- `aix <anything else> ...` → injects `OPENAI_*` variables

```bash
# aix <tool> [PROFILE] [--dry-run] [-- TOOL_ARGS...]

aix claude work -- --model claude-opus-4
aix opencode work -- --model gpt-4o
aix aider work -- --no-auto-commits
aix goose work -- session start
aix my-new-agent work -- --prompt "Hello"

# Interactive profile picker (when both stdin and stdout are TTYs)
aix claude -- chat

# Dry-run: print what would run, never print secrets
aix opencode work --dry-run
```

### Listing profiles

```bash
# Human-readable list
aix profiles

# JSON envelope with { name, label } objects in data (no secrets)
aix profiles --json

# The global flag is also accepted before the command
aix --json profiles
```

---

## Model discovery

`aix models` fetches model IDs live from the selected profile's OpenAI-compatible `/v1/models` endpoint. It does not cache the response or assume provider-specific metadata.

```bash
# List model IDs for the default or selected profile
aix models work

# Filter IDs by a case-insensitive substring
aix models work --filter gpt

# Emit the stable JSON envelope for scripts
aix --json models work
```

Human output contains one sorted model ID per line. If no IDs match a filter, the command succeeds with empty output. Use `--filter <TEXT>` rather than a second positional argument so the profile position remains unambiguous.

---

## Spend and budget

`aix spend` fetches live spend and budget data from your LiteLLM gateway and displays it as a human-readable progress bar.

```bash
# Show spend for the default or selected profile
aix spend work

# JSON envelope containing aix's spend and optional max_budget fields
aix spend work --json

# The global flag is also accepted before the command
aix --json spend work

# Always fetch fresh data (result is still cached for future calls)
aix spend work --no-cache
```

**Example output:**

```
$41.53 of $500.00  ·  $458.47 available (92%)  ·  cached 5m ago
[████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░]  8% used
```

Responses are cached locally for 1 hour by default (configurable via `[cache]`). The `·  cached N ago` suffix appears when the result was served from cache. Use `--no-cache` to bypass the cache for a single call; the fresh response is still written to cache for subsequent calls.

`aix spend` requires a LiteLLM-compatible gateway. If `gateway` is set in your config to a non-LiteLLM value, `aix spend` will refuse to run with a clear error. Leaving `gateway` unset (or setting it to `"litellm"`) is accepted.

---

## Gateway status

`aix status` gives a one-screen overview of the selected profile, a live authenticated gateway probe, and available LiteLLM spend data:

```bash
# Use the default profile or select one explicitly
aix status
aix status work

# Refresh spend data instead of using the spend cache
aix status work --refresh

# Emit the shared JSON envelope
aix --json status work
```

The OpenAI-compatible `GET /v1/models` probe is always live, including when spend data comes from cache. `--refresh` bypasses only the spend cache; a successful probe reports its latency and authenticated state. The output shows configured gateway/provider metadata and only the API key's final four characters (short keys are fully redacted). The resolved gateway URL and full key are never printed.

```text
Profile     work · Work
Gateway     litellm · provider=litellm · reachable · authenticated · 84ms
Key         sk-...7fa2
Spend       $41.53 / $500.00 · $458.47 remaining · 8% used · cached 5m ago
```

For a non-LiteLLM gateway, or when the LiteLLM management endpoint is unsupported, status still succeeds after a healthy gateway probe and reports spend as unsupported. Other spend lookup failures are reported as unavailable; they do not hide a successful connectivity/authentication result.

---

## Historical usage

`aix usage` is the historical counterpart to `aix spend`. `spend` is the quick current-budget check: it reads LiteLLM's key information and caches the response for an hour by default. `usage` queries LiteLLM's `/user/daily/activity` accounting endpoint for daily spend, tokens, request counts, and model breakdowns. It does not estimate costs locally and does not use the `spend` cache.

```bash
# Default: the last 30 local calendar dates, including today
aix usage work

# Last seven calendar dates, including today
aix usage work --since 7d

# Inclusive explicit date range
aix usage work --start 2026-09-01 --end 2026-09-30

# Filter and aggregate metrics for an exact model ID returned by LiteLLM
aix usage work --model gpt-6-luna

# Stable JSON envelope
aix usage work --json
```

`--since Nd` requires a positive whole number of days and cannot be combined with `--start`/`--end`. Explicit dates must both be supplied as inclusive `YYYY-MM-DD` dates, with the end on or after the start. An unset `gateway` is accepted for compatibility; an explicitly non-LiteLLM gateway is rejected. If the gateway or LiteLLM version does not provide the daily activity endpoint, `aix usage` reports that history is unavailable. Authentication failures remain separate errors.

JSON output uses the shared envelope. `usage.data` contains `start_date`, `end_date`, aggregate `spend`, `prompt_tokens`, `completion_tokens`, `total_tokens`, and `request_count`, plus `daily` and spend-sorted `models` arrays. `model_filter` is included when `--model` is applied. Only aix-owned fields are emitted; upstream API-key breakdowns and arbitrary response fields are discarded.

---

## One-shot inference

`aix ask` sends one instruction and any explicitly supplied text context to the selected profile's OpenAI-compatible `/v1/chat/completions` endpoint. It uses `--model` or the configured model default and returns the assistant's text without caching prompts or responses.

```sh
# Diagnose a service log; stdin becomes explicit context.
journalctl -u nginx -n 100 | aix ask "Identify the likely failure and suggest the next diagnostic step"

# Summarize a document from a pipe.
cat contract.txt | aix ask "Summarize the obligations and deadlines"

# Add one or more explicitly named files as context.
aix ask --file notes.md "Turn these notes into a concise summary"

# Select a profile and model alias.
aix --profile work ask --model smart "Explain TCP slow start"
```

The optional `--system TEXT` is sent as the only system message. Repeated `--file PATH` values are sent after piped stdin, in command-line order, with file boundaries that include each supplied path. If there is no instruction, non-empty piped stdin becomes the user message; file context alone requires an instruction. `aix ask` does not start an interactive chat.

Input to `ask` is sent to the configured gateway. Only the prompt, non-TTY stdin, and paths passed through `--file` are read. **aix does not inspect the current project or coding harness, inspect Git state, discover neighboring files, or infer project context.** This is a domain-agnostic inference command; pipes and files are ordinary user-supplied context.

Human output contains only the assistant's text and a trailing newline. Use `--json` for the stable aix JSON envelope, which includes the resolved model, assistant content, and token usage (null when the gateway omits a field).

---

## JSON output and exit codes

The global `--json` flag is currently supported by `profiles`, `spend`, `models`, `status`, `usage`, and `ask`. It can appear before or after the command. Existing JSON forms remain supported. Other commands with their own output options, such as `aix env --format json`, keep those existing formats; using the global `--json` flag with an unsupported command is a validation error.

JSON mode writes exactly one JSON document to stdout. Errors and diagnostics go to stderr, and a failed JSON command leaves stdout empty. The envelope is owned by `aix`:

```json
{
  "schema_version": 1,
  "command": "profiles",
  "data": [
    { "name": "work", "label": "Work" }
  ]
}
```

`schema_version` is an integer that starts at `1`; `command` is the canonical command name. `profiles.data` contains profile names and labels only. `spend.data` contains the selected key's `spend` and optional `max_budget` when a key matches, or the gateway's user totals otherwise. `models.data` contains sorted `{ "id" }` entries and the requested `filter` (or `null` when no filter was used). `status.data` contains stable profile, gateway/probe, key-suffix, spend/budget, and cache fields. Optional metadata and unavailable spend values are represented as `null` with an explicit spend status; cache source is `live`, `cache`, or `none`. `usage.data` contains the historical date range, aggregates, daily series, and model aggregates described above. These commands do not expose raw upstream responses, credentials, API-key breakdowns, or unrelated upstream fields.

```json
{
  "schema_version": 1,
  "command": "status",
  "data": {
    "profile": { "name": "work", "label": "Work" },
    "gateway": {
      "gateway": "litellm",
      "provider": "litellm",
      "status": "reachable",
      "authentication_status": "authenticated",
      "latency_ms": 84
    },
    "key_suffix": "7fa2",
    "spend": {
      "status": "available",
      "spend": 41.53,
      "max_budget": 500.0,
      "remaining_budget": 458.47,
      "percent_used": 8.306,
      "cache": { "source": "cache", "age_seconds": 300 }
    }
  }
}
```
When LiteLLM reports a budget-exceeded response, `aix spend` returns exit code `6`. Human mode still shows the spend summary; JSON mode treats it as an error and leaves stdout empty.

Process exit codes are stable:

| Code | Meaning |
|---:|---|
| `0` | Success |
| `1` | Uncategorized or internal failure |
| `2` | CLI or configuration validation failure |
| `3` | Secret resolution failure |
| `4` | Authentication or authorization failure |
| `5` | Network or gateway failure |
| `6` | Budget or policy failure |

---

## Cache

`aix spend` and `aix status` cache LiteLLM spend responses in the platform cache directory to avoid redundant management requests. `aix status` still performs its gateway probe on every invocation:

| Platform | Default cache directory |
|----------|------------------------|
| Linux    | `~/.cache/aix/`        |
| macOS    | `~/Library/Caches/aix/` |
| Windows  | `%LOCALAPPDATA%\aix\cache\` |

Override the directory for all invocations:

```bash
export AIX_CACHE_DIR=/tmp/aix-cache
```

### Cache configuration

Add a `[cache]` block to your config to tune behaviour:

```toml
[cache]
ttl_secs = 3600  # cached entries older than this are treated as stale (0 = never expire)
disabled = false # set to true to always fetch live data (equivalent to always passing --no-cache)
```

### Clearing the cache

```bash
aix cache clear
```

Deletes all cached response files and prints a count of removed files.

---

## Security notes

### Prefer indirect secret sources

Avoid `direct` keys in config files that live on disk. Prefer, in order:

1. **`file`** — secret lives in a permission-restricted file (e.g., Nix `agenix`, Docker secrets, systemd credentials). No shell expansion of the value.
2. **`env`** — key is injected by your shell session, CI system, or secrets manager at launch time. Never written to disk by `aix`.
3. **`command`** — delegates to a CLI secrets manager (`pass`, `1password`, `bitwarden-cli`). Subprocess is launched per invocation; output is never logged.
4. **`direct`** — plain string in the config. Acceptable only for throwaway keys in local-only, non-shared configs.

### Never commit real keys

The config file can contain real file paths and env var names — those are fine to commit. Only `direct` values embed the secret itself. Add your config file to `.gitignore` if it contains any `direct` keys:

```gitignore
# If your config has direct keys, keep it out of git
.config/aix/aix.toml
```

If you use the Nix deployment, the generated config uses `file` sources pointing to `/run/secrets/aix/*` — those paths are safe to commit.

### .env files are opt-in

`aix` does **not** automatically load `.env` files from the working directory. Loading happens only when you explicitly list files under `env_files` in your config. Environment variables set before `aix` runs always win over `.env` values.

### Secrets are zeroized after use

Resolved secret values are held in memory types that zero their contents when dropped. They are never written to disk, printed in logs, or exposed via `--dry-run` output. Dry-run shows variable *names* only.
