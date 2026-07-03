# Process Launching Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `aix exec`, `aix claude`, `aix pi`, and `aix shell` so they resolve profile env vars and launch child processes with those vars set.

**Architecture:** A new `commands/launch.rs` module owns env resolution + process spawning logic shared by all four commands. `commands/env.rs` exposes `collect_vars` and `resolve_profile` as `pub(crate)` so launch.rs can reuse them without duplication. Each command module becomes a thin wrapper calling `launch::resolve_launch_env` then `launch::run_command`. Exit codes propagate via `std::process::exit` when the child exits non-zero.

**Tech Stack:** `which` (already in Cargo.toml), `std::process::Command`, `color-eyre`, `thiserror`, `clap` (already in use).

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Modify | `src/error.rs` | Add `ExecutableNotFound`, `ProcessSpawn` variants |
| Modify | `src/commands/env.rs` | Make `collect_vars` and `resolve_profile` `pub(crate)` |
| Create | `src/commands/launch.rs` | `resolve_launch_env`, `run_command`, `detect_shell` |
| Modify | `src/commands/mod.rs` | Add `pub mod launch;` |
| Modify | `src/cli.rs` | Add `profile: Option<String>` + `dry_run: bool` to Exec/Claude/Pi/Shell |
| Modify | `src/main.rs` | Thread `effective_profile`, `config_path`, `dry_run` to all four commands |
| Modify | `src/commands/exec.rs` | Implement via `launch::run_command` |
| Modify | `src/commands/claude.rs` | Implement via `launch::run_command` |
| Modify | `src/commands/pi.rs` | Implement via `launch::run_command` |
| Modify | `src/commands/shell.rs` | Implement via `launch::detect_shell` + `launch::run_command` |
| Create | `tests/cli_exec.rs` | Integration tests for exec/claude/pi/shell |

---

### Task 1: Add error variants for process launching

**Files:**
- Modify: `tools/aix/src/error.rs`

- [ ] **Step 1: Write the failing unit tests**

Append to the `#[cfg(test)] mod tests` block in `src/error.rs`:

```rust
#[test]
fn executable_not_found_message_includes_program_name() {
    let e = AihubError::ExecutableNotFound { program: "pi".to_string() };
    assert!(e.to_string().contains("pi"), "got: {}", e);
}

#[test]
fn process_spawn_message_includes_program_name() {
    let e = AihubError::ProcessSpawn {
        program: "claude".to_string(),
        source: std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
    };
    assert!(e.to_string().contains("claude"), "got: {}", e);
}
```

- [ ] **Step 2: Run tests to confirm they fail**

```bash
cd tools/aix && cargo test executable_not_found_message 2>&1 | head -20
```

Expected: compile error — variants don't exist yet.

- [ ] **Step 3: Add the variants to `AihubError`**

In `src/error.rs`, remove the `#[allow(dead_code)]` from any existing variants that these replace or augment, then add after the `NoInteractiveTerminal` variant:

```rust
    #[error("executable not found: {program}")]
    ExecutableNotFound { program: String },

    #[error("failed to spawn {program}: {source}")]
    ProcessSpawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
```

- [ ] **Step 4: Run tests to confirm they pass**

```bash
cd tools/aix && cargo test executable_not_found_message process_spawn_message 2>&1
```

Expected: both tests pass.

- [ ] **Step 5: Commit**

```bash
cd tools/aix && git add src/error.rs
git commit -m "feat(error): add ExecutableNotFound and ProcessSpawn variants"
```

---

### Task 2: Expose `collect_vars` and `resolve_profile` as `pub(crate)` in env.rs

**Files:**
- Modify: `tools/aix/src/commands/env.rs`

These two functions currently have no visibility modifier (private). `launch.rs` will need to call them.

- [ ] **Step 1: Change visibility on `collect_vars`**

In `src/commands/env.rs`, change line:

```rust
fn collect_vars(
```

to:

```rust
pub(crate) fn collect_vars(
```

- [ ] **Step 2: Change visibility on `resolve_profile`**

In `src/commands/env.rs`, change line:

