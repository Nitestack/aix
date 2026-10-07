# aix — AI eXecute

> [!NOTE]
> **Not IBM AIX.** This project has no relation to IBM's AIX operating system.

Profile-aware CLI for AI tools and AI gateway utilities. Profiles can use gateway API keys or aix-managed Sign in with ChatGPT credentials. Spend reporting uses LiteLLM's admin API; model discovery works with any OpenAI-compatible gateway, and `aix ask` plus configured `aix prompt` presets provide domain-agnostic one-shot inference for API-key profiles.

`aix` reads a config file, resolves API keys and gateway URLs from various secret sources, and either exports them as shell variables, runs a command with those variables pre-set, or sends a one-shot text request to the gateway. ChatGPT sign-in credentials are stored separately with owner-only filesystem permissions; they are not exported to subprocesses.

---

## Table of contents

- [How it works](#how-it-works)
- [Installation](#installation)
- [Config path resolution](#config-path-resolution)
- [Config format](#config-format)
- [ChatGPT authentication](#chatgpt-authentication)
- [Secret sources](#secret-sources)
- [Environment variables emitted](#environment-variables-emitted)
- [Shell integration](#shell-integration)
- [Running tools directly](#running-tools-directly)
- [Model discovery](#model-discovery)
- [Spend and budget](#spend-and-budget)
- [Gateway status](#gateway-status)
- [Diagnostics](#diagnostics)
- [Historical usage](#historical-usage)
- [One-shot inference](#one-shot-inference)
  - [Reusable prompt presets](#reusable-prompt-presets)
- [Managed runs](#managed-runs)
- [Global options](#global-options)
- [JSON output and exit codes](#json-output-and-exit-codes)
- [Cache](#cache)
- [Security notes](#security-notes)

---

## How it works

`aix` manages named profiles pairing API keys with gateway URLs. It injects credentials into subprocesses and shells, reads gateway status and usage, discovers model IDs, sends one-shot inference requests, and can record metadata-only history around arbitrary commands.

```
config file → profile selection → secret resolution → env vars → your tool
```

API-key values are not logged or written to disk by aix. OAuth credentials are the exception: they are retained in aix's local auth store so `aix auth status` and `aix auth logout` work across invocations.

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

# Profile used after --profile and AIX_PROFILE, before the interactive picker.
# Remove it to let interactive invocations open the picker when no profile is selected.
default_profile = "work"

# Optional: .env files to source before resolving secrets.
# Real environment variables always win over .env values.
# Paths support ~ expansion.
env_files = ["~/.env.aix"]

[endpoint]
# The gateway base URL — accepts any secret source (see below).
base_url = { env = "AIX_BASE_URL" }
# Optional metadata. `aix spend` and `aix usage` accept an unset gateway or "litellm" only.
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

### ChatGPT-authenticated profile

ChatGPT authentication does not use `[endpoint]` or an API key. Add a profile
with the explicit `auth` object:

```toml
default_profile = "personal"

[profiles.personal]
label = "Personal ChatGPT"
auth = { type = "chatgpt" }
```

Then manage the local sign-in:

```sh
aix auth login personal   # opens the system browser; requires an interactive session
aix auth status personal  # reads local state only; makes no network requests
aix auth logout personal  # revokes remotely when possible and always clears local tokens
```

The profile can also be declared in Home Manager as
`programs.aix.profiles.personal.auth = "chatgpt";`. Existing profiles with
`api_key` remain API-key profiles and need no migration.

ChatGPT credentials are handed to a child only when the selected profile and
named tool have an explicit `[tools.<name>.chatgpt]` binding. The binding names
the one environment variable that receives the current short-lived access
token. `aix env`, `aix shell`, `aix exec`, `aix ask`, and unconfigured tools do
not receive OAuth credentials. `aix run -- <configured-tool>` uses the same
binding, while LiteLLM leases and run policies remain API-key-only.

The refresh token, retained ID token, and issued OAuth client ID stay in aix's
owner-only auth store; they are never passed to the child or written to run
history. `--dry-run` prints the executable, effective arguments, and variable
names, and does not start a gateway or refresh a token. Direct transport obtains
the access token before the child starts, so a long-lived direct-mode harness
must be restarted to receive a renewed token. The supported Codex app-server
local-gateway mode instead keeps a stable local bearer and obtains a current
upstream token for each inference request. This integration covers Codex
app-server only, not the Codex TUI or other Codex modes.

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

### Reusable prompt presets

Define named instructions under `[prompts.<name>]` to reuse them with `aix prompt`:

```toml
[prompts.summarize]
prompt = "Summarize the supplied material clearly and concisely."

[prompts.diagnose]
prompt = "Analyze the supplied diagnostic output and suggest the next verification step."
system = "Be precise and distinguish evidence from inference."
model = "smart"

[prompts.extract-actions]
prompt = "Extract concrete actions, owners, dates, and unresolved questions from the supplied notes."

# An optional user-defined code-oriented preset is ordinary prompt text too.
[prompts.review-code]
prompt = "Review the supplied change for correctness, regressions, and security issues."
```

`prompt` is required and non-empty. `system` and `model` are optional and must
be non-empty when present. A model can be an alias or raw model ID. Selection
uses `--model`, then the preset's model, then the selected profile's default,
then the global default. Aliases resolve through the same model configuration
as `aix ask`.

```sh
# Presets are user-defined; aix ships no prompt catalog.
aix prompt --list
aix prompt --json --list

# Only explicitly piped or named-file context is supplied.
cat meeting-notes.txt | aix prompt summarize
journalctl -u nginx -n 100 | aix prompt diagnose
aix prompt extract-actions --file notes.txt
git diff | aix prompt review-code --model smart
```

`--file` can be repeated. Stdin and files are passed as opaque context with the
same boundaries and ordering as `aix ask`. Preset text is sent literally: there
is no template substitution, shell execution, or environment expansion. Listing
sorts names and shows only model overrides; JSON entries contain `name`,
`model`, and `has_system`, never prompt or system bodies. Presets do not inspect
the working directory, Git state, project files, or a coding harness.

Home Manager exposes the same settings declaratively:

```nix
programs.aix.prompts = {
  summarize.prompt = "Summarize the supplied material clearly and concisely.";
  diagnose = {
    prompt = "Analyze the supplied diagnostic output.";
    system = "Distinguish evidence from inference.";
    model = "smart";
  };
};
```

### Named tool launch configuration

An optional `[tools.<name>]` entry controls how a named tool is launched by
`aix <tool>` and `aix run -- <tool> ...`. The map key is the logical tool name;
`command` optionally selects a different executable, `api_format` selects
`anthropic`, `openai`, or `both`, and `env` adds tool-specific variables. Tool
environment values override profile values, which override generated
credentials for API-key profiles. A ChatGPT profile instead requires a
`chatgpt` binding. Other ChatGPT-bound tools receive the selected access token
in `access_token_env`; the special `opencode` binding instead starts an
ephemeral SIWC bridge and gives OpenCode only a random local bridge credential.
It never gives OpenCode the ChatGPT access, refresh, or ID token. Unconfigured
tools never receive a ChatGPT token.

For `aix <tool> ... --dry-run`, aix lists environment variable names without
resolving configured secret sources or refreshing ChatGPT credentials.

```toml
[tools.review]
command = "review-agent"
api_format = "openai"

[tools.review.env]
REVIEW_MODE = "review"
REVIEW_TOKEN = { env = "AIX_REVIEW_TOKEN" }
```

The equivalent Home Manager options use camelCase option names:

```nix
programs.aix.tools.review = {
  command = "review-agent";
  apiFormat = "openai";
  env = {
    REVIEW_MODE = "review";
    REVIEW_TOKEN = { env = "AIX_REVIEW_TOKEN"; };
  };
};
```

#### Profile-specific Codex configuration

Select a separate Codex home for launches using a named aix profile:

```toml
[profiles.work.tool_configs.codex]
config_dir = "~/.config/codex/work"
```

The directory must already exist and be accessible. Absolute paths and `~` are
supported; relative paths are resolved from the aix configuration file. If the
directory is unavailable, aix warns and launches Codex with its standard
configuration instead. The equivalent Home Manager option is
`programs.aix.profiles.work.toolConfigs.codex.configDir`.

This setting selects Codex's `CODEX_HOME`. That replaces the user configuration
directory, but `CODEX_HOME` also selects Codex's persistent state. The selected
home does not automatically share saved authentication, sessions, or history
from the standard Codex home. You may need to sign in again, and your existing
sessions and history will not automatically appear there. aix does not currently
share or migrate state between Codex homes. This behavior was checked with
`codex-cli 0.157.0`; other versions are unverified.

#### Codex app-server with ChatGPT plan authentication

OpenAI documents ChatGPT-plan token sharing for Codex app-server using a custom
Responses provider. Opt into aix's local gateway to keep the real access token
inside aix and observe usage:

```toml
[tools.codex]
command = "codex"
api_format = "openai"

[tools.codex.chatgpt]
transport = "local_gateway"
access_token_env = "ACCESS_TOKEN"
prepend_args = ["app-server", "--listen", "stdio://"]
clear_env = ["OPENAI_API_KEY", "CODEX_API_KEY"]
```

The equivalent Home Manager settings use camelCase:

```nix
programs.aix.tools.codex = {
  command = "codex";
  apiFormat = "openai";
  chatgpt = {
    transport = "local_gateway";
    accessTokenEnv = "ACCESS_TOKEN";
    prependArgs = [ "app-server" "--listen" "stdio://" ];
    clearEnv = [ "OPENAI_API_KEY" "CODEX_API_KEY" ];
  };
};
```

The local-gateway transport injects one-shot Codex `-c` overrides for a unique
Responses provider, the ephemeral loopback base URL, and `env_key =
"ACCESS_TOKEN"`. That variable contains only the random local bearer; aix fetches
or refreshes the real access token for each request to the public OpenAI API.
No `codex login` is needed, and aix does not write Codex config, auth, or session
files or create a separate `CODEX_HOME`. Other ChatGPT-bound tools remain direct
by default; local-gateway mode is currently limited to Codex app-server over
stdio and the existing OpenCode adapter.

Dry-run displays `transport: local_gateway` and that a dynamic local Responses
provider/base URL will be used, but no actual URL or credential. Codex CLI
0.157.0 was checked for the `app-server --listen stdio:// -c ...` syntax. The
CLI-only check did not perform inference; a live inference/tool/refresh smoke
test still needs an eligible ChatGPT account. Local usage events report request
and token counts without inventing a USD cost; account-level plan usage remains
in ChatGPT **Settings → Usage**. See OpenAI's [Codex app-server guide](https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server)
and [preview limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)
for the current provider schema and SIWC request contract.

#### OpenCode v2 with ChatGPT plan authentication

OpenCode v2 uses a process-local `aix-chatgpt` Responses provider rather than
its built-in `openai` provider. This keeps any saved OpenCode login untouched
and prevents it from overriding the selected aix profile. Configure an
effective default model for the profile and bind the logical `opencode` tool:

```toml
[profiles.personal]
auth = { type = "chatgpt" }

[profiles.personal.models]
default = "gpt-5-codex"

[tools.opencode]
command = "opencode"
api_format = "openai"

[tools.opencode.chatgpt]
access_token_env = "AIX_OPENCODE_BRIDGE_TOKEN"
```

Then run:

```bash
aix opencode personal
# Or select it for the shell first:
aix use personal
aix opencode
aix opencode personal --dry-run
```

The dry-run reports that the ephemeral bridge/provider would be used and lists
environment variable names only. It does not start a listener or read/refresh
ChatGPT tokens. On a real launch, aix starts a loopback-only bridge on an
ephemeral port, injects its process-local OpenCode config through
`OPENCODE_CONFIG_CONTENT`, and gives OpenCode only a random bearer credential
that is valid for that local bridge. The real ChatGPT access token stays in aix;
`AuthService` supplies a current token for each upstream request, including
after refresh. The only upstream destination is the public
`https://api.openai.com/v1` Responses API. OpenCode's config, credentials,
sessions, and other user state are not edited.

The bridge streams SSE, enforces `store: false` and `stream: true`, moves text
system input into `instructions` without dropping its content, and removes the
fields disallowed by the current [SIWC preview contract](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations).
Supported function/custom tools and their history are preserved. Unsupported
hosted Responses tools fail locally rather than being silently removed.
Only the profile's effective default model is configured; OpenCode model
catalog synchronization is not included.

The runtime provider shape is based on the OpenCode
[v2.0.22 provider configuration](https://opencode.ai/v2/docs/providers/). The
automated bridge tests use a fake upstream. A real-account, multi-turn live
smoke test against OpenCode v2.0.22 remains a manual release check; the host's
installed version may be checked with `opencode --version`.

No tool entries or harness adapters are preconfigured. API-key profiles retain
the legacy fallback (`claude` gets Anthropic variables; other names get OpenAI
variables), while an unconfigured `aix run` command receives both formats.

### Cache config

```toml
# Optional: cache aix spend/status LiteLLM responses on disk (default: 1-hour TTL, enabled)
[cache]
ttl_secs = 3600  # seconds before a cached response is considered stale; 0 = never expires
disabled = false # set to true to always fetch fresh data
```

---

## Secret sources

Every secret-backed field (`api_key`, `base_url`, profile `env`, and tool `env` values) accepts one of four forms. Profile `label` supports the same dynamic forms:

| Source | Syntax | Notes |
|--------|--------|-------|
| Direct | `"sk-literal-string"` | Plain string, no expansion. Use only for non-sensitive / throwaway values. |
| Env var | `{ env = "VAR_NAME" }` | Read from the process environment at runtime. |
| File | `{ file = "/path/to/file" }` | Read file contents; `~` is expanded; trailing newline stripped. |
| Command | `{ command = "some-cli read secret/path" }` | Run command via shell; stdout becomes the secret; trailing newline stripped. |

Only one source per field is allowed. Mixing sources in the same field is a config validation error.

---

## Environment variables emitted

For API-key profiles, `aix env` and `aix exec` always inject all seven variables.
These commands reject ChatGPT profiles instead of exporting OAuth credentials.

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
LiteLLM-aware config formats). They are included for API-key launches,
regardless of tool name or selected API format.

Named-tool subcommands use a matching `[tools.<name>]` configuration when
present. Its `api_format` selects Anthropic, OpenAI, or both; otherwise the
legacy fallback is Anthropic for `claude` and OpenAI for other names. `aix run`
uses a matching tool entry too; an unconfigured API-key command receives both
formats. A ChatGPT profile requires an explicit `chatgpt` binding and receives
no generated API-key variables:

| Launch | Credential variables |
|---|---|
| Configured `aix <tool>` / `aix run -- <tool>` | Selected format from `api_format` |
| Unconfigured `aix claude ...` | Anthropic pair |
| Other unconfigured `aix <tool> ...` | OpenAI pair |
| Unconfigured `aix run -- CMD` | Both Anthropic and OpenAI pairs |
| Configured ChatGPT tool | Only the access token under `access_token_env`, plus configured profile/tool env |
| Unconfigured ChatGPT tool | Rejected; no token or API-key fallback |

`AIX_API_KEY` and `AIX_BASE_URL` are not generated by aix. Tools that previously
read those variables should switch to the `ANTHROPIC_*` or `OPENAI_*` equivalents.

---

## Shell integration

### Profile switching

Install the lightweight shell wrapper once. It intercepts `aix use` to set only
`AIX_PROFILE` in the current shell; every other invocation is passed to the aix
binary unchanged. Switching profiles checks profile names only and does not
resolve API keys, base URLs, or custom profile variables.

#### POSIX sh / bash / zsh

```sh
# Pick the shell matching your current session.
eval "$(command aix init zsh)"

aix use work
aix current                 # prints: work
aix current --format short  # same one-line output, suitable for prompts
aix use --clear             # unsets AIX_PROFILE
```

Use `command aix init sh` or `command aix init bash` for those shells.

#### Fish

```fish
command aix init fish | source
aix use work
aix current
```

#### Nushell

Nushell's `source` command needs a file path, so save the generated wrapper once:

```nu
mkdir ~/.config/aix
^aix init nu | save --force ~/.config/aix/init.nu
source ~/.config/aix/init.nu

aix use work
aix current
```

#### PowerShell

```powershell
aix init powershell | Invoke-Expression
aix use work
aix current
```

`aix current --json` uses the standard JSON envelope and returns `name`, `label`,
and `source` (`env`, `default`, or `none`) in `data`. If no valid profile is
selected, human output is `none` and the JSON `name` and `label` are `null`.

Without shell initialization, `aix use work` prints a POSIX `sh` assignment;
source or eval that output in the current shell if desired. The shell wrapper
selects the correct output format explicitly. Direct calls can choose one with
`--shell sh|bash|zsh|fish|nu|powershell`; `--format json` prints an object with
`AIX_PROFILE` set to the selected name or `null` when cleared. For example:

```sh
aix use work --shell fish
aix use --clear --shell fish
```

### Explicit credential export with `aix env`

`aix env` remains available for workflows that intentionally need credential
variables in the current shell. Unlike `aix use`, it resolves and prints those
credentials.

#### POSIX sh / bash / zsh

```sh
# Emit and eval (current shell)
eval "$(aix env work)"

# Explicitly request sh format
eval "$(aix env work --format sh)"

# Use default profile
eval "$(aix env)"
```

#### Nushell

```nu
# JSON is the simplest approach — no temp file needed
aix env work --format json | from json | load-env

# Alternatively, use the Nushell format and source a file
# (source requires a literal path, so save first)
aix env work --format nu | save --force /tmp/aix-env.nu
source /tmp/aix-env.nu
```

#### Fish

```fish
# Fish format uses set -x KEY 'value' syntax
aix env work --format fish | source
```

> **Note:** `eval (aix env work --format fish)` does **not** work. Fish command substitution splits output on newlines into separate list elements, and `eval` then joins them with spaces — collapsing all the `set -x` lines into a single malformed `set` call. Use `| source` instead.

#### PowerShell

```powershell
# PowerShell format uses $env:KEY = 'value' syntax
aix env work --format powershell | Invoke-Expression
```

#### Windows Command Prompt (cmd.exe)

```cmd
REM Save to a temp file and call it in the current session
aix env work --format cmd > "%TEMP%\aix-env.cmd" && call "%TEMP%\aix-env.cmd"

REM Use default profile
aix env --format cmd > "%TEMP%\aix-env.cmd" && call "%TEMP%\aix-env.cmd"
```

> **Note:** Values containing `%` are safe — the output doubles them to `%%` so `SET` interprets them correctly. Values containing `"` use a `""` encoding that works on modern Windows 10/11 cmd.exe but is not guaranteed on all NT versions. If your API key or base URL contains a literal double-quote (rare in practice), use `--format powershell` instead.

#### Inspect without loading (JSON)

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

`aix <tool>` works with any binary on `$PATH`; tools do not need to be configured.
If a matching `[tools.<name>]` entry exists, aix uses its executable, credential
format, and extra environment. Otherwise it preserves the legacy fallback:

- `aix claude ...` → Anthropic and LiteLLM variables
- `aix <anything else> ...` → OpenAI and LiteLLM variables

```bash
# aix <tool> [PROFILE] [--dry-run] [-- TOOL_ARGS...]

aix claude work -- --model claude-opus-4
aix opencode work -- --model gpt-4o
aix aider work -- --no-auto-commits
aix goose work -- session start
aix my-new-agent work -- --prompt "Hello"

# A configured logical name can launch a different executable.
aix review work -- --summary

# Interactive profile picker (when both stdin and stdout are TTYs)
aix claude -- chat

# Dry-run: print what would run, never print secrets
aix opencode work --dry-run
```

The `[tools]` map is otherwise generic launch wiring. ChatGPT-authenticated
OpenCode and Codex app-server entries can use their narrow SIWC bridge adapters
described above. Other tools do not get harness-specific adapters or workflows.

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

## Diagnostics

`aix doctor [PROFILE]` runs read-only checks for config discovery and validation, profile and secret resolution, gateway authentication and model discovery, LiteLLM management access, and cache-directory writability. It never opens the profile picker. If no profile is available from the positional argument, `--profile`/`AIX_PROFILE`, or `default_profile`, the profile check fails instead of prompting.

```bash
# Human-readable checks
aix doctor
aix doctor work

# Stable JSON report (global --json may appear before or after the command)
aix doctor work --json
aix --json doctor work
```

Each row has one of four statuses: `PASS` means the check succeeded; `FAIL` includes a remediation hint; `SKIP` means a prerequisite failed; and `N/A` means the check does not apply. The checks run in this order:

| Check | What it verifies |
|---|---|
| `config_discovery` | A config path can be found from `--config`, `AIX_CONFIG`, or the platform config directory. |
| `config_validation` | The config can be parsed and structurally validated, and its configured `env_files` can be loaded. |
| `profile_selection` | The requested/global/default profile exists; no interactive picker is used. |
| `api_key` | The selected profile's API-key source resolves to a non-empty value. The value is never displayed. |
| `base_url` | The profile or endpoint base-URL source resolves to a non-empty value. The value is never displayed. |
| `gateway_auth` | An authenticated OpenAI-compatible request to the gateway succeeds. |
| `model_discovery` | `/v1/models` returns a parseable model list with non-empty string IDs. |
| `litellm_admin` | When `endpoint.gateway` is unset or `litellm`, the same safe `/key/info` management capability used by spend is reachable. An explicitly non-LiteLLM gateway is `N/A`. |
| `cache_directory` | The configured/default cache directory can be created and a temporary write probe succeeds; existing data is not deleted. |

Checks that do not require the network have `duration_ms: null`; network checks report elapsed milliseconds. `doctor --json` uses the shared JSON envelope with an ordered `data.checks` array. Successful reports go to stdout; if any check fails, the report goes to stderr so failed commands leave stdout empty. Scripts can inspect failed checks from stderr. The process exits `0` only when every applicable check passes; otherwise it uses the exit category for the first failing layer (config `2`, secret `3`, authentication `4`, gateway/network `5`, cache/internal `1`, or budget/policy `6`). Skipped and not-applicable checks do not fail the command by themselves.

Neither mode prints API keys, resolved secret-backed URLs, secret-file contents, secret-command stdout, or raw upstream error bodies.

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

`aix ask` sends one instruction and any explicitly supplied text context to the selected profile's OpenAI-compatible `/v1/chat/completions` endpoint. `aix prompt NAME` sends the configured preset instruction, optional system message, and explicit context through the same inference pipeline. Neither command caches prompts or responses.

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

Input to `ask` and `prompt` is sent to the configured gateway. Only the instruction, non-TTY stdin, and paths passed through `--file` are read. **aix does not inspect the current project or coding harness, inspect Git state, discover neighboring files, or infer project context.** These are domain-agnostic inference commands; pipes and files are ordinary user-supplied context.

Human output contains only the assistant's text and a trailing newline. Use `--json` for the stable aix JSON envelope, which includes the resolved model, assistant content, and token usage (null when the gateway omits a field).

---

## Managed runs

Use `aix run` when a command needs a stable run ID and local, metadata-only
provenance. The child keeps normal stdin/stdout/stderr behavior; `aix exec`
remains the stateless wrapper and does not create run records.

```sh
aix --profile work run \
  --name "nightly review" \
  --workflow verification \
  --task-id task-42 \
  --tag nightly --tag ci \
  -- review --summary

# List the newest 20 runs, or request JSON for scripts.
aix runs
aix runs --json --limit 10

# Inspect one complete non-secret record.
aix runs show RUN_ID --json
```

When the command token matches `[tools.<name>]`, `run` uses that tool's launch
configuration. An unconfigured command receives both credential formats, as
with `aix exec`. The child receives `AIX_RUN_ID`, plus the optional
`AIX_RUN_NAME`, `AIX_WORKFLOW`, `AIX_TASK_ID`, and JSON-array `AIX_RUN_TAGS`.

Records are stored under the platform state directory; set `AIX_STATE_DIR` to
override it. Records include run metadata and lifecycle status, but never
command arguments, prompts, stdin/stdout/stderr, API keys, base URLs, or secret
environment values. Listing is local and does not contact the gateway. `aix`
does not manage worktrees, tasks, retries, or orchestration.

---

## Global options

`--profile` selects a profile and can be used before or after a command. Profile resolution is positional profile, then `--profile`/`AIX_PROFILE`, then `default_profile`; without `--non-interactive`, a TTY-only picker remains the final fallback.

`--non-interactive` disables the profile picker. If a command needs a profile and none resolves from its positional profile, `--profile`/`AIX_PROFILE`, or `default_profile`, it fails immediately with a selection error instead of prompting or waiting for stdin. This is useful for scripts and CI. Without this flag, existing TTY detection still controls whether the picker is available.

`--timeout DURATION` sets the maximum duration for each HTTP request made by `aix`. It accepts milliseconds, seconds, or minutes (for example, `750ms`, `5s`, or `2m`) and defaults to `30s`. It does not limit subprocesses started by `aix exec`, `aix run`, or a named tool.

Both flags are global and can appear before or after regular subcommands.

---

## JSON output and exit codes

The global `--json` flag is currently supported by `current`, `profiles`, `spend`, `models`, `status`, `doctor`, `usage`, `ask`, `prompt`, and `runs`. It can appear before or after the command. Existing JSON forms remain supported. Other commands with their own output options, such as `aix env --format json`, keep those existing formats; using the global `--json` flag with an unsupported command is a validation error.

JSON mode writes exactly one JSON document to stdout on success. Errors and diagnostics go to stderr, and a failed JSON command leaves stdout empty; `doctor` sends its failed-check report to stderr so callers can inspect the findings there. The envelope is owned by `aix`:

```json
{
  "schema_version": 1,
  "command": "profiles",
  "data": [
    { "name": "work", "label": "Work" }
  ]
}
```

`schema_version` is an integer that starts at `1`; `command` is the canonical command name. `current.data` contains `name`, `label`, and `source` (`env`, `default`, or `none`); without a selection, `name` and `label` are `null`. `profiles.data` contains profile names and labels only. `spend.data` contains the selected key's `spend` and optional `max_budget` when a key matches, or the gateway's user totals otherwise. `models.data` contains sorted `{ "id" }` entries and the requested `filter` (or `null` when no filter was used). `status.data` contains stable profile, gateway/probe, key-suffix, spend/budget, and cache fields. `doctor.data.checks` contains ordered check objects with `name`, `status`, `message`, and `duration_ms`; failed doctor reports go to stderr so automation can inspect the findings there. Optional metadata and unavailable spend values are represented as `null` with an explicit spend status; cache source is `live`, `cache`, or `none`. `usage.data` contains the historical date range, aggregates, daily series, and model aggregates described above. `prompt --list` returns sorted `{ "name", "model", "has_system" }` entries without preset bodies; prompt execution returns the same resolved `model`, assistant `content`, and `usage` fields as `ask`. `runs.data` contains non-secret records only; `runs show` returns one complete record. These commands do not expose raw upstream responses, credentials, API-key breakdowns, or unrelated upstream fields.

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
When LiteLLM reports a budget-exceeded response, `aix spend` returns exit code `6`. Both human and JSON modes leave stdout empty and report the error on stderr.

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

Cache entries are replaced atomically so an interrupted write does not corrupt the previous entry. On Unix, aix restricts cache directories to owner access and cache files to owner read/write access; other platforms use their available filesystem defaults.

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

### ChatGPT OAuth credentials

The auth store defaults to the platform's local application-data directory under
`aix/auth`; set `AIX_AUTH_DIR` to use another location. On Unix, the auth
directory is mode `0700` and credential files are mode `0600`. The store keeps
the issued client ID, verified account identity, scopes, and OAuth tokens as
local JSON. They are protected by filesystem permissions, **not encrypted at
rest**. Protect the account and storage device accordingly.

Login uses a loopback callback with OAuth state and PKCE verification. The ID
token signature, issuer, audience, expiry, and nonce are checked before the
identity is saved. Refreshes serialize per profile and persist rotated tokens
atomically. Status is offline and omits token values. Logout clears local token
values even if remote revocation cannot be confirmed; in that case aix prints a
warning to disconnect aix from ChatGPT settings if needed.

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

### In-memory secrets are zeroized on drop

Resolved secret values use memory types that zero their contents when dropped. API keys are not written to disk by aix or printed in logs. ChatGPT tokens are intentionally persisted in the owner-only auth store described above; dry-run never prints token values and shows environment variable names, not values.
