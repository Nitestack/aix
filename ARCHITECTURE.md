# Architecture checklist for PRs

## Manual verification before release

These scenarios require a real interactive environment and cannot be covered by automated tests. Verify each before tagging a release.

- [ ] **Interactive profile selector** — run `aix exec` (or `aix env`) with no `--profile` flag and no `default_profile` in a real TTY; confirm the `inquire` picker appears and selecting a profile works.
- [ ] **`aix shell`** — run `aix shell <profile>` in a real shell; confirm the sub-shell launches with `AIX_PROFILE`, `AIX_API_KEY`, and `AIX_BASE_URL` set.
- [ ] **`aix claude` with `claude` installed** — run `aix claude <profile> -- --version`; confirm claude receives the Anthropic env vars.
- [ ] **`aix pi` with `pi` installed** — run `aix pi <profile> -- --version`; confirm pi receives the env vars.
- [ ] **PowerShell env output** — run `aix env <profile> --format powershell` in a real PowerShell session and pipe to `Invoke-Expression`; confirm `$env:AIX_PROFILE` is set.
- [ ] **Nushell env loading** — run `aix env <profile> --format nu` in a real Nushell session and source the output; confirm `$env.AIX_PROFILE` is set.
- [ ] **cmd.exe env output** — run `aix env <profile> --format cmd > "%TEMP%\aix-env.cmd" && call "%TEMP%\aix-env.cmd"` in a real cmd.exe session; confirm `%AIX_PROFILE%` is set.

---



Before submitting a PR, confirm each item below.

## Invariants

- [ ] No Nix paths hardcoded in Rust (`/run/secrets`, `/nix/store`, etc.)
- [ ] No profile names hardcoded in Rust (`"swtb"`, `"work"`, etc.)
- [ ] No JSON hand-rolled — use `serde_json`
- [ ] No silent `.env` auto-loading — user must opt in via `env_files` in config
- [ ] No secret values in logs, errors, or non-env output

## Adding a new provider or gateway

- [ ] Added as a named profile in user config — no Rust changes needed
- [ ] If a new `ApiFormat` variant is required: added to `collect_vars()` only
- [ ] No new conditional branches in `launch.rs`, `exec.rs`, `claude.rs`, or `pi.rs`

## Adding a new output format

- [ ] Format function added in `commands/env.rs`
- [ ] Unit tests for correct syntax and special-character escaping
- [ ] Integration test in `tests/cli_env.rs`

## Adding a new secret source

- [ ] New variant in `SecretSource` with custom `Debug` that hides the value
- [ ] Resolution errors include source identifier (name/path), not the resolved value
- [ ] Unit tests for deserialization and resolution

## Tests

- [ ] Config change? Update all four format fixtures (TOML, YAML, JSON, JSON5)
- [ ] New invariant? Add a named test that asserts it explicitly
- [ ] `cargo fmt --all && cargo clippy --all-targets -- -D warnings` passes

## Full reference

See [`docs/aix/architecture.md`](../../docs/aix/architecture.md) for module
boundaries, invariants, and rationale.
