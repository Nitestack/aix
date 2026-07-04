# Current `aix` Behavior — Rust CLI

This document describes the behavior of the `aix` Rust CLI as of the current codebase.
The old Nix shell-script implementation has been superseded; see `docs/aix/migration.md` for the transition notes.

---

## Commands

| Invocation | Behavior |
|---|---|
| `aix profiles` | List available profiles (name + label) |
| `aix profiles --json` | Emit profiles as a JSON array (no secrets) |
| `aix env [PROFILE] [--format FORMAT]` | Print env vars to stdout; always emits all 5 vars |
| `aix shell [PROFILE] [--dry-run]` | Launch the user's detected shell with profile env set |
| `aix exec [PROFILE] [--dry-run] -- CMD...` | Exec a command with profile env set |
| `aix claude [PROFILE] [--dry-run] [-- args...]` | Exec `claude` with Anthropic credentials only |
| `aix <tool> [PROFILE] [--dry-run] [-- args...]` | Exec `<tool>` with OpenAI credentials only |
| `aix config path` | Print the resolved config file path |
| `aix config validate` | Validate the config file and exit |

---

## Profile resolution

Profiles are defined in the TOML config file (see `docs/aix/example-config.toml`).

**Selection order:**

1. `--profile NAME` global flag (or `AIX_PROFILE` env var)
2. Positional profile argument on the subcommand
3. `default_profile` key in the config file
4. Interactive picker via `inquire::Select` — only when both stdin and stdout are a TTY

If none applies and stdin/stdout are not both TTYs, the CLI exits with an error.

---

## Environment variables emitted

`aix env` and `aix exec` always emit **all five** variables (`ApiFormat::Both`):

| Variable | Value |
|---|---|
| `AIX_PROFILE` | Selected profile name |
| `ANTHROPIC_API_KEY` | Resolved API key |
| `ANTHROPIC_BASE_URL` | Gateway base URL |
| `OPENAI_API_KEY` | Resolved API key |
| `OPENAI_BASE_URL` | Gateway base URL with `/v1` appended |

Named-tool dispatch emits a subset:

| Command | Format | Variables emitted |
|---|---|---|
| `aix claude` | Anthropic | `AIX_PROFILE`, `ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL` |
| `aix <other>` | OpenAI | `AIX_PROFILE`, `OPENAI_API_KEY`, `OPENAI_BASE_URL` |

`AIX_API_KEY` and `AIX_BASE_URL` are **never** emitted.

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

All secret fields (`api_key`, `base_url`, profile `label`) accept any of:

| TOML form | Resolved from |
|---|---|
| `"literal"` | Literal string (avoid for secrets — appears in config file) |
| `{ env = "VAR" }` | Environment variable |
| `{ file = "/run/secrets/..." }` | File contents (trailing newline stripped) |
| `{ command = "op read ..." }` | stdout of a shell command (trailing newline stripped) |

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
