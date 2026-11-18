# Configuration

`aix` reads one config file that defines named profiles, the gateway endpoint, and where each secret comes from. This page covers the file location, the core format, secret sources, and model settings. Other config sections are documented next to the features they control:

| Section | Documented in |
|---|---|
| `auth = { type = "chatgpt" }` profiles | [ChatGPT authentication](chatgpt.md) |
| `[tools.<name>]`, `[profiles.<name>.tool_configs]` | [Running AI tools](tools.md) |
| `[prompts.<name>]` | [One-shot inference](inference.md#reusable-prompt-presets) |
| `[run_policies.<name>]` | [Managed runs](runs.md#run-policies) |
| `[cache]` | [Gateway commands](gateway.md#cache-configuration) |

If you deploy with Nix, the [home-manager module](nix.md) generates this file for you.

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
# Optional gateway metadata. `aix spend` uses the gateway's native key-info endpoint;
# `aix usage` reads local events and does not contact the gateway.
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

## Secret sources

Every secret-backed field (`api_key`, `base_url`, profile `env`, and tool `env` values) accepts one of four forms. Profile `label` supports the same dynamic forms:

| Source | Syntax | Notes |
|--------|--------|-------|
| Direct | `"sk-literal-string"` | Plain string, no expansion. Use only for non-sensitive / throwaway values. |
| Env var | `{ env = "VAR_NAME" }` | Read from the process environment at runtime. |
| File | `{ file = "/path/to/file" }` | Read file contents; `~` is expanded; trailing newline stripped. |
| Command | `{ command = "some-cli read secret/path" }` | Run command via shell; stdout becomes the secret; trailing newline stripped. |

Only one source per field is allowed. Mixing sources in the same field is a config validation error.

## Model defaults and aliases

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

`aix ask` uses this configuration and shared resolver for [one-shot inference](inference.md).
