# Migration: Nix shell wrapper → Rust CLI

This document records the behavioral delta between the retired `aix` shell wrapper and the Rust replacement. The old wrapper's Nix module is no longer part of this repository.

---

## Parity checklist

| Behavior | Old wrapper | Rust CLI | Test file |
|---|---|---|---|
| Rust `env <profile> --format sh` emits 7 variables | ✓ (5 vars: AIX_*+ ANTHROPIC_*) | ✓ (AIX_PROFILE + ANTHROPIC_*+ OPENAI_* + LITELLM_*) | `cli_parity.rs`, `cli_env.rs` |
| `env <profile> --format json` produces valid JSON | ✗ (broken) | ✓ | `cli_env.rs` |
| `env` with missing profile → non-zero exit | ✓ | ✓ | `cli_env.rs`, `cli_profiles.rs` |
| `env` with unknown profile → non-zero exit | ✓ | ✓ | `cli_env.rs` |
| Missing API key → non-zero exit, names the variable | partial* | ✓ | `cli_parity.rs` |
| Missing base URL → non-zero exit, names the variable | partial* | ✓ | `cli_parity.rs` |
| Duplicate profile labels → non-zero exit | ✓ | ✓ | `cli_profiles.rs` |
| Non-interactive (no TTY) with no profile → non-zero, no hang | ✓ | ✓ | `cli_profiles.rs`, `cli_parity.rs` |
| `exec/shell -- cmd` runs command with env set | ✓ (via `shell`) | ✓ (via `exec`) | `cli_exec.rs`, `cli_parity.rs` |
| Exit code of child process is propagated | ✓ | ✓ | `cli_exec.rs` |
| Secrets never appear in `--dry-run` output | N/A | ✓ | `cli_exec.rs`, `cli_parity.rs` |

\* Old wrapper only checks if the secret file is readable (`[ -r "$path" ]`), not whether
the variable it maps to is set. Empty or absent files give an empty key with no error.

---

## Environment variable name parity

The Rust CLI always emits Anthropic, OpenAI, and LiteLLM credential sets (`aix env` / `aix exec`).
`AIX_API_KEY` and `AIX_BASE_URL` are no longer emitted; tools that read them must switch to
`ANTHROPIC_API_KEY` / `ANTHROPIC_BASE_URL` or `OPENAI_API_KEY` / `OPENAI_BASE_URL`.

| Variable | Old wrapper | Rust CLI |
|---|---|---|
| `AIX_PROFILE` | ✓ | ✓ |
| `AIX_API_KEY` | ✓ | ✗ (removed) |
| `AIX_BASE_URL` | ✓ | ✗ (removed) |
| `ANTHROPIC_API_KEY` | ✓ | ✓ (always) |
| `ANTHROPIC_BASE_URL` | ✓ | ✓ (always) |
| `OPENAI_API_KEY` | ✗ | ✓ (always) |
| `OPENAI_BASE_URL` | ✗ | ✓ (always, `/v1` appended) |
| `LITELLM_API_KEY` | ✗ | ✓ (always) |
| `LITELLM_BASE_URL` | ✗ | ✓ (always, `/v1` appended) |

---

## Intentional differences

### 1. `shell -- cmd` is replaced by `exec`

**Old wrapper:** `aix shell myprofile -- some-command args…` ran `some-command` with the
profile environment set.

**Rust CLI:** `aix shell myprofile` only launches an interactive shell. To run a
one-shot command, use `aix exec myprofile -- some-command args…`.

Rationale: separating "launch a shell" from "exec a command" makes the intent explicit
and avoids ambiguity about whether `--` is required.

### 2. Profile list is dynamic (config file), not baked in at build time

**Old wrapper:** valid profile names were interpolated into the shell script during
`nix build`. Adding a profile required a NixOS rebuild.

**Rust CLI:** profiles come from the config file (`aix.toml`). No rebuild needed.

### 3. Secrets come from config, env vars, or a command — not only from `/run/secrets/`

**Old wrapper:** API key from `/run/secrets/aix/<profile>`, base URL from
`/run/secrets/aix/base-url`. Hard-coded paths; only works on the NixOS host.

**Rust CLI:** each secret-backed config field chooses one resolution strategy:

1. Direct value in config (`api_key = "sk-..."`)
2. Environment variable (`api_key = { env = "MY_VAR" }`)
3. File (`api_key = { file = "/run/secrets/aix/myprofile" }`)
4. Shell command (`api_key = { command = "pass show aix/myprofile" }`)

The current Nix module generates TOML from the configured literal, environment-variable, file, or command sources; it does not impose the legacy `/run/secrets/aix/…` paths.

### 4. `env --format json` now works correctly

**Old wrapper:** the heredoc delimiter was single-quoted (`<<'__AIX_ENV_JSON__'`),
suppressing variable expansion. The output was literal shell syntax, not valid JSON.
The feature was effectively broken and never usable.

**Rust CLI:** JSON is produced by `serde_json`; all values are properly escaped.

### 5. Additional output formats

**Old wrapper:** only `sh` (default) and `json` (broken).

**Rust CLI:** `sh`, `json`, `nu`, `fish`, `powershell`, `cmd`.

### 6. `--format` is validated before profile selection

**Old wrapper:** `--format` was not validated until inside `emit_env`, after
`choose_profile` had already run. An invalid format could trigger an interactive
profile picker and then fail.

**Rust CLI:** `clap` rejects unknown formats before any profile resolution occurs.

### 7. Interactive profile selection uses `inquire` instead of `gum`

**Old wrapper:** required `gum` as a runtime dependency (provided by Nix).

**Rust CLI:** uses the `inquire` crate; no external tool required.

### 8. Non-empty secret validation

**Old wrapper:** checked only that the secret file was readable (`[ -r "$path" ]`).
A readable but empty file produced `AIX_API_KEY=""` silently.

**Rust CLI:** same for `file` sources — an empty file resolves to an empty string
without error. This is a known gap; validation of secret contents is left to callers.

---

## Behaviors preserved exactly

- Non-interactive invocations (piped stdin/stdout) without a profile fail immediately
  with a clear error message; they never hang waiting for input.
- Duplicate profile labels cause a non-zero exit before any secret is read.
- The selected profile name appears as the `AIX_PROFILE` value.
- Child process exit codes are propagated unchanged.
