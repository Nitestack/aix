# aix env Command Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `aix env [profile] --format <sh|json|nu|fish|powershell>` so users can load AI Hub credentials into any shell.

**Architecture:** Env vars are collected into an ordered `Vec<(&'static str, String)>` from resolved config secrets, then a per-format renderer serializes them to stdout. The `[compat]` section in config controls which extra env var sets (ANTHROPIC_*, OPENAI_*) are emitted.

**Tech Stack:** Rust, clap (derive), serde_json (JSON output), existing `config.rs`/`secrets.rs` infrastructure, assert_cmd + assert_fs for integration tests.

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Modify | `tools/aix/src/config.rs` | Add `Compat` struct + field on `Config` |
| Modify | `tools/aix/src/error.rs` | Add `ProfileNotFound` and `NoProfile` error variants |
| Modify | `tools/aix/src/cli.rs` | Add positional `profile` arg to `Env` variant |
| Modify | `tools/aix/src/main.rs` | Thread profile + config into `env::run` |
| Modify | `tools/aix/src/commands/env.rs` | Full implementation: collect, format, run |
| Create | `tools/aix/tests/cli_env.rs` | Integration tests for all formats + error paths |

---

## Task 1: Add `Compat` config section

**Files:**
- Modify: `tools/aix/src/config.rs`

- [ ] **Step 1.1: Write failing tests for Compat parsing**

Add at the bottom of the `#[cfg(test)]` block in `config.rs`:

```rust
#[test]
fn compat_defaults_to_anthropic_true_openai_false() {
    // Config without [compat] section must produce the right defaults.
    let cfg: Config = toml::from_str(TOML).unwrap();
    assert!(cfg.compat.anthropic_env);
    assert!(!cfg.compat.openai_env);
}

#[test]
fn compat_can_be_overridden() {
    let toml_with_compat = r#"
[compat]
anthropic_env = false
openai_env = true

[endpoint]
base_url = { env = "X" }
api_format = "anthropic"

[profiles.work]
api_key = "sk-test"
"#;
    let cfg: Config = toml::from_str(toml_with_compat).unwrap();
    assert!(!cfg.compat.anthropic_env);
    assert!(cfg.compat.openai_env);
}

#[test]
fn compat_rejects_unknown_fields() {
    let bad = r#"
[compat]
unknown_compat_key = true

[endpoint]
base_url = { env = "X" }
api_format = "anthropic"

[profiles.work]
api_key = "sk-test"
"#;
    assert!(toml::from_str::<Config>(bad).is_err());
}
```

- [ ] **Step 1.2: Run tests to confirm they fail**

```bash
cd tools/aix && cargo test compat 2>&1 | head -30
```

Expected: compilation error — `Compat` does not exist yet.

- [ ] **Step 1.3: Add `Compat` struct and `default_true` helper to `config.rs`**

Insert after the `use` block (before the `Config` struct):

```rust
fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compat {
    #[serde(default = "default_true")]
    pub anthropic_env: bool,
    #[serde(default)]
    pub openai_env: bool,
}

impl Default for Compat {
    fn default() -> Self {
        Self {
            anthropic_env: true,
            openai_env: false,
        }
    }
}
```

- [ ] **Step 1.4: Add `compat` field to `Config`**

In the `Config` struct, add after `profiles`:

```rust
#[serde(default)]
pub compat: Compat,
```