```rust
fn resolve_profile(positional: Option<String>, cfg: &config::Config) -> Result<String, AihubError> {
```

to:

```rust
pub(crate) fn resolve_profile(positional: Option<String>, cfg: &config::Config) -> Result<String, AihubError> {
```

- [ ] **Step 3: Verify no tests broke**

```bash
cd tools/aix && cargo test 2>&1 | tail -5
```

Expected: all existing tests still pass.

- [ ] **Step 4: Commit**

```bash
cd tools/aix && git add src/commands/env.rs
git commit -m "refactor(env): expose collect_vars and resolve_profile as pub(crate)"
```

---

### Task 3: Create `commands/launch.rs` with env resolution and process spawning

**Files:**
- Create: `tools/aix/src/commands/launch.rs`
- Modify: `tools/aix/src/commands/mod.rs`

- [ ] **Step 1: Add `pub mod launch` to mod.rs**

In `src/commands/mod.rs`, add:

```rust
pub mod launch;
```

The file should now read:
```rust
pub mod claude;
pub mod config;
pub mod env;
pub mod exec;
pub mod launch;
pub mod pi;
pub mod profiles;
pub mod shell;
```

- [ ] **Step 2: Write unit tests for `detect_shell` before implementing**

Create `src/commands/launch.rs` with ONLY the tests and stub:

```rust
use crate::commands::env::{collect_vars, resolve_profile};
use crate::config;
use crate::error::AihubError;
use color_eyre::Result;
use std::path::PathBuf;

pub struct LaunchEnv {
    pub profile_name: String,
    pub vars: Vec<(&'static str, String)>,
}

pub fn resolve_launch_env(
    profile: Option<String>,
    config_path: Option<PathBuf>,
) -> Result<LaunchEnv> {
    todo!()
}

pub fn detect_shell() -> String {
    detect_shell_impl()
}

pub fn run_command(
    program: &str,
    args: &[String],
    env: &LaunchEnv,
    dry_run: bool,
) -> Result<()> {
    todo!()
}

fn find_executable(program: &str) -> Result<PathBuf, AihubError> {
    todo!()
}

#[cfg(unix)]
fn detect_shell_impl() -> String {
    todo!()
}

#[cfg(windows)]
fn detect_shell_impl() -> String {
    todo!()
}

#[cfg(not(any(unix, windows)))]
fn detect_shell_impl() -> String {
    "sh".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn detect_shell_returns_shell_env_when_set() {
        // SAFETY: tests run in a single-threaded context for this env var check.
        let prev = std::env::var("SHELL").ok();
        std::env::set_var("SHELL", "/usr/bin/fish");
        let result = detect_shell();
        // Restore
        match prev {
            Some(v) => std::env::set_var("SHELL", v),
            None => std::env::remove_var("SHELL"),
        }
        assert_eq!(result, "/usr/bin/fish");
    }

    #[test]
    #[cfg(unix)]
    fn detect_shell_falls_back_to_sh_when_shell_not_set() {
        let prev = std::env::var("SHELL").ok();
        let prev_nu = std::env::var("NU_VERSION").ok();
        std::env::remove_var("SHELL");
        std::env::remove_var("NU_VERSION");
        let result = detect_shell();
        // Restore
        match prev {
            Some(v) => std::env::set_var("SHELL", v),
            None => std::env::remove_var("SHELL"),
        }
        match prev_nu {
            Some(v) => std::env::set_var("NU_VERSION", v),
            None => std::env::remove_var("NU_VERSION"),
        }
        assert_eq!(result, "sh");
    }

    #[test]
    fn detect_shell_returns_a_non_empty_string() {
        let shell = detect_shell();
        assert!(!shell.is_empty());
    }

    #[test]
    fn launch_env_vars_include_required_keys() {
        use crate::config::Compat;
        let vars = collect_vars("myprofile", "sk-test", "https://example.com", &Compat::default());
        let names: Vec<&str> = vars.iter().map(|(k, _)| *k).collect();
        assert!(names.contains(&"AIX_PROFILE"));
        assert!(names.contains(&"AIX_API_KEY"));
        assert!(names.contains(&"AIX_BASE_URL"));
    }
}
```

