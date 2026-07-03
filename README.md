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

### Via Nix (recommended for NixOS / home-manager)

The Nix deployment generates a config file at the appropriate path and wires up secret files from your secrets store. Add the `aix` home-manager module and rebuild:

```bash
nixos-rebuild switch --flake .#your-host
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

Prebuilt binaries for Linux x86_64 and aarch64 are attached to each GitHub Release.

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
base_url = "https://ai-hub.example.com/anthropic"
api_format = "anthropic"   # "anthropic" | "openai" | "both"

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
# The LiteLLM gateway base URL — accepts any secret source (see below).
base_url = { env = "AIX_BASE_URL" }

# api_format controls which downstream env vars are emitted:
#   "anthropic" → ANTHROPIC_API_KEY + ANTHROPIC_BASE_URL
#   "openai"    → OPENAI_API_KEY + OPENAI_BASE_URL
#   "both"      → both sets
api_format = "anthropic"

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

---

## Secret sources

Every field that holds a secret (currently `api_key` and `base_url`) accepts one of four forms:

| Source | Syntax | Notes |
|--------|--------|-------|
| Direct | `"sk-literal-string"` | Plain string, no expansion. Use only for non-sensitive / throwaway values. |
| Env var | `{ env = "VAR_NAME" }` | Read from the process environment at runtime. |
| File | `{ file = "/path/to/file" }` | Read file contents; `~` is expanded; trailing newline stripped. |
| Command | `{ command = "some-cli read secret/path" }` | Run command via shell; stdout becomes the secret; trailing newline stripped. |

Only one source per field is allowed. Mixing sources in the same field is a config validation error.

---

## Environment variables emitted

All commands that set up an environment (exec, shell, claude, pi) resolve the selected profile and inject:

```
AIX_PROFILE   — the resolved profile name
AIX_API_KEY   — the resolved API key
AIX_BASE_URL  — the resolved gateway base URL
```

Plus, depending on `api_format`:

| api_format   | Additional variables |
|--------------|---------------------|
| `anthropic`  | `ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL` |
| `openai`     | `OPENAI_API_KEY`, `OPENAI_BASE_URL` |
| `both`       | All four of the above |

`ANTHROPIC_*` / `OPENAI_*` values are always identical to the corresponding `AIX_*` values.

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

Opens an interactive shell with the profile environment pre-loaded. Shell detection order: `NU_VERSION` (Nushell) → `$SHELL` → `sh`.

```bash
# Work profile
aix shell work

# Default profile
aix shell

# See what shell would be launched
aix shell work --dry-run
```

### aix claude — Claude CLI wrapper

Finds `claude` on `$PATH` and runs it with the profile environment set.

```bash
# aix claude PROFILE -- CLAUDE_ARGS...

# Run Claude with the work profile and the swtb model shorthand
aix claude work -- --model swtb

# Pass through all claude flags
aix claude work -- --model swtb --output-format json

# Interactive profile picker (when both stdin and stdout are TTYs)
aix claude -- --help
```

### aix pi — Pi CLI wrapper

Finds `pi` on `$PATH` and runs it with the profile environment set.

```bash
# aix pi PROFILE -- PI_ARGS...

aix pi work -- --model swtb

aix pi work -- chat --system "You are a helpful assistant."

# Dry-run (prints command and variable names, never secrets)
aix pi work --dry-run -- --model swtb
```

### Listing profiles

```bash
# Human-readable list
aix profiles

# JSON array of { name, label } objects (no secrets)
aix profiles --json
```

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

---

## Manual verification before release

The following scenarios require a real interactive environment and cannot be covered by automated tests.

| Scenario | Command | What to confirm |
|----------|---------|-----------------|
| Interactive profile selector | `aix exec` with no `--profile` and no `default_profile` in a real TTY | `inquire` picker appears; selecting a profile works |
| `aix shell` | `aix shell <profile>` | Sub-shell launches with `AIX_PROFILE`, `AIX_API_KEY`, `AIX_BASE_URL` set |
| `aix claude` | `aix claude <profile> -- --version` (with `claude` installed) | `claude` receives the Anthropic env vars |
| `aix pi` | `aix pi <profile> -- --version` (with `pi` installed) | `pi` receives the env vars |
| PowerShell env | `aix env <profile> --format powershell \| Invoke-Expression` in PowerShell | `$env:AIX_PROFILE` is set |
| Nushell env | `aix env <profile> --format json \| from json \| load-env` in Nushell | `$env.AIX_PROFILE` is set |