The final `Config` struct:

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub default_profile: Option<String>,
    #[serde(default)]
    pub env_files: Vec<PathBuf>,
    pub endpoint: Endpoint,
    pub profiles: HashMap<String, Profile>,
    #[serde(default)]
    pub compat: Compat,
}
```

- [ ] **Step 1.5: Run tests to confirm they pass**

```bash
cd tools/aix && cargo test compat
```

Expected: all 3 compat tests pass. All pre-existing tests still pass.

```bash
cd tools/aix && cargo test
```

Expected: all tests pass.

- [ ] **Step 1.6: Commit**

```bash
cd tools/aix
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add src/config.rs
git commit -m "feat(config): add [compat] section with anthropic_env/openai_env flags"
```

---

## Task 2: Add error variants for missing profile

**Files:**
- Modify: `tools/aix/src/error.rs`

- [ ] **Step 2.1: Write a test confirming the error messages**

Add at the end of `error.rs` (or in a new `#[cfg(test)]` block):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_not_found_message_includes_name() {
        let e = AihubError::ProfileNotFound("swtb".to_string());
        assert!(e.to_string().contains("swtb"));
    }

    #[test]
    fn no_profile_message_is_clear() {
        let e = AihubError::NoProfile;
        let msg = e.to_string();
        assert!(msg.contains("profile") || msg.contains("default"));
    }
}
```

- [ ] **Step 2.2: Run tests to confirm they fail**

```bash
cd tools/aix && cargo test -p aix -- error 2>&1 | head -20
```

Expected: compile error — variants not defined.

- [ ] **Step 2.3: Add the two new variants to `error.rs`**

```rust
#[error("profile \"{0}\" is not defined in profiles")]
ProfileNotFound(String),