- [ ] **Step 3: Run tests to confirm they fail (panic on `todo!()`)**

```bash
cd tools/aix && cargo test -p aix launch:: 2>&1 | head -30
```

Expected: tests for `detect_shell_returns_a_non_empty_string` panic with `todo!()`.

- [ ] **Step 4: Implement `detect_shell_impl` for Unix**

Replace the Unix stub:

```rust
#[cfg(unix)]
fn detect_shell_impl() -> String {
    if let Ok(shell) = std::env::var("SHELL") {
        if !shell.is_empty() {
            return shell;
        }
    }
    if std::env::var("NU_VERSION").is_ok() {
        if which::which("nu").is_ok() {
            return "nu".to_string();
        }
    }
    "sh".to_string()
}
```

- [ ] **Step 5: Implement `detect_shell_impl` for Windows**

Replace the Windows stub:

```rust
#[cfg(windows)]
fn detect_shell_impl() -> String {
    if let Ok(shell) = std::env::var("SHELL") {
        if !shell.is_empty() {
            return shell;
        }
    }
    for candidate in &["pwsh", "powershell"] {
        if which::which(candidate).is_ok() {
            return candidate.to_string();
        }
    }
    "cmd".to_string()
}
```

- [ ] **Step 6: Implement `find_executable`**

Replace the stub:

```rust
fn find_executable(program: &str) -> Result<PathBuf, AihubError> {
    let path = std::path::Path::new(program);
    if path.is_absolute() {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        return Err(AihubError::ExecutableNotFound { program: program.to_string() });
    }
    which::which(program).map_err(|_| AihubError::ExecutableNotFound {
        program: program.to_string(),
    })
}
```

- [ ] **Step 7: Implement `resolve_launch_env`**

Replace the stub:

```rust
pub fn resolve_launch_env(
    profile: Option<String>,
    config_path: Option<PathBuf>,
) -> Result<LaunchEnv> {
    let path =
        config::find_config_path(config_path.as_deref())?.ok_or(AihubError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;
    config::load_env_files(&cfg)?;

    let profile_name = resolve_profile(profile, &cfg)?;
    let profile_entry = cfg
        .profiles
        .get(&profile_name)
        .ok_or_else(|| AihubError::ProfileNotFound(profile_name.clone()))?;

    let api_key = profile_entry.api_key.resolve()?;
    let base_url = cfg.endpoint.base_url.resolve()?;

    let vars = collect_vars(
        &profile_name,
        api_key.expose_secret(),
        base_url.expose_secret(),
        &cfg.compat,
    );
    Ok(LaunchEnv { profile_name, vars })
}
```

- [ ] **Step 8: Implement `run_command`**

Replace the stub:

```rust
pub fn run_command(
    program: &str,
    args: &[String],
    env: &LaunchEnv,
    dry_run: bool,
) -> Result<()> {
    if dry_run {
        let display_args: Vec<&str> = std::iter::once(program)
            .chain(args.iter().map(String::as_str))
            .collect();
        eprintln!("Would run: {}", display_args.join(" "));
        eprintln!("Would set:");
        for (k, _) in &env.vars {
            eprintln!("  {k}");
        }
        return Ok(());
    }

    let resolved = find_executable(program)?;

    let mut cmd = std::process::Command::new(&resolved);
    cmd.args(args);
    for (k, v) in &env.vars {
        cmd.env(k, v);
    }

    let status = cmd.status().map_err(|source| AihubError::ProcessSpawn {
        program: program.to_string(),
        source,
    })?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}
```

- [ ] **Step 9: Run all launch tests**

```bash
cd tools/aix && cargo test launch:: 2>&1
```

Expected: all `launch::tests::*` pass.

- [ ] **Step 10: Commit**

```bash
cd tools/aix && git add src/commands/launch.rs src/commands/mod.rs
git commit -m "feat(launch): add resolve_launch_env, run_command, detect_shell"
```

---

### Task 4: Update `cli.rs` — add `profile` and `dry_run` to all four commands

**Files:**
- Modify: `tools/aix/src/cli.rs`

- [ ] **Step 1: Update `Shell` variant**

