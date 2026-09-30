# aix — AI eXecute

> [!NOTE]
> **Not IBM AIX.** This project has no relation to IBM's AIX operating system.

Profile-aware credential injector for AI tools backed by a LiteLLM-compatible AI gateway.

`aix` reads a config file, resolves API keys and gateway URLs from various secret sources, and either exports them as shell variables or runs a command with those variables pre-set. No credentials are stored in shell history or process lists.

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
- [Spend and budget](#spend-and-budget)
- [Cache](#cache)
- [Security notes](#security-notes)

---

## How it works

The gateway is a LiteLLM proxy that exposes Anthropic- and OpenAI-compatible endpoints. `aix` manages one or more named *profiles*, each pairing an API key with a gateway URL, and injects the right set of variables into a subprocess or your shell session.

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

This adds configuration and the shared resolver only; the current CLI does not issue inference requests.

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

# JSON array of { name, label } objects (no secrets)
aix profiles --json
```

---

## Spend and budget

`aix spend` fetches live spend and budget data from your LiteLLM gateway and displays it as a human-readable progress bar.

```bash
# Show spend for the default or selected profile
aix spend work

# JSON output — full /user/info payload
aix spend work --json

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

## Cache

`aix spend` caches API responses in the platform cache directory to avoid redundant network calls:

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
