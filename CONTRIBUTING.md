# Contributing

## Development setup

The repository's flake provides the Rust toolchain and the tools CI uses:

```sh
nix develop
```

Without Nix, install a stable Rust toolchain with `rustfmt` and `clippy`. Nix is
only needed for the home-manager module and its checks.

```sh
cargo build                                   # binary at target/debug/aix
cargo test                                    # unit + integration tests
cargo fmt --all
cargo clippy --all-targets -- -D warnings
nix flake check                               # home-manager rendering checks
```

CI runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and
`cargo test`. Tests never need a live gateway; use `wiremock` for HTTP.

Read [ARCHITECTURE.md](ARCHITECTURE.md) for module boundaries and invariants,
and [docs/releasing.md](docs/releasing.md) for how releases are cut.

## Commits

Use [Conventional Commits](https://www.conventionalcommits.org/). The commit
type decides whether a change shows up in [`CHANGELOG.md`](CHANGELOG.md):
`feat`, `fix`, and `perf` are included; `docs`, `ci`, `chore`, `build`, `test`,
`style`, and `refactor` are skipped. Keep commits small: one logical change
each.

## PR checklist

### Invariants

- [ ] No Nix paths hardcoded in Rust (`/run/secrets`, `/nix/store`, etc.)
- [ ] No profile names hardcoded in Rust (`"work"`, `"personal"`, etc.)
- [ ] No JSON hand-rolled — use `serde_json`
- [ ] No silent `.env` auto-loading — user must opt in via `env_files` in config
- [ ] No secret values in logs, errors, or non-env output
- [ ] `aix use`/`aix current` do not resolve or export credentials
- [ ] Run history stores metadata only: no args, prompts, streams, or resolved secrets

### Adding a new provider or gateway

- [ ] Added as a named profile in user config — no Rust changes needed
- [ ] If a new credential format is required: update `ApiFormat`, config parsing, the Home Manager enum, credential collection, and tests
- [ ] Keep process launching generic; tool-specific executable/format/env wiring belongs in `[tools.<name>]`, not harness-specific branches

### Adding a new output format

- [ ] Format function added in `commands/env.rs`
- [ ] Unit tests for correct syntax and special-character escaping
- [ ] Integration test in `tests/cli_env.rs`

### Adding a new secret source

- [ ] New variant in `SecretSource` with custom `Debug` that hides the value
- [ ] Resolution errors include source identifier (name/path), not the resolved value
- [ ] Unit tests for deserialization and resolution

### Tests and docs

- [ ] Config change? Update all four format fixtures (TOML, YAML, JSON, JSON5)
- [ ] Home Manager config change? Add or update a rendering check
- [ ] Nix file changed? Run `nixfmt` (`nixfmt-rfc-style`)
- [ ] New invariant? Add a named test that asserts it explicitly
- [ ] User-visible change? Update the matching page under [`docs/`](docs/README.md)
- [ ] `cargo fmt --all && cargo clippy --all-targets -- -D warnings` passes

## Manual verification

These scenarios need real shells, installed tools, or real accounts and are not
fully covered by automated tests. Check the ones your change touches before
merging to `main`; every push to `main` publishes a new
[nightly build](docs/installation.md#nightly-builds).

- [ ] **Interactive profile selector** — run a profile-resolving command with no explicit profile, no `AIX_PROFILE`, and no `default_profile` in a real TTY; confirm the picker appears and selecting a profile works.
- [ ] **Shell profile switching** — initialize a supported shell with `aix init`, run `aix use <profile>`, and confirm `aix current` reports it in the parent shell. Confirm only `AIX_PROFILE` persists there; credentials are not resolved or exported.
- [ ] **`aix current`** — verify environment, default, and no-profile cases in a non-interactive session; confirm it never opens the picker.
- [ ] **`aix shell`** — run `aix shell <profile>` in a real shell; confirm the child receives `AIX_PROFILE` and the seven generated credential variables (not `AIX_API_KEY`/`AIX_BASE_URL`).
- [ ] **Configured named tool** — configure a generic `[tools.<name>]` entry with an executable alias and non-default `api_format`; confirm `aix <name>` launches that executable with tool env. Also confirm unconfigured `claude` and another tool retain their legacy fallbacks.
- [ ] **Managed run** — run an interactive child with `aix run -- ...`; confirm streams stay attached, `AIX_RUN_ID` reaches the child, and `aix runs show` displays metadata without command arguments or output.
- [ ] **Local usage** — run `aix usage` for an API-key or ChatGPT profile with no usable gateway/key secret; confirm it reports local events without contacting the gateway.
- [ ] **Native key budget** — against a gateway implementing `GET /v1/key/info`, confirm spend/status use the authenticated key's native budget response and expose no other credential data.
- [ ] **PowerShell env output** — run `aix env <profile> --format powershell` in a real PowerShell session and pipe to `Invoke-Expression`; confirm `$env:AIX_PROFILE` is set.
- [ ] **Nushell env loading** — run `aix env <profile> --format nu` in a real Nushell session and source the output; confirm `$env.AIX_PROFILE` is set.
- [ ] **cmd.exe env output** — run `aix env <profile> --format cmd > "%TEMP%\aix-env.cmd" && call "%TEMP%\aix-env.cmd"` in a real cmd.exe session; confirm `%AIX_PROFILE%` is set.
- [ ] **`aix upgrade`** — for each install method you touched, install an older release that way, run `aix upgrade --check`, then `aix upgrade`, and confirm `aix --version` reports the new release. On Windows, also confirm the Scoop/winget hand-off window and that `aix.exe.old` is removed on the next run.

Scripted procedures for checks that need real tools or accounts live in
[`tests/manual/`](tests/manual), for example the
[Codex local gateway smoke test](tests/manual/codex_local_gateway_smoke.md).