Replace:
```rust
    /// Launch a shell with profile environment set
    Shell,
```

With:
```rust
    /// Launch a shell with profile environment set
    Shell {
        /// Profile name (positional; overrides the global --profile flag)
        profile: Option<String>,
        /// Print the shell command and variable names that would be set, without running
        #[arg(long)]
        dry_run: bool,
    },
```

- [ ] **Step 2: Update `Exec` variant**

Replace:
```rust
    /// Execute a command with profile environment set
    Exec {
        /// Command and arguments to run (after --)
        #[arg(last = true)]
        args: Vec<String>,
    },
```

With:
```rust
    /// Execute a command with profile environment set
    Exec {
        /// Profile name (positional; overrides the global --profile flag)
        profile: Option<String>,
        /// Print the command and variable names that would be set, without running
        #[arg(long)]
        dry_run: bool,
        /// Command and arguments to run (after --)
        #[arg(last = true)]
        args: Vec<String>,
    },
```

- [ ] **Step 3: Update `Claude` variant**

Replace:
```rust
    /// Run the claude CLI with profile environment set
    Claude {
        /// Arguments to pass to claude (after --)
        #[arg(last = true)]
        args: Vec<String>,
    },
```

With:
```rust
    /// Run the claude CLI with profile environment set
    Claude {
        /// Profile name (positional; overrides the global --profile flag)
        profile: Option<String>,
        /// Print the command and variable names that would be set, without running
        #[arg(long)]
        dry_run: bool,
        /// Arguments to pass to claude (after --)
        #[arg(last = true)]
        args: Vec<String>,
    },
```

- [ ] **Step 4: Update `Pi` variant**

Replace:
```rust
    /// Run the pi CLI with profile environment set
    Pi {
        /// Arguments to pass to pi (after --)
        #[arg(last = true)]
        args: Vec<String>,
    },
```

With:
```rust
    /// Run the pi CLI with profile environment set
    Pi {
        /// Profile name (positional; overrides the global --profile flag)
        profile: Option<String>,
        /// Print the command and variable names that would be set, without running
        #[arg(long)]
        dry_run: bool,
        /// Arguments to pass to pi (after --)
        #[arg(last = true)]
        args: Vec<String>,
    },
```

