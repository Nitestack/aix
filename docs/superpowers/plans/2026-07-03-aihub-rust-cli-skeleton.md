# aix Rust CLI Skeleton Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create a Cargo workspace with a `tools/aix` binary package that parses all planned subcommands via clap and stubs every handler with a "not yet implemented" error.

**Architecture:** Root `Cargo.toml` workspace with `tools/aix` as its only member. The binary is structured into `cli.rs` (all clap types), `error.rs` (AihubError), `config.rs` (stub data types), and `commands/` (one module per subcommand). `main.rs` installs color-eyre and dispatches to command handlers. Integration tests in `tests/cli_help.rs` verify all `--help` invocations exit 0.

**Tech Stack:** Rust edition 2021, clap 4 (derive+env), color-eyre 0.6, thiserror 2, serde 1, assert_cmd 2, predicates 3.

---

### Task 1: Create workspace and package scaffolding

**Files:**
- Create: `Cargo.toml`
- Create: `tools/aix/Cargo.toml`
- Create: `tools/aix/src/main.rs`

- [ ] **Step 1: Create workspace Cargo.toml**

Create `Cargo.toml` at the repo root:

```toml
[workspace]
members = ["tools/aix"]
resolver = "2"
```

- [ ] **Step 2: Create package Cargo.toml**

Create `tools/aix/Cargo.toml`:

```toml
[package]
name = "aix"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "aix"
path = "src/main.rs"

[dependencies]
clap        = { version = "4", features = ["derive", "env"] }
config      = { version = "0.15", default-features = false, features = ["toml", "json", "json5", "ron", "yaml"] }
serde       = { version = "1", features = ["derive"] }
serde_json  = "1"
directories = "6"
which       = "8"
shellexpand = "3"
inquire     = { version = "0.9", default-features = false, features = ["crossterm"] }
thiserror   = "2"
color-eyre  = { version = "0.6", default-features = false }
zeroize     = "1"

[dev-dependencies]
assert_cmd = "2"
assert_fs  = "1"
predicates = "3"
```

- [ ] **Step 3: Create minimal main.rs**

Create `tools/aix/src/main.rs`:

```rust
fn main() {}
```

- [ ] **Step 4: Verify it compiles**

From the repo root:

```
cargo check
```

Expected: `Finished` with no errors. Cargo fetches dependencies on the first run — this may take a minute.

---

### Task 2: Write failing integration tests

**Files:**
- Create: `tools/aix/tests/cli_help.rs`

- [ ] **Step 1: Create cli_help.rs**

Create `tools/aix/tests/cli_help.rs`:

```rust
use assert_cmd::Command;
use predicates::str::contains;

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

#[test]
fn help_exits_zero() {
    cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Usage: aix"));
}

#[test]
fn no_args_exits_nonzero() {
    cmd().assert().failure();
}

#[test]
fn profiles_help() {
    cmd().args(["profiles", "--help"]).assert().success();
}

#[test]
fn env_help() {
    cmd().args(["env", "--help"]).assert().success();
}

#[test]
fn shell_help() {
    cmd().args(["shell", "--help"]).assert().success();
}

#[test]
fn exec_help() {
    cmd().args(["exec", "--help"]).assert().success();
}

#[test]
fn claude_help() {
    cmd().args(["claude", "--help"]).assert().success();
}

#[test]
fn pi_help() {
    cmd().args(["pi", "--help"]).assert().success();
}

#[test]
fn config_help() {
    cmd().args(["config", "--help"]).assert().success();
}

#[test]
fn profiles_run_exits_nonzero() {
    cmd().arg("profiles").assert().failure();
}

#[test]
fn env_default_exits_nonzero() {
    cmd().arg("env").assert().failure();
}
```

- [ ] **Step 2: Run tests — verify they fail**

```
cargo test
```

