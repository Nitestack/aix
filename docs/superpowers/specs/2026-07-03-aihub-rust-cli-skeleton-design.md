# Design: aix Rust CLI Skeleton (Ticket 02)

**Date:** 2026-07-03  
**Status:** Approved  
**Scope:** Scaffold only — no Nix behavior replaced, no real handlers implemented.

---

## Goals

Create a standalone Rust CLI package for `aix` under a new Cargo workspace root. The skeleton:

- Parses all planned subcommands via `clap` derive
- Returns a clear "not yet implemented" error from every handler
- Passes `cargo fmt --check`, `cargo clippy`, and `cargo test`
- Does not touch any existing Nix modules

---

## Workspace Layout

```
/Cargo.toml          # workspace root: members = ["tools/aix"]
/Cargo.lock

tools/aix/
  Cargo.toml         # package: name = "aix", edition = "2021"
  src/
    main.rs          # color-eyre init, clap parse, dispatch
    cli.rs           # Cli struct + all subcommand enums (clap derive)
    config.rs        # stub: Config/Profile types, load() placeholder
    error.rs         # AihubError (thiserror)
    commands/
      mod.rs
      profiles.rs
      env.rs
      shell.rs
      exec.rs
      claude.rs
      pi.rs
      config.rs      # config path / validate
  tests/
    cli_help.rs      # assert_cmd integration tests
```

---

## CLI Structure

Global flags on the root struct, inherited by all subcommands:

```
aix [--profile <name>] [--config <path>] <subcommand>
```

| Flag | Env var | Default |
|------|---------|---------|
| `--profile <name>` | `AIX_PROFILE` | none (future: interactive picker) |
| `--config <path>` | `AIX_CONFIG` | none (future: XDG default) |

### Subcommands

| Invocation | Description |
|---|---|
| `aix profiles` | List available profiles |
| `aix env [--format sh\|json\|nu\|fish\|powershell]` | Print env vars for the selected profile |
| `aix shell` | Launch a shell with profile env set |
| `aix exec -- <cmd> [args...]` | Exec a command with profile env set |
| `aix claude -- [args...]` | Exec `claude` with profile env set |
| `aix pi -- [args...]` | Exec `pi` with profile env set |
| `aix config path` | Print resolved config file path |
| `aix config validate` | Validate the config file |

`aix` with no subcommand prints help and exits 2 (clap default).

### Clap types (abbreviated)

```rust
#[derive(Parser)]
pub struct Cli {
    #[arg(long, short, global = true, env = "AIX_PROFILE")]
    pub profile: Option<String>,
    #[arg(long, global = true, env = "AIX_CONFIG")]
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    Profiles,
    Env { #[arg(long, value_enum, default_value = "sh")] format: EnvFormat },
    Shell,
    Exec   { #[arg(last = true)] args: Vec<String> },
    Claude { #[arg(last = true)] args: Vec<String> },
    Pi     { #[arg(last = true)] args: Vec<String> },
    Config { #[command(subcommand)] action: ConfigAction },
}

#[derive(Subcommand)]
pub enum ConfigAction { Path, Validate }

#[derive(ValueEnum, Clone)]
pub enum EnvFormat { Sh, Json, Nu, Fish, Powershell }
```

---

## Error Handling

- `color-eyre` installed in `main()` for beautiful terminal output.
- `thiserror`-derived `AihubError` in `error.rs` for library-facing types.
- All stub handlers return `Err(AihubError::NotImplemented("...").into())` — exits non-zero with a readable message, no silent no-ops.

```rust
#[derive(Debug, thiserror::Error)]
pub enum AihubError {
    #[error("not yet implemented: {0}")]
    NotImplemented(&'static str),
}
```

---

## Dependencies

```toml
[dependencies]
clap         = { version = "4", features = ["derive", "env"] }
config       = { version = "0.15", default-features = false, features = ["toml", "json", "json5", "ron", "yaml"] }
serde        = { version = "1", features = ["derive"] }
serde_json   = "1"
directories  = "6"
which        = "8"
shellexpand  = "3"
inquire      = { version = "0.9", default-features = false, features = ["crossterm"] }
thiserror    = "2"
color-eyre   = { version = "0.6", default-features = false }
zeroize      = "1"

[dev-dependencies]
assert_cmd   = "2"
assert_fs    = "1"
predicates   = "3"
```

---

## Testing

`tests/cli_help.rs` uses `assert_cmd` to verify:

- `aix --help` → exits 0
- `aix env --help` → exits 0
- `aix shell --help` → exits 0
- `aix exec --help` → exits 0
- `aix profiles --help` → exits 0
- `aix config --help` → exits 0
- `aix` (no args) → exits non-zero

No live endpoint required. No mocking needed at this stage.

---

## Acceptance Criteria

- `cargo test` passes
- `cargo fmt --check` passes
- `cargo clippy --all-targets -- -D warnings` passes
- All `--help` invocations above work
- No existing Nix modules modified

---

## Out of Scope (later tickets)

- Real profile loading from config file
- Secret resolution (env → file → `--secret-cmd`)
- Interactive profile picker (`inquire`)
- Shell env export (`exec`, `shell`, `env` real implementations)
- `keyring` integration
- Nix home-manager module updates