#[error("no profile specified and no default_profile set in config")]
NoProfile,
```

The full `AihubError` after the addition:

```rust
#[derive(Debug, thiserror::Error)]
pub enum AihubError {
    #[error("not yet implemented: {0}")]
    NotImplemented(&'static str),

    #[error("unknown config format: {ext}")]
    UnknownFormat { ext: String },

    #[error("failed to parse {path}: {source}")]
    ParseError {
        path: std::path::PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("default_profile \"{0}\" is not defined in profiles")]
    UnknownDefaultProfile(String),

    #[allow(dead_code)]
    #[error("ambiguous secret source for {field}: specify exactly one of env / file / command")]
    AmbiguousSecretSource { field: String },

    #[error("no config file found")]
    NoConfigFile,

    #[error("profile \"{0}\" is not defined in profiles")]
    ProfileNotFound(String),

    #[error("no profile specified and no default_profile set in config")]
    NoProfile,

    #[allow(dead_code)]
    #[error("env var {name:?} is not set")]
    SecretMissingEnvVar { name: String },

    #[allow(dead_code)]
    #[error("failed to read secret file {path}: {source}")]
    SecretFileRead {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[allow(dead_code)]
    #[error("secret command {cmd:?} failed (exit code: {exit_code:?})")]
    SecretCommandFailed { cmd: String, exit_code: Option<i32> },

    #[allow(dead_code)]
    #[error("failed to spawn secret command {cmd:?}: {source}")]
    SecretCommandSpawn {
        cmd: String,
        #[source]
        source: std::io::Error,
    },

    #[allow(dead_code)]
    #[error("secret command {cmd:?} produced non-UTF-8 output")]
    SecretCommandEncoding { cmd: String },

    #[allow(dead_code)]
    #[error("failed to load env file {path}: {source}")]
    EnvFileLoad {
        path: std::path::PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}
```

- [ ] **Step 2.4: Run tests to confirm they pass**

```bash
cd tools/aix && cargo test
```

Expected: all tests pass including the two new error message tests.

- [ ] **Step 2.5: Commit**

```bash
cd tools/aix
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add src/error.rs
git commit -m "feat(error): add ProfileNotFound and NoProfile variants"
```

---

## Task 3: Add positional profile arg to CLI and update dispatch

**Files:**
- Modify: `tools/aix/src/cli.rs`
- Modify: `tools/aix/src/main.rs`

- [ ] **Step 3.1: Update the `Env` variant in `cli.rs`**

Replace:

```rust
/// Print environment variables for the selected profile
Env {
    /// Output format
    #[arg(long, value_enum, default_value = "sh")]
    format: EnvFormat,
},
```

With:

```rust
/// Print environment variables for the selected profile
Env {
    /// Profile name (positional; overrides the global --profile flag)
    profile: Option<String>,
    /// Output format
    #[arg(long, value_enum, default_value = "sh")]
    format: EnvFormat,
},
```

The positional `profile: Option<String>` field has no `#[arg(long)]` or `#[arg(short)]`, so clap treats it as an optional positional argument.

- [ ] **Step 3.2: Update `main.rs` to pass profile and config to `env::run`**

Replace the existing `run` function with:

```rust
fn run(cli: Cli) -> color_eyre::Result<()> {
    let global_profile = cli.profile;
    let config_path = cli.config;
    match cli.command {
        Command::Profiles => commands::profiles::run(),
        Command::Env { profile, format } => {
            let effective_profile = profile.or(global_profile);
            commands::env::run(effective_profile, config_path, format)
        }
        Command::Shell => commands::shell::run(),
        Command::Exec { args } => commands::exec::run(args),
        Command::Claude { args } => commands::claude::run(args),
        Command::Pi { args } => commands::pi::run(args),
        Command::Config { action } => commands::config::run(action, config_path),
    }
}
```

- [ ] **Step 3.3: Update `env::run` signature to match the new call site**

Replace the entire `tools/aix/src/commands/env.rs` with a stub that compiles:

```rust
use crate::cli::EnvFormat;
use crate::error::AihubError;
use color_eyre::Result;
use std::path::PathBuf;

pub fn run(
    _profile: Option<String>,
    _config_path: Option<PathBuf>,
    _format: EnvFormat,
) -> Result<()> {
    Err(AihubError::NotImplemented("env").into())
}
```

- [ ] **Step 3.4: Verify compilation and existing tests still pass**

```bash
cd tools/aix && cargo test
```

Expected: all tests pass. `env_default_exits_nonzero` still passes because `NotImplemented` is still an error.

- [ ] **Step 3.5: Commit**

```bash
cd tools/aix
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add src/cli.rs src/main.rs src/commands/env.rs
git commit -m "feat(cli): add positional profile arg to aix env subcommand"
```

---

## Task 4: Implement `collect_vars` and shell formatters in `env.rs`

**Files:**
- Modify: `tools/aix/src/commands/env.rs`

This task replaces the stub with all pure functions (no I/O). `run` stays as a stub.

- [ ] **Step 4.1: Write all unit tests (they will fail to compile until step 4.2)**

Replace `tools/aix/src/commands/env.rs` with:

```rust
use crate::cli::EnvFormat;
use crate::config;
use crate::error::AihubError;
use color_eyre::Result;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub fn run(
    _profile: Option<String>,
    _config_path: Option<PathBuf>,
    _format: EnvFormat,
) -> Result<()> {
    Err(AihubError::NotImplemented("env").into())
}

fn collect_vars(
    profile_name: &str,
    api_key: &str,
    base_url: &str,
    compat: &config::Compat,
) -> Vec<(&'static str, String)> {
    todo!()
}

fn format_vars(vars: &[(&'static str, String)], format: &EnvFormat) -> String {
    todo!()
}

fn sh_escape(s: &str) -> String {
    todo!()
}

fn format_sh(vars: &[(&'static str, String)]) -> String {
    todo!()
}

fn nu_escape(s: &str) -> String {
    todo!()
}

fn format_nu(vars: &[(&'static str, String)]) -> String {
    todo!()
}

fn fish_escape(s: &str) -> String {
    todo!()
}

fn format_fish(vars: &[(&'static str, String)]) -> String {
    todo!()
}

fn ps_escape(s: &str) -> String {
    todo!()
}

fn format_powershell(vars: &[(&'static str, String)]) -> String {
    todo!()
}

fn format_json(vars: &[(&'static str, String)]) -> String {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Compat;

    fn default_compat() -> Compat {
        Compat::default() // anthropic_env: true, openai_env: false
    }

    fn full_compat() -> Compat {
        Compat {
            anthropic_env: true,
            openai_env: true,
        }
    }

    fn minimal_compat() -> Compat {
        Compat {
            anthropic_env: false,
            openai_env: false,
        }
    }

    // --- collect_vars ---

    #[test]
    fn collect_vars_default_compat_produces_5_vars() {
        let vars = collect_vars("swtb", "sk-key", "https://example.com", &default_compat());
        assert_eq!(vars.len(), 5);
        assert_eq!(vars[0], ("AIX_PROFILE", "swtb".to_string()));
        assert_eq!(vars[1], ("AIX_API_KEY", "sk-key".to_string()));
        assert_eq!(vars[2], ("AIX_BASE_URL", "https://example.com".to_string()));
        assert_eq!(vars[3], ("ANTHROPIC_API_KEY", "sk-key".to_string()));
        assert_eq!(vars[4], ("ANTHROPIC_BASE_URL", "https://example.com".to_string()));
    }

    #[test]
    fn collect_vars_full_compat_produces_7_vars() {
        let vars = collect_vars("swtb", "sk-key", "https://example.com", &full_compat());
        assert_eq!(vars.len(), 7);
        assert_eq!(vars[5], ("OPENAI_API_KEY", "sk-key".to_string()));
        assert_eq!(vars[6], ("OPENAI_BASE_URL", "https://example.com".to_string()));
    }

    #[test]
    fn collect_vars_minimal_compat_produces_3_vars() {
        let vars = collect_vars("p", "k", "u", &minimal_compat());
        assert_eq!(vars.len(), 3);
        let names: Vec<&str> = vars.iter().map(|(k, _)| *k).collect();
        assert!(!names.contains(&"ANTHROPIC_API_KEY"));
        assert!(!names.contains(&"OPENAI_API_KEY"));
    }

    // --- sh ---

    #[test]
    fn sh_escape_plain_value() {
        assert_eq!(sh_escape("hello"), "'hello'");
    }

    #[test]
    fn sh_escape_single_quote_in_value() {
        // The shell trick: close ', insert \', reopen '
        assert_eq!(sh_escape("it's"), r"'it'\''s'");
    }

    #[test]
    fn sh_escape_url_with_query_string() {
        assert_eq!(
            sh_escape("https://x.com/v1?a=b&c=d"),
            "'https://x.com/v1?a=b&c=d'"
        );
    }

    #[test]
    fn format_sh_emits_export_lines() {
        let vars = vec![
            ("FOO", "bar".to_string()),
            ("BAZ", "qux".to_string()),
        ];
        assert_eq!(format_sh(&vars), "export FOO='bar'\nexport BAZ='qux'\n");
    }

    #[test]
    fn format_sh_escapes_special_chars() {
        let vars = vec![("KEY", "it's a 'test'".to_string())];
        let out = format_sh(&vars);
        // must be eval-safe: no unescaped single quotes
        assert_eq!(out, "export KEY='it'\\''s a '\\''test'\\'''\n");
    }

    // --- json ---

    #[test]
    fn format_json_produces_valid_json_object() {
        let vars = vec![("FOO", "bar".to_string()), ("BAZ", "42".to_string())];
        let out = format_json(&vars);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["FOO"], "bar");
        assert_eq!(parsed["BAZ"], "42");
    }

    #[test]
    fn format_json_handles_double_quotes_in_values() {
        let vars = vec![("KEY", r#"say "hi""#.to_string())];
        let out = format_json(&vars);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["KEY"], r#"say "hi""#);
    }

    #[test]
    fn format_json_handles_backslash_in_values() {
        let vars = vec![("KEY", r"a\b".to_string())];
        let out = format_json(&vars);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["KEY"].as_str().unwrap(), r"a\b");
    }

    // --- nu ---

    #[test]
    fn nu_escape_plain_value() {
        assert_eq!(nu_escape("hello"), "\"hello\"");
    }

    #[test]
    fn nu_escape_double_quote_in_value() {
        assert_eq!(nu_escape(r#"say "hi""#), r#""say \"hi\"""#);
    }

    #[test]
    fn nu_escape_backslash_in_value() {
        assert_eq!(nu_escape(r"a\b"), r#""a\\b""#);
    }

    #[test]
    fn format_nu_emits_env_assignment() {
        let vars = vec![("FOO", "bar".to_string())];
        assert_eq!(format_nu(&vars), "$env.FOO = \"bar\"\n");
    }

    // --- fish ---

    #[test]
    fn fish_escape_plain_value() {
        assert_eq!(fish_escape("hello"), "'hello'");
    }

    #[test]
    fn fish_escape_single_quote_in_value() {
        assert_eq!(fish_escape("it's"), r"'it\'s'");
    }

    #[test]
    fn fish_escape_backslash_in_value() {
        assert_eq!(fish_escape(r"a\b"), r"'a\\b'");
    }

    #[test]
    fn format_fish_emits_set_x() {
        let vars = vec![("FOO", "bar".to_string())];
        assert_eq!(format_fish(&vars), "set -x FOO 'bar'\n");
    }

    // --- powershell ---

    #[test]
    fn ps_escape_plain_value() {
        assert_eq!(ps_escape("hello"), "'hello'");
    }

    #[test]
    fn ps_escape_single_quote_in_value() {
        // PowerShell: single quote inside single-quoted string → ''
        assert_eq!(ps_escape("it's"), "'it''s'");
    }

    #[test]
    fn format_powershell_emits_env_colon_assignment() {
        let vars = vec![("FOO", "bar".to_string())];
        assert_eq!(format_powershell(&vars), "$env:FOO = 'bar'\n");
    }
}
```

- [ ] **Step 4.2: Run tests to verify they compile but fail at runtime**

```bash
cd tools/aix && cargo test -- env::tests 2>&1 | tail -20
```

Expected: tests panic at `todo!()`, not compilation errors.

- [ ] **Step 4.3: Implement all pure functions**

Replace the `todo!()` bodies one by one:

**`collect_vars`:**
```rust
fn collect_vars(
    profile_name: &str,
    api_key: &str,
    base_url: &str,
    compat: &config::Compat,
) -> Vec<(&'static str, String)> {
    let mut vars = vec![
        ("AIX_PROFILE", profile_name.to_string()),
        ("AIX_API_KEY", api_key.to_string()),
        ("AIX_BASE_URL", base_url.to_string()),
    ];
    if compat.anthropic_env {
        vars.push(("ANTHROPIC_API_KEY", api_key.to_string()));
        vars.push(("ANTHROPIC_BASE_URL", base_url.to_string()));
    }
    if compat.openai_env {
        vars.push(("OPENAI_API_KEY", api_key.to_string()));
        vars.push(("OPENAI_BASE_URL", base_url.to_string()));
    }
    vars
}
```

**`format_vars`:**
```rust
fn format_vars(vars: &[(&'static str, String)], format: &EnvFormat) -> String {
    match format {
        EnvFormat::Sh => format_sh(vars),
        EnvFormat::Json => format_json(vars),
        EnvFormat::Nu => format_nu(vars),
        EnvFormat::Fish => format_fish(vars),
        EnvFormat::Powershell => format_powershell(vars),
    }
}
```

**`sh_escape` and `format_sh`:**
```rust
fn sh_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn format_sh(vars: &[(&'static str, String)]) -> String {
    let mut out = vars
        .iter()
        .map(|(k, v)| format!("export {}={}", k, sh_escape(v)))
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}
```

**`nu_escape` and `format_nu`:**
```rust
fn nu_escape(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}

fn format_nu(vars: &[(&'static str, String)]) -> String {
    let mut out = vars
        .iter()
        .map(|(k, v)| format!("$env.{} = {}", k, nu_escape(v)))
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}
```

**`fish_escape` and `format_fish`:**
```rust
fn fish_escape(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn format_fish(vars: &[(&'static str, String)]) -> String {
    let mut out = vars
        .iter()
        .map(|(k, v)| format!("set -x {} {}", k, fish_escape(v)))
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}
```

**`ps_escape` and `format_powershell`:**
```rust
fn ps_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn format_powershell(vars: &[(&'static str, String)]) -> String {
    let mut out = vars
        .iter()
        .map(|(k, v)| format!("$env:{} = {}", k, ps_escape(v)))
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}
```

**`format_json`:**
```rust
fn format_json(vars: &[(&'static str, String)]) -> String {
    let map: BTreeMap<&str, &str> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let mut out = serde_json::to_string_pretty(&map).expect("BTreeMap<&str,&str> is always serializable");
    out.push('\n');
    out
}
```

- [ ] **Step 4.4: Run unit tests to verify all pass**

```bash
cd tools/aix && cargo test -- env::tests
```

Expected: all tests pass.

- [ ] **Step 4.5: Run full test suite**

```bash
cd tools/aix && cargo test
```

Expected: all tests pass.

- [ ] **Step 4.6: Commit**

```bash
cd tools/aix
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add src/commands/env.rs
git commit -m "feat(env): implement collect_vars and all shell formatters"
```

---

## Task 5: Implement `env::run`

**Files:**
- Modify: `tools/aix/src/commands/env.rs`

- [ ] **Step 5.1: Replace the stub `run` with the real implementation**

Replace only the `run` function body (keep all other functions unchanged):

```rust
pub fn run(
    positional_profile: Option<String>,
    config_path: Option<PathBuf>,
    format: EnvFormat,
) -> Result<()> {
    let path = config::find_config_path(config_path.as_deref())?
        .ok_or(AihubError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;
    config::load_env_files(&cfg)?;

    let profile_name = positional_profile
        .or_else(|| cfg.default_profile.clone())
        .ok_or(AihubError::NoProfile)?;

    let profile = cfg
        .profiles
        .get(&profile_name)
        .ok_or_else(|| AihubError::ProfileNotFound(profile_name.clone()))?;

    let api_key = profile.api_key.resolve()?;
    let base_url = cfg.endpoint.base_url.resolve()?;

    let vars = collect_vars(
        &profile_name,
        api_key.expose_secret(),
        base_url.expose_secret(),
        &cfg.compat,
    );

    print!("{}", format_vars(&vars, &format));
    Ok(())
}
```

Also update the imports at the top of the file — the final import block should be:

```rust
use crate::cli::EnvFormat;
use crate::config;
use crate::error::AihubError;
use color_eyre::Result;
use std::collections::BTreeMap;
use std::path::PathBuf;
```

- [ ] **Step 5.2: Verify tests still pass**

```bash
cd tools/aix && cargo test
```

Expected: all tests pass. `env_default_exits_nonzero` still passes because no config file is found.

- [ ] **Step 5.3: Commit**

```bash
cd tools/aix
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add src/commands/env.rs
git commit -m "feat(env): implement env::run — profile selection, secret resolution, output"
```

---

## Task 6: Integration tests

**Files:**
- Create: `tools/aix/tests/cli_env.rs`

- [ ] **Step 6.1: Create the integration test file**

```rust
// tools/aix/tests/cli_env.rs

use assert_cmd::Command;
use assert_fs::prelude::*;

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

// Config with direct (non-env) secret values so tests don't depend on env vars.
const CONFIG_DIRECT: &str = r#"
[endpoint]
base_url = "https://ai.example.com/v1"
api_format = "anthropic"

[profiles.swtb]
api_key = "sk-swtb-key"

[profiles.work]
api_key = "sk-work-key"
"#;

const CONFIG_WITH_DEFAULT: &str = r#"
default_profile = "swtb"

[endpoint]
base_url = "https://ai.example.com/v1"
api_format = "anthropic"

[profiles.swtb]
api_key = "sk-swtb-key"
"#;

const CONFIG_OPENAI_COMPAT: &str = r#"
[compat]
anthropic_env = true
openai_env = true

[endpoint]
base_url = "https://ai.example.com/v1"
api_format = "anthropic"

[profiles.swtb]
api_key = "sk-swtb-key"
"#;

const CONFIG_NO_ANTHROPIC: &str = r#"
[compat]
anthropic_env = false
openai_env = false

[endpoint]
base_url = "https://ai.example.com/v1"
api_format = "anthropic"

[profiles.swtb]
api_key = "sk-swtb-key"
"#;

// Value with special chars that need escaping in every shell format.
const CONFIG_SPECIAL_CHARS: &str = r#"
[endpoint]
base_url = "https://ai.example.com/v1?token=abc&id=1"
api_format = "anthropic"

[profiles.test]
api_key = "sk-it's a test"
"#;

// --- sh format ---

#[test]
fn env_sh_contains_aix_vars() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "sh"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    assert!(s.contains("export AIX_PROFILE='swtb'"), "got: {s}");
    assert!(s.contains("export AIX_API_KEY='sk-swtb-key'"), "got: {s}");
    assert!(s.contains("export AIX_BASE_URL='https://ai.example.com/v1'"), "got: {s}");
    // anthropic_env defaults to true
    assert!(s.contains("export ANTHROPIC_API_KEY='sk-swtb-key'"), "got: {s}");
    assert!(s.contains("export ANTHROPIC_BASE_URL='https://ai.example.com/v1'"), "got: {s}");
    // openai_env defaults to false
    assert!(!s.contains("OPENAI_"), "got: {s}");
}

#[test]
fn env_sh_default_format_is_sh() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb"])  // no --format flag → defaults to sh
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("export AIX_PROFILE='swtb'"), "got: {s}");
}

#[test]
fn env_sh_special_chars_in_key_and_url() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_SPECIAL_CHARS).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "test", "--format", "sh"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    // Single quote in API key must be escaped as '\''
    assert!(s.contains(r"'sk-it'\''s a test'"), "got: {s}");
    // URL with & and = must be preserved inside single quotes (no escaping needed for those)
    assert!(s.contains("'https://ai.example.com/v1?token=abc&id=1'"), "got: {s}");
}

// --- json format ---

#[test]
fn env_json_produces_valid_json() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value = serde_json::from_slice(&out).expect("output must be valid JSON");
    assert_eq!(parsed["AIX_PROFILE"], "swtb");
    assert_eq!(parsed["AIX_API_KEY"], "sk-swtb-key");
    assert_eq!(parsed["AIX_BASE_URL"], "https://ai.example.com/v1");
    assert_eq!(parsed["ANTHROPIC_API_KEY"], "sk-swtb-key");
    assert!(parsed.get("OPENAI_API_KEY").is_none());
}

#[test]
fn env_json_special_chars_round_trip() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_SPECIAL_CHARS).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "test", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value = serde_json::from_slice(&out).expect("output must be valid JSON");
    assert_eq!(parsed["AIX_API_KEY"], "sk-it's a test");
    assert_eq!(parsed["AIX_BASE_URL"], "https://ai.example.com/v1?token=abc&id=1");
}

// --- nu format ---

#[test]
fn env_nu_contains_env_assignments() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "nu"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    assert!(s.contains("$env.AIX_PROFILE = \"swtb\""), "got: {s}");
    assert!(s.contains("$env.AIX_API_KEY = \"sk-swtb-key\""), "got: {s}");
}

// --- fish format ---

#[test]
fn env_fish_contains_set_x_lines() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "fish"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    assert!(s.contains("set -x AIX_PROFILE 'swtb'"), "got: {s}");
    assert!(s.contains("set -x AIX_API_KEY 'sk-swtb-key'"), "got: {s}");
}

#[test]
fn env_fish_special_chars_escaped() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_SPECIAL_CHARS).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "test", "--format", "fish"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    // fish: single quote escaped as \'
    assert!(s.contains(r"'sk-it\'s a test'"), "got: {s}");
}

// --- powershell format ---

#[test]
fn env_powershell_contains_env_colon_assignments() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "powershell"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    assert!(s.contains("$env:AIX_PROFILE = 'swtb'"), "got: {s}");
    assert!(s.contains("$env:AIX_API_KEY = 'sk-swtb-key'"), "got: {s}");
}

#[test]
fn env_powershell_single_quote_doubled() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_SPECIAL_CHARS).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "test", "--format", "powershell"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    // PowerShell: single quote doubled → ''
    assert!(s.contains("'sk-it''s a test'"), "got: {s}");
}

// --- compat flags ---

#[test]
fn env_openai_compat_enabled_includes_openai_vars() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_OPENAI_COMPAT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(parsed["OPENAI_API_KEY"], "sk-swtb-key");
    assert_eq!(parsed["OPENAI_BASE_URL"], "https://ai.example.com/v1");
}

#[test]
fn env_no_anthropic_compat_omits_anthropic_vars() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_NO_ANTHROPIC).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert!(parsed.get("ANTHROPIC_API_KEY").is_none());
    assert!(parsed.get("OPENAI_API_KEY").is_none());
    // Core vars still present
    assert_eq!(parsed["AIX_API_KEY"], "sk-swtb-key");
}

// --- profile selection ---

#[test]
fn env_uses_default_profile_when_none_specified() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_WITH_DEFAULT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "--format", "json"])  // no positional profile
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(parsed["AIX_PROFILE"], "swtb");
}

#[test]
fn env_errors_when_no_profile_and_no_default() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap(); // no default_profile

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "--format", "json"])  // no positional profile
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("profile") || s.contains("default"), "got: {s}");
}

#[test]
fn env_errors_when_named_profile_not_in_config() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "nonexistent", "--format", "json"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("nonexistent"), "got: {s}");
}

#[test]
fn env_no_config_file_exits_nonzero() {
    let home = assert_fs::TempDir::new().unwrap();
    cmd()
        .env("HOME", home.path())
        .env_remove("AIX_CONFIG")
        .env_remove("XDG_CONFIG_HOME")
        .args(["env", "swtb", "--format", "json"])
        .assert()
        .failure();
}

// --- no debug/config data leaks ---

#[test]
fn env_output_does_not_contain_unrelated_config_fields() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "sh"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    // Should not contain config file path, internal field names, etc.
    assert!(!s.contains("api_format"), "got: {s}");
    assert!(!s.contains("aix.toml"), "got: {s}");
}
```

- [ ] **Step 6.2: Run integration tests**

```bash
cd tools/aix && cargo test --test cli_env
```

Expected: all integration tests pass.

- [ ] **Step 6.3: Run full test suite**

```bash
cd tools/aix && cargo test
```

Expected: all tests (unit + integration) pass.

- [ ] **Step 6.4: Commit**

```bash
cd tools/aix
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add tests/cli_env.rs
git commit -m "test(env): integration tests for all output formats and error paths"
```

---

## Task 7: Final lint and verification

- [ ] **Step 7.1: Run clippy with warnings-as-errors**

```bash
cd tools/aix && cargo clippy --all-targets -- -D warnings
```

Expected: zero warnings.

- [ ] **Step 7.2: Run formatter check**

```bash
cd tools/aix && cargo fmt --all -- --check
```

Expected: exit 0 (no formatting drift).

- [ ] **Step 7.3: Run full test suite one last time**

```bash
cd tools/aix && cargo test
```

Expected: all tests pass.

---

## Self-Review: Spec Coverage Check

| Requirement | Covered by |
|-------------|------------|
| `AIX_PROFILE`, `AIX_API_KEY`, `AIX_BASE_URL` | Task 4 `collect_vars` + Task 6 |
| `ANTHROPIC_*` when `compat.anthropic_env = true` (default) | Task 4 + Task 6 compat tests |
| `OPENAI_*` when `compat.openai_env = true` | Task 4 + Task 6 `env_openai_compat_enabled_includes_openai_vars` |
| `[compat]` config flags with correct defaults | Task 1 |
| sh format + POSIX eval-safe | Task 4 `sh_escape` + special-char tests |
| json format via serde_json | Task 4 `format_json` + Task 6 round-trip |
| nu format | Task 4 `format_nu` + Task 6 |
| fish format | Task 4 `format_fish` + Task 6 |
| powershell format | Task 4 `format_powershell` + Task 6 |
| Special chars in keys/URLs | Task 4 unit tests + Task 6 `CONFIG_SPECIAL_CHARS` tests |
| Profile omitted → use default | Task 5 `run` + Task 6 `env_uses_default_profile_when_none_specified` |
| No profile + no default → clear error | Task 2 `NoProfile` + Task 6 `env_errors_when_no_profile_and_no_default` |
| Named profile not in config → clear error | Task 2 `ProfileNotFound` + Task 6 `env_errors_when_named_profile_not_in_config` |
| No output format leaks unrelated config | Task 6 `env_output_does_not_contain_unrelated_config_fields` |
| No interactive terminal required when profile supplied | Task 5 `run` (no inquire call) + Task 6 integration tests all work without TTY |