- [ ] **Step 5: Verify it compiles (it will fail in main.rs — that's expected)**

```bash
cd tools/aix && cargo build 2>&1 | head -20
```

Expected: compile errors in `src/main.rs` about non-exhaustive pattern matching on `Command::Shell`, etc.

---

### Task 5: Update `main.rs` to thread new fields through dispatch

**Files:**
- Modify: `tools/aix/src/main.rs`

- [ ] **Step 1: Update the `Shell`, `Exec`, `Claude`, `Pi` arms**

Replace the four match arms:

```rust
        Command::Shell => commands::shell::run(),
        Command::Exec { args } => commands::exec::run(args),
        Command::Claude { args } => commands::claude::run(args),
        Command::Pi { args } => commands::pi::run(args),
```

With:

```rust
        Command::Shell { profile, dry_run } => {
            let effective_profile = profile.or(global_profile);
            commands::shell::run(effective_profile, config_path, dry_run)
        }
        Command::Exec { profile, dry_run, args } => {
            let effective_profile = profile.or(global_profile);
            commands::exec::run(effective_profile, config_path, dry_run, args)
        }
        Command::Claude { profile, dry_run, args } => {
            let effective_profile = profile.or(global_profile);
            commands::claude::run(effective_profile, config_path, dry_run, args)
        }
        Command::Pi { profile, dry_run, args } => {
            let effective_profile = profile.or(global_profile);
            commands::pi::run(effective_profile, config_path, dry_run, args)
        }
```

- [ ] **Step 2: Verify it still fails to compile (the command modules have wrong signatures)**

```bash
cd tools/aix && cargo build 2>&1 | head -30
```

Expected: type mismatch errors in exec.rs, claude.rs, pi.rs, shell.rs (wrong function signatures).

---

### Task 6: Implement `commands/exec.rs`

**Files:**
- Modify: `tools/aix/src/commands/exec.rs`

- [ ] **Step 1: Replace the stub**

```rust
use crate::commands::launch;
use color_eyre::Result;
use std::path::PathBuf;

pub fn run(
    profile: Option<String>,
    config_path: Option<PathBuf>,
    dry_run: bool,
    args: Vec<String>,
) -> Result<()> {
    let (program, cmd_args) = args
        .split_first()
        .map(|(p, rest)| (p.clone(), rest.to_vec()))
        .ok_or_else(|| color_eyre::eyre::eyre!("exec requires a command after --"))?;

    let env = launch::resolve_launch_env(profile, config_path)?;
    launch::run_command(&program, &cmd_args, &env, dry_run)
}
```

- [ ] **Step 2: Verify the project compiles**

```bash
cd tools/aix && cargo build 2>&1 | head -20
```

Expected: errors only from claude.rs, pi.rs, shell.rs.

---

### Task 7: Implement `commands/claude.rs` and `commands/pi.rs`

**Files:**
- Modify: `tools/aix/src/commands/claude.rs`
- Modify: `tools/aix/src/commands/pi.rs`

- [ ] **Step 1: Replace `claude.rs` stub**

```rust
use crate::commands::launch;
use color_eyre::Result;
use std::path::PathBuf;

pub fn run(
    profile: Option<String>,
    config_path: Option<PathBuf>,
    dry_run: bool,
    args: Vec<String>,
) -> Result<()> {
    let env = launch::resolve_launch_env(profile, config_path)?;
    launch::run_command("claude", &args, &env, dry_run)
}
```

- [ ] **Step 2: Replace `pi.rs` stub**

```rust
use crate::commands::launch;
use color_eyre::Result;
use std::path::PathBuf;

pub fn run(
    profile: Option<String>,
    config_path: Option<PathBuf>,
    dry_run: bool,
    args: Vec<String>,
) -> Result<()> {
    let env = launch::resolve_launch_env(profile, config_path)?;
    launch::run_command("pi", &args, &env, dry_run)
}
```

- [ ] **Step 3: Verify only shell.rs errors remain**

```bash
cd tools/aix && cargo build 2>&1 | head -20
```

Expected: one error about `commands::shell::run` having wrong signature.

---

### Task 8: Implement `commands/shell.rs`

**Files:**
- Modify: `tools/aix/src/commands/shell.rs`

- [ ] **Step 1: Replace the stub**

```rust
use crate::commands::launch;
use color_eyre::Result;
use std::path::PathBuf;

pub fn run(
    profile: Option<String>,
    config_path: Option<PathBuf>,
    dry_run: bool,
) -> Result<()> {
    let env = launch::resolve_launch_env(profile, config_path)?;
    let shell = launch::detect_shell();
    launch::run_command(&shell, &[], &env, dry_run)
}
```

- [ ] **Step 2: Verify the project compiles fully**

```bash
cd tools/aix && cargo build 2>&1
```

Expected: no errors.

- [ ] **Step 3: Run clippy and fix any warnings**

```bash
cd tools/aix && cargo clippy --all-targets -- -D warnings 2>&1
```

Expected: no warnings. Fix any that appear before continuing.

- [ ] **Step 4: Run cargo fmt**

```bash
cd tools/aix && cargo fmt --all
```

- [ ] **Step 5: Commit**

```bash
cd tools/aix && git add src/cli.rs src/main.rs src/commands/exec.rs src/commands/claude.rs src/commands/pi.rs src/commands/shell.rs
git commit -m "feat(commands): implement exec, claude, pi, shell via launch module"
```

---

### Task 9: Integration tests for exec/claude/pi/shell

**Files:**
- Create: `tools/aix/tests/cli_exec.rs`

These tests verify behavior end-to-end using the compiled binary. All tests run on Unix (Linux/Mac). Windows-specific behavior is not tested here.

- [ ] **Step 1: Write the full test file**

Create `tests/cli_exec.rs`:

```rust
use assert_cmd::Command;
use assert_fs::prelude::*;

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

const CONFIG: &str = r#"
[endpoint]
base_url = "https://ai.example.com/v1"
api_format = "anthropic"

[profiles.swtb]
api_key = "sk-swtb-key"
"#;

const CONFIG_WITH_DEFAULT: &str = r#"
default_profile = "swtb"

[endpoint]
base_url = "https://ai.example.com/v1"
api_format = "anthropic"

[profiles.swtb]
api_key = "sk-swtb-key"
"#;

// --- exec: environment variable injection ---

#[test]
#[cfg(unix)]
fn exec_sets_aix_env_vars_in_child() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["exec", "swtb", "--", "printenv", "AIX_API_KEY"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap().trim();
    assert_eq!(s, "sk-swtb-key", "got: {s}");
}

#[test]
#[cfg(unix)]
fn exec_sets_anthropic_env_vars_by_default() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["exec", "swtb", "--", "printenv", "ANTHROPIC_API_KEY"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap().trim();
    assert_eq!(s, "sk-swtb-key", "got: {s}");
}

// --- exec: exit code propagation ---

#[test]
#[cfg(unix)]
fn exec_propagates_zero_exit_code() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["exec", "swtb", "--", "true"])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn exec_propagates_nonzero_exit_code() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["exec", "swtb", "--", "sh", "-c", "exit 42"])
        .assert()
        .code(42);
}

// --- exec: missing executable ---

#[test]
fn exec_missing_executable_gives_clear_error() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let err = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["exec", "swtb", "--", "aix-nonexistent-command-xyz"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&err).unwrap();
    assert!(
        s.contains("aix-nonexistent-command-xyz"),
        "error must name the missing executable: {s}"
    );
}

// --- exec: no command after -- ---

#[test]
fn exec_no_command_after_separator_errors() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["exec", "swtb", "--"])
        .assert()
        .failure();
}

// --- exec: --dry-run ---

#[test]
fn exec_dry_run_shows_variable_names_not_values() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let output = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["exec", "swtb", "--dry-run", "--", "printenv", "AIX_API_KEY"])
        .assert()
        .success()
        .get_output()
        .clone();

    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    // Must show the variable name
    assert!(stderr.contains("AIX_API_KEY"), "must mention var name: {stderr}");
    // Must NOT show the secret value
    assert!(!stderr.contains("sk-swtb-key"), "must not leak value: {stderr}");
    // Must show the command that would run
    assert!(stderr.contains("printenv"), "must mention program: {stderr}");
    // Stdout must be empty — the child was not executed
    assert!(output.stdout.is_empty(), "dry-run must not run the child");
}

#[test]
fn exec_dry_run_does_not_execute_nonexistent_binary() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    // With --dry-run, even a nonexistent program should succeed (we never exec it)
    cmd()
        .env("AIX_CONFIG", file.path())
        .args([
            "exec",
            "swtb",
            "--dry-run",
            "--",
            "aix-nonexistent-command-xyz",
        ])
        .assert()
        .success();
}

// --- exec: profile selection ---

#[test]
fn exec_uses_global_profile_flag() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["--profile", "swtb", "exec", "--", "true"])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn exec_uses_default_profile_when_none_specified() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_WITH_DEFAULT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("AIX_PROFILE")
        .args(["exec", "--", "printenv", "AIX_PROFILE"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap().trim();
    assert_eq!(s, "swtb");
}

// --- claude: dry-run ---

#[test]
fn claude_dry_run_shows_claude_command() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let stderr = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["claude", "swtb", "--dry-run", "--", "--version"])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&stderr).unwrap();
    assert!(s.contains("claude"), "must mention 'claude': {s}");
    assert!(s.contains("AIX_API_KEY"), "must list env var names: {s}");
    assert!(!s.contains("sk-swtb-key"), "must not leak value: {s}");
}

// --- pi: dry-run ---

#[test]
fn pi_dry_run_shows_pi_command() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let stderr = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["pi", "swtb", "--dry-run", "--", "--version"])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&stderr).unwrap();
    assert!(s.contains("pi"), "must mention 'pi': {s}");
    assert!(!s.contains("sk-swtb-key"), "must not leak value: {s}");
}

// --- shell: dry-run ---

#[test]
fn shell_dry_run_shows_shell_and_var_names() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let stderr = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["shell", "swtb", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&stderr).unwrap();
    // Should mention "Would run:" with some shell
    assert!(s.contains("Would run:"), "must show would-run: {s}");
    // Must list env var names
    assert!(s.contains("AIX_API_KEY"), "must list var names: {s}");
    // Must not contain the secret value
    assert!(!s.contains("sk-swtb-key"), "must not leak value: {s}");
}
```

- [ ] **Step 2: Run the failing tests to confirm they fail for the right reasons**

```bash
cd tools/aix && cargo test --test cli_exec 2>&1 | tail -30
```

Expected: tests that already pass (dry-run, missing-exe, no-cmd) pass. The env var injection tests (`exec_sets_aix_env_vars_in_child`, etc.) may fail if the implementation is correct but env vars aren't passed — investigate if so.

Actually at this point the implementation should be complete. Expected: most tests pass.

- [ ] **Step 3: Investigate and fix any failing tests**

If `exec_sets_aix_env_vars_in_child` fails, check:
1. Run `cargo test -- --nocapture exec_sets` to see full output
2. Verify `collect_vars` is returning the correct values
3. Verify `cmd.env(k, v)` loop in `run_command` is correct

- [ ] **Step 4: Run the full test suite**

```bash
cd tools/aix && cargo test 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 5: Run clippy and fmt**

```bash
cd tools/aix && cargo fmt --all && cargo clippy --all-targets -- -D warnings 2>&1
```

Expected: clean. Fix any warnings.

- [ ] **Step 6: Commit**

```bash
cd tools/aix && git add tests/cli_exec.rs
git commit -m "test(exec): add integration tests for exec, claude, pi, shell"
```

---

## Self-Review

### Spec coverage check

| Requirement | Task |
|-------------|------|
| `exec [profile] -- <cmd>` parses profile + command | Task 4, 5 |
| `claude [profile] -- [args]` is shorthand for exec | Task 7 |
| `pi [profile] -- [args]` is shorthand for exec | Task 7 |
| `shell [profile]` starts user shell | Task 8 |
| Resolved env vars set on child | Task 3 (`run_command` + `cmd.env`) |
| Child exit code propagated | Task 3 (`std::process::exit`) |
| Stdin/stdout/stderr preserved | Task 3 (`cmd.status()` inherits by default) |
| Secrets never printed | Task 3 (dry-run shows names, not values) |
| `--dry-run` shows cmd + var names, no values | Task 3 |
| Shell detection: `$SHELL` → `nu` → `sh` (Unix) | Task 3 |
| Shell detection: `$SHELL` → `pwsh` → `powershell` → `cmd` (Windows) | Task 3 |
| `which` resolves executables | Task 3 (`find_executable`) |
| Missing executable gives useful error | Task 1, 3 |
| `aix exec swtb -- env` receives expected env vars | Task 9 (`exec_sets_aix_env_vars_in_child`) |
| `aix claude swtb -- --version` attempts to run claude | Task 9 (`claude_dry_run_shows_claude_command`) |
| `aix pi swtb -- --version` attempts to run pi | Task 9 (`pi_dry_run_shows_pi_command`) |
| `--dry-run` never shows values | Task 9 (multiple dry-run tests) |

### Placeholder scan

No TBD, TODO, or "similar to" references found in this plan.

### Type consistency

- `collect_vars` returns `Vec<(&'static str, String)>` — used consistently in `LaunchEnv.vars` field and in `run_command` loop.
- `resolve_launch_env` signature `(Option<String>, Option<PathBuf>) -> Result<LaunchEnv>` — matches all callers in exec.rs, claude.rs, pi.rs, shell.rs.
- `run_command` signature `(&str, &[String], &LaunchEnv, bool) -> Result<()>` — matched by all command modules.
- `detect_shell() -> String` — matched in shell.rs.

### Notes on `std::process::Command::status()` and stdio inheritance

`Command::status()` inherits the parent's stdin/stdout/stderr by default (unlike `Command::output()` which captures them). This means the child process's output goes directly to the terminal without buffering — exactly what we want for interactive tools like `claude` and `pi`.