Expected: multiple FAILED. Minimally `help_exits_zero` fails (stdout is empty, doesn't match "Usage: aix"), `no_args_exits_nonzero` fails (empty `main()` exits 0), and all subcommand help tests fail (unknown subcommands).

---

### Task 3: Create error.rs, cli.rs, config.rs, and all command stubs

**Files:**
- Create: `tools/aix/src/error.rs`
- Create: `tools/aix/src/cli.rs`
- Create: `tools/aix/src/config.rs`
- Create: `tools/aix/src/commands/mod.rs`
- Create: `tools/aix/src/commands/profiles.rs`
- Create: `tools/aix/src/commands/env.rs`
- Create: `tools/aix/src/commands/shell.rs`
- Create: `tools/aix/src/commands/exec.rs`
- Create: `tools/aix/src/commands/claude.rs`
- Create: `tools/aix/src/commands/pi.rs`
- Create: `tools/aix/src/commands/config.rs`

- [ ] **Step 1: Create error.rs**

Create `tools/aix/src/error.rs`:

```rust
#[derive(Debug, thiserror::Error)]
pub enum AihubError {
    #[error("not yet implemented: {0}")]
    NotImplemented(&'static str),
}
```

- [ ] **Step 2: Create cli.rs**

Create `tools/aix/src/cli.rs`:

```rust
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "aix", about = "AI Hub CLI — profile-aware wrapper for AI tools")]
pub struct Cli {
    /// Profile to use (overrides AIX_PROFILE env var)
    #[arg(long, short, global = true, env = "AIX_PROFILE")]
    pub profile: Option<String>,

    /// Path to config file (overrides AIX_CONFIG env var)
    #[arg(long, global = true, env = "AIX_CONFIG")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// List available profiles
    Profiles,
    /// Print environment variables for the selected profile
    Env {
        /// Output format
        #[arg(long, value_enum, default_value = "sh")]
        format: EnvFormat,
    },
    /// Launch a shell with profile environment set
    Shell,
    /// Execute a command with profile environment set
    Exec {
        /// Command and arguments to run (after --)
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Run the claude CLI with profile environment set
    Claude {
        /// Arguments to pass to claude (after --)
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Run the pi CLI with profile environment set
    Pi {
        /// Arguments to pass to pi (after --)
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Print the resolved config file path
    Path,
    /// Validate the config file
    Validate,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum EnvFormat {
    Sh,
    Json,
    Nu,
    Fish,
    Powershell,
}
```

- [ ] **Step 3: Create config.rs (stub types)**

Create `tools/aix/src/config.rs`:

```rust
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub profiles: HashMap<String, Profile>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Profile {
    pub base_url: String,
    pub model: String,
}
```

`#![allow(dead_code)]` suppresses warnings because `Config` and `Profile` are not wired up yet. Future note: to access the external `config` crate (TOML loader) from other modules once it is used, prefix with `::config::...` to distinguish from this module.

- [ ] **Step 4: Create commands/mod.rs**

Create `tools/aix/src/commands/mod.rs`:

```rust
pub mod claude;
pub mod config;
pub mod env;
pub mod exec;
pub mod pi;
pub mod profiles;
pub mod shell;
```

- [ ] **Step 5: Create commands/profiles.rs**

Create `tools/aix/src/commands/profiles.rs`:

```rust
use crate::error::AihubError;
use color_eyre::Result;

pub fn run() -> Result<()> {
    Err(AihubError::NotImplemented("profiles").into())
}
```

- [ ] **Step 6: Create commands/env.rs**

Create `tools/aix/src/commands/env.rs`:

```rust
use crate::cli::EnvFormat;
use crate::error::AihubError;
use color_eyre::Result;

pub fn run(_format: EnvFormat) -> Result<()> {
    Err(AihubError::NotImplemented("env").into())
}
```

- [ ] **Step 7: Create commands/shell.rs**

Create `tools/aix/src/commands/shell.rs`:

```rust
use crate::error::AihubError;
use color_eyre::Result;

pub fn run() -> Result<()> {
    Err(AihubError::NotImplemented("shell").into())
}
```

- [ ] **Step 8: Create commands/exec.rs**

Create `tools/aix/src/commands/exec.rs`:

```rust
use crate::error::AihubError;
use color_eyre::Result;

pub fn run(_args: Vec<String>) -> Result<()> {
    Err(AihubError::NotImplemented("exec").into())
}
```

- [ ] **Step 9: Create commands/claude.rs**

Create `tools/aix/src/commands/claude.rs`:

```rust
use crate::error::AihubError;
use color_eyre::Result;

pub fn run(_args: Vec<String>) -> Result<()> {
    Err(AihubError::NotImplemented("claude").into())
}
```

- [ ] **Step 10: Create commands/pi.rs**

Create `tools/aix/src/commands/pi.rs`:

```rust
use crate::error::AihubError;
use color_eyre::Result;

pub fn run(_args: Vec<String>) -> Result<()> {
    Err(AihubError::NotImplemented("pi").into())
}
```

- [ ] **Step 11: Create commands/config.rs**

Create `tools/aix/src/commands/config.rs`:

```rust
use crate::cli::ConfigAction;
use crate::error::AihubError;
use color_eyre::Result;

pub fn run(action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Path => Err(AihubError::NotImplemented("config path").into()),
        ConfigAction::Validate => Err(AihubError::NotImplemented("config validate").into()),
    }
}
```

---

### Task 4: Wire main.rs and verify all tests pass

**Files:**
- Modify: `tools/aix/src/main.rs`

- [ ] **Step 1: Replace main.rs with full implementation**

Replace `tools/aix/src/main.rs` with:

```rust
use clap::Parser;
use cli::{Cli, Command};

mod cli;
mod commands;
mod config;
mod error;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    run(cli)
}

fn run(cli: Cli) -> color_eyre::Result<()> {
    match cli.command {
        Command::Profiles => commands::profiles::run(),
        Command::Env { format } => commands::env::run(format),
        Command::Shell => commands::shell::run(),
        Command::Exec { args } => commands::exec::run(args),
        Command::Claude { args } => commands::claude::run(args),
        Command::Pi { args } => commands::pi::run(args),
        Command::Config { action } => commands::config::run(action),
    }
}
```

- [ ] **Step 2: Run all tests**

```
cargo test
```

Expected — all 11 tests pass:

```
test help_exits_zero              ... ok
test no_args_exits_nonzero        ... ok
test profiles_help                ... ok
test env_help                     ... ok
test shell_help                   ... ok
test exec_help                    ... ok
test claude_help                  ... ok
test pi_help                      ... ok
test config_help                  ... ok
test profiles_run_exits_nonzero   ... ok
test env_default_exits_nonzero    ... ok

test result: ok. 11 passed; 0 failed
```

---

### Task 5: Format and lint

- [ ] **Step 1: Format**

```
cargo fmt --all
```

Expected: no output. If files are modified, that is expected — the formatter is authoritative.

- [ ] **Step 2: Clippy**

```
cargo clippy --all-targets -- -D warnings
```

Expected: `Finished` with zero warnings. If any appear, fix them before proceeding. Common cases:

- Unused import → remove the `use` line.
- `needless_pass_by_ref_mut` or similar on `_args` parameters → the `_` prefix already suppresses the unused-variable lint; clippy may still suggest taking by reference. If so, change `_args: Vec<String>` to `_args: &[String]` in the command stub.

- [ ] **Step 3: Verify tests still pass after any fmt/clippy fixes**

```
cargo test
```

Expected: still 11 passed, 0 failed.

---

### Task 6: Commit

- [ ] **Step 1: Stage and commit**

```
git add Cargo.toml Cargo.lock tools/
git commit -m "feat(aix): add Rust CLI skeleton with clap subcommands"
```

---

### Task 7: Update aix-rust.md convention file

**Files:**
- Modify: `docs/agents/aix-rust.md`

- [ ] **Step 1: Replace the Error handling section**

In `docs/agents/aix-rust.md`, replace:

```markdown
## Error handling

Use `anyhow` for application errors, `thiserror` for library-facing error types.
Emit structured errors to stderr; streaming output to stdout only.
```

With:

```markdown
## Error handling

Use `color-eyre` for application-level errors in `main()` — it produces beautiful terminal output with span traces. Use `thiserror` for library-facing error types in `error.rs`. Do not use `anyhow`.
Emit structured errors to stderr; streaming output to stdout only.
```

- [ ] **Step 2: Commit**

```
git add docs/agents/aix-rust.md
git commit -m "docs(rules): replace anyhow with color-eyre in aix-rust conventions"
```
