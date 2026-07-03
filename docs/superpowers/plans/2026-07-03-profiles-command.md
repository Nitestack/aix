# Profiles Command Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `aix profiles` / `aix profiles --json`, replace `gum choose` with `inquire::Select` in `aix env`, add duplicate-label validation, and add tests for all paths.

**Architecture:** `config::sorted_profiles()` returns a sorted `(name, display_label)` list used by both the `profiles` command and the interactive picker in `env`. Profile resolution in `env::run` becomes an explicit three-step chain: positional → default_profile → interactive selector (TTY) or error (non-TTY). Duplicate-label validation is added to `config::validate()` so it fires for all commands.

**Tech Stack:** Rust, clap (derive), inquire (already in Cargo.toml), serde_json, std::io::IsTerminal (stable since Rust 1.70), assert_cmd + assert_fs for integration tests.

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Modify | `tools/aix/src/error.rs` | Add `DuplicateLabel`, `SelectionCancelled`, `NoInteractiveTerminal`; annotate `NoProfile` |
| Modify | `tools/aix/src/config.rs` | Add `sorted_profiles()`; extend `validate()` with duplicate-label check |
| Modify | `tools/aix/src/cli.rs` | Change `Profiles` unit variant to `Profiles { json: bool }` |
| Modify | `tools/aix/src/main.rs` | Thread `config_path` into `profiles::run`; match new `Profiles { json }` |
| Modify | `tools/aix/src/commands/profiles.rs` | Full implementation replacing the stub |
| Modify | `tools/aix/src/commands/env.rs` | Replace `.ok_or(NoProfile)` with `resolve_profile()` + `select_profile_interactively()` |
| Create | `tools/aix/tests/cli_profiles.rs` | Integration tests: text, JSON, duplicate, env non-interactive |

---

## Task 1: Add error variants

**Files:**
- Modify: `tools/aix/src/error.rs`

- [ ] **Step 1.1: Write failing unit tests for new variant messages**

Add to the `#[cfg(test)]` block at the bottom of `error.rs`:

```rust
#[test]
fn duplicate_label_message_includes_label_and_profiles() {
    let e = AihubError::DuplicateLabel {
        label: "Work".to_string(),
        first: "alpha".to_string(),
        second: "beta".to_string(),
    };
    let msg = e.to_string();
    assert!(msg.contains("Work"), "got: {msg}");
    assert!(msg.contains("alpha"), "got: {msg}");
    assert!(msg.contains("beta"), "got: {msg}");
}

#[test]
fn selection_cancelled_message_is_clear() {
    let e = AihubError::SelectionCancelled;
    let msg = e.to_string();
    assert!(msg.contains("cancel") || msg.contains("select"), "got: {msg}");
}

#[test]
fn no_interactive_terminal_message_contains_profile() {
    let e = AihubError::NoInteractiveTerminal;
    let msg = e.to_string();
    assert!(msg.contains("profile"), "got: {msg}");
}
```

- [ ] **Step 1.2: Run tests to confirm they fail**

```sh
cd tools/aix && cargo test --lib error 2>&1 | tail -20
```

Expected: FAIL — `DuplicateLabel`, `SelectionCancelled`, `NoInteractiveTerminal` not defined.

- [ ] **Step 1.3: Add the new variants and annotate `NoProfile`**

In `error.rs`, add the three new variants and add `#[allow(dead_code)]` to `NoProfile` (it will become unused in Task 5). The full updated enum body should look like this — replace the existing variants block:

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

    #[error("profile \"{0}\" is not defined in profiles")]
    ProfileNotFound(String),

    #[allow(dead_code)]
    #[error("no profile specified and no default_profile set in config")]
    NoProfile,

    #[error("duplicate profile label \"{label}\" — found in profiles \"{first}\" and \"{second}\"")]
    DuplicateLabel {
        label: String,
        first: String,
        second: String,
    },

    #[error("profile selection cancelled")]
    SelectionCancelled,

    #[error("no profile specified; pass a profile name or run in an interactive terminal")]
    NoInteractiveTerminal,
}
```

- [ ] **Step 1.4: Run tests to confirm they pass**

```sh
cd tools/aix && cargo test --lib error 2>&1 | tail -20
```

Expected: all `error` tests PASS.

- [ ] **Step 1.5: Run clippy and format**

```sh
cd tools/aix && cargo fmt --all && cargo clippy --all-targets -- -D warnings 2>&1 | tail -20
```

Expected: no warnings or errors.

- [ ] **Step 1.6: Commit**

```sh
git add tools/aix/src/error.rs
git commit -m "feat(error): add DuplicateLabel, SelectionCancelled, NoInteractiveTerminal variants"
```

---

## Task 2: `config::sorted_profiles` and duplicate-label validation

**Files:**
- Modify: `tools/aix/src/config.rs`

- [ ] **Step 2.1: Write failing unit tests**

Add to the `#[cfg(test)]` block in `config.rs` (after the existing tests):

```rust
// --- sorted_profiles ---

#[test]
fn sorted_profiles_sorted_by_name() {
    let cfg: Config = toml::from_str(
        r#"
[endpoint]
base_url = { env = "X" }
api_format = "anthropic"

[profiles.zzz]
label = "Z profile"
api_key = "sk-z"

[profiles.aaa]
label = "A profile"
api_key = "sk-a"
"#,
    )
    .unwrap();
    let pairs = sorted_profiles(&cfg);
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].0, "aaa");
    assert_eq!(pairs[1].0, "zzz");
}

#[test]
fn sorted_profiles_label_falls_back_to_name() {
    let cfg: Config = toml::from_str(
        r#"
[endpoint]
base_url = { env = "X" }
api_format = "anthropic"

[profiles.myprofile]
api_key = "sk-test"
"#,
    )
    .unwrap();
    let pairs = sorted_profiles(&cfg);
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].0, "myprofile");
    assert_eq!(pairs[0].1, "myprofile"); // no label → falls back to name
}

#[test]
fn sorted_profiles_uses_label_when_set() {
    let cfg: Config = toml::from_str(
        r#"
[endpoint]
base_url = { env = "X" }
api_format = "anthropic"

[profiles.work]
label = "Work account"
api_key = "sk-work"
"#,
    )
    .unwrap();
    let pairs = sorted_profiles(&cfg);
    assert_eq!(pairs[0].1, "Work account");
}

// --- duplicate label validation ---

#[test]
fn validate_fails_on_duplicate_labels() {
    let cfg: Config = toml::from_str(
        r#"
[endpoint]
base_url = { env = "X" }
api_format = "anthropic"

[profiles.alpha]
label = "Shared"
api_key = "sk-a"

[profiles.beta]
label = "Shared"
api_key = "sk-b"
"#,
    )
    .unwrap();
    let err = validate(&cfg).unwrap_err();
    assert!(
        matches!(err, AihubError::DuplicateLabel { ref label, ref first, ref second }
            if label == "Shared" && first == "alpha" && second == "beta"),
        "unexpected error: {err}"
    );
}

#[test]
fn validate_passes_when_labels_are_distinct() {
    let cfg: Config = toml::from_str(
        r#"
[endpoint]
base_url = { env = "X" }
api_format = "anthropic"

[profiles.a]
label = "Alpha"
api_key = "sk-a"

[profiles.b]
label = "Beta"
api_key = "sk-b"
"#,
    )
    .unwrap();
    assert!(validate(&cfg).is_ok());
}

#[test]
fn validate_passes_when_all_labels_none() {
    let cfg: Config = toml::from_str(
        r#"
[endpoint]
base_url = { env = "X" }
api_format = "anthropic"

[profiles.a]
api_key = "sk-a"

[profiles.b]
api_key = "sk-b"
"#,
    )
    .unwrap();
    assert!(validate(&cfg).is_ok());
}
```

- [ ] **Step 2.2: Run tests to confirm they fail**

```sh
cd tools/aix && cargo test --lib config 2>&1 | tail -20
```

Expected: FAIL — `sorted_profiles` not defined; duplicate label test fails.

- [ ] **Step 2.3: Add `sorted_profiles` to `config.rs`**

Add after the `validate` function:

```rust
/// Returns (name, display_label) pairs sorted alphabetically by name.
/// display_label is the configured label, or the profile name when no label is set.
pub fn sorted_profiles(cfg: &Config) -> Vec<(&str, &str)> {
    let mut pairs: Vec<(&str, &str)> = cfg
        .profiles
        .iter()
        .map(|(name, profile)| {
            let label = profile.label.as_deref().unwrap_or(name.as_str());
            (name.as_str(), label)
        })
        .collect();
    pairs.sort_by_key(|(name, _)| *name);
    pairs
}
```

- [ ] **Step 2.4: Extend `validate()` with duplicate-label check**

Replace the existing `validate` function body:

```rust
pub fn validate(config: &Config) -> Result<(), AihubError> {
    if let Some(ref name) = config.default_profile {
        if !config.profiles.contains_key(name.as_str()) {
            return Err(AihubError::UnknownDefaultProfile(name.clone()));
        }
    }

    // Collect names sorted for deterministic first/second in the error.
    let mut sorted_names: Vec<&str> = config.profiles.keys().map(String::as_str).collect();
    sorted_names.sort();
    let mut seen: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for name in sorted_names {
        if let Some(label) = config.profiles[name].label.as_deref() {
            if let Some(&first) = seen.get(label) {
                return Err(AihubError::DuplicateLabel {
                    label: label.to_string(),
                    first: first.to_string(),
                    second: name.to_string(),
                });
            }
            seen.insert(label, name);
        }
    }

    Ok(())
}
```

- [ ] **Step 2.5: Run tests to confirm they pass**

```sh
cd tools/aix && cargo test --lib config 2>&1 | tail -20
```

Expected: all `config` tests PASS.

- [ ] **Step 2.6: Run clippy and format**

```sh
cd tools/aix && cargo fmt --all && cargo clippy --all-targets -- -D warnings 2>&1 | tail -20
```

Expected: no warnings or errors.

- [ ] **Step 2.7: Commit**

```sh
git add tools/aix/src/config.rs
git commit -m "feat(config): add sorted_profiles helper and duplicate-label validation"
```

---

## Task 3: CLI plumbing — add `--json` flag and thread `config_path`

**Files:**
- Modify: `tools/aix/src/cli.rs`
- Modify: `tools/aix/src/main.rs`

- [ ] **Step 3.1: Update `Profiles` variant in `cli.rs`**

Replace:
```rust
/// List available profiles
Profiles,
```

With:
```rust
/// List available profiles
Profiles {
    /// Emit profiles as a JSON array (no secrets)
    #[arg(long)]
    json: bool,
},
```

- [ ] **Step 3.2: Update `main.rs` to thread `config_path` and match new variant**

Replace:
```rust
Command::Profiles => commands::profiles::run(),
```

With:
```rust
Command::Profiles { json } => commands::profiles::run(config_path, json),
```

- [ ] **Step 3.3: Verify it compiles (the stub signature mismatch is expected to error)**

```sh
cd tools/aix && cargo build 2>&1 | grep -E "error|warning" | head -20
```

Expected: compile error about `profiles::run` argument mismatch (the stub takes no args). This confirms the wiring is correct and Task 4 completes the circuit.

- [ ] **Step 3.4: Commit the plumbing change**

```sh
git add tools/aix/src/cli.rs tools/aix/src/main.rs
git commit -m "feat(cli): add --json flag to profiles; thread config_path to profiles::run"
```

---

## Task 4: `profiles` command implementation

**Files:**
- Modify: `tools/aix/src/commands/profiles.rs`
- Create: `tools/aix/tests/cli_profiles.rs`

- [ ] **Step 4.1: Write the integration test file**

Create `tools/aix/tests/cli_profiles.rs`:

```rust
use assert_cmd::Command;
use assert_fs::prelude::*;

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

// Two profiles with labels. "fast" sorts before "work" alphabetically.
const CONFIG: &str = r#"
[endpoint]
base_url = "https://ai.example.com/v1"
api_format = "anthropic"

[profiles.work]
label = "Work account"
api_key = "sk-work"

[profiles.fast]
label = "Fast model"
api_key = "sk-fast"
"#;

// Profiles with no labels — name is used as display label.
const CONFIG_NO_LABELS: &str = r#"
[endpoint]
base_url = "https://ai.example.com/v1"
api_format = "anthropic"

[profiles.alpha]
api_key = "sk-alpha"

[profiles.beta]
api_key = "sk-beta"
"#;

// Two profiles with the same label — should error at validate time.
const CONFIG_DUPLICATE_LABEL: &str = r#"
[endpoint]
base_url = "https://ai.example.com/v1"
api_format = "anthropic"

[profiles.work]
label = "Shared label"
api_key = "sk-work"

[profiles.work2]
label = "Shared label"
api_key = "sk-work2"
"#;

// --- profiles text output ---

#[test]
fn profiles_text_shows_labels_sorted_by_name() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .arg("profiles")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    // fast (label: "Fast model") sorts before work (label: "Work account")
    assert_eq!(s, "Fast model\nWork account\n", "got: {s}");
}

#[test]
fn profiles_text_uses_name_as_fallback_when_no_label() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_NO_LABELS).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .arg("profiles")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert_eq!(s, "alpha\nbeta\n", "got: {s}");
}

// --- profiles --json ---

#[test]
fn profiles_json_is_valid_json_array_sorted_by_name() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["profiles", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value =
        serde_json::from_slice(&out).expect("output must be valid JSON");
    let arr = parsed.as_array().expect("must be an array");
    assert_eq!(arr.len(), 2);
    // sorted by name: fast < work
    assert_eq!(arr[0]["name"], "fast");
    assert_eq!(arr[0]["label"], "Fast model");
    assert_eq!(arr[1]["name"], "work");
    assert_eq!(arr[1]["label"], "Work account");
}

#[test]
fn profiles_json_contains_no_secrets() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["profiles", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(!s.contains("sk-"), "output must not contain secrets: {s}");
    assert!(!s.contains("api_key"), "output must not contain api_key field: {s}");
}

#[test]
fn profiles_json_label_falls_back_to_name() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_NO_LABELS).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["profiles", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let arr = parsed.as_array().unwrap();
    // When no label set, label field equals the name
    assert_eq!(arr[0]["name"], "alpha");
    assert_eq!(arr[0]["label"], "alpha");
}

// --- duplicate label validation ---

#[test]
fn duplicate_label_causes_failure_for_profiles_command() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DUPLICATE_LABEL).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .arg("profiles")
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("Shared label"), "got: {s}");
}

#[test]
fn duplicate_label_causes_failure_for_env_command() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DUPLICATE_LABEL).unwrap();

    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "work"])
        .assert()
        .failure();
}

// --- env non-interactive error (no profile, no default, piped stdin) ---

#[test]
fn env_non_interactive_no_profile_errors_clearly() {
    // CONFIG has no default_profile. assert_cmd pipes stdin so is_terminal() → false.
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("AIX_PROFILE")
        .arg("env") // no positional profile
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("profile"), "error must mention 'profile': {s}");
}
```

- [ ] **Step 4.2: Run tests to confirm they fail**

```sh
cd tools/aix && cargo test --test cli_profiles 2>&1 | tail -30
```

Expected: FAIL — `profiles::run` still returns `NotImplemented`.

- [ ] **Step 4.3: Implement `profiles::run`**

Replace the entire contents of `tools/aix/src/commands/profiles.rs`:

```rust
use crate::config;
use crate::error::AihubError;
use color_eyre::Result;
use serde::Serialize;
use std::path::PathBuf;

pub fn run(config_path: Option<PathBuf>, json: bool) -> Result<()> {
    let path =
        config::find_config_path(config_path.as_deref())?.ok_or(AihubError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;

    let profiles = config::sorted_profiles(&cfg);

    if json {
        print_json(&profiles)?;
    } else {
        print_text(&profiles);
    }
    Ok(())
}

#[derive(Serialize)]
struct ProfileEntry<'a> {
    name: &'a str,
    label: &'a str,
}

fn print_json(profiles: &[(&str, &str)]) -> Result<()> {
    let entries: Vec<ProfileEntry> = profiles
        .iter()
        .map(|(name, label)| ProfileEntry { name, label })
        .collect();
    let mut out = serde_json::to_string_pretty(&entries)?;
    out.push('\n');
    print!("{out}");
    Ok(())
}

fn print_text(profiles: &[(&str, &str)]) {
    for (_, label) in profiles {
        println!("{label}");
    }
}
```

- [ ] **Step 4.4: Run tests to confirm they pass**

```sh
cd tools/aix && cargo test --test cli_profiles 2>&1 | tail -30
```

Expected: all `cli_profiles` tests PASS.

- [ ] **Step 4.5: Run the full test suite**

```sh
cd tools/aix && cargo test 2>&1 | tail -20
```

Expected: all tests PASS (existing `cli_env` and `cli_help` tests unaffected).

- [ ] **Step 4.6: Run clippy and format**

```sh
cd tools/aix && cargo fmt --all && cargo clippy --all-targets -- -D warnings 2>&1 | tail -20
```

Expected: no warnings or errors.

- [ ] **Step 4.7: Commit**

```sh
git add tools/aix/src/commands/profiles.rs tools/aix/tests/cli_profiles.rs
git commit -m "feat(profiles): implement profiles command with text and JSON output"
```

---

## Task 5: Interactive profile selection in `env`

**Files:**
- Modify: `tools/aix/src/commands/env.rs`

- [ ] **Step 5.1: Verify the non-interactive test already fails correctly**

The `env_non_interactive_no_profile_errors_clearly` test written in Task 4 exercises this path. Run it to see current behavior:

```sh
cd tools/aix && cargo test --test cli_profiles env_non_interactive 2>&1 | tail -10
```

If it passes already (because `NoProfile` message contains "profile"), proceed anyway — after this task it will return `NoInteractiveTerminal` with the same test guarantee.

- [ ] **Step 5.2: Replace profile resolution in `env::run`**

In `tools/aix/src/commands/env.rs`, replace:

```rust
let profile_name = positional_profile
    .or_else(|| cfg.default_profile.clone())
    .ok_or(AihubError::NoProfile)?;
```

With:

```rust
let profile_name = resolve_profile(positional_profile, &cfg)?;
```

- [ ] **Step 5.3: Add `resolve_profile` and `select_profile_interactively` to `env.rs`**

Add these two functions anywhere below the `run` function (before the `#[cfg(test)]` block):

```rust
fn resolve_profile(positional: Option<String>, cfg: &config::Config) -> Result<String, AihubError> {
    if let Some(p) = positional {
        return Ok(p);
    }
    if let Some(p) = cfg.default_profile.clone() {
        return Ok(p);
    }
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        select_profile_interactively(cfg)
    } else {
        Err(AihubError::NoInteractiveTerminal)
    }
}

fn select_profile_interactively(cfg: &config::Config) -> Result<String, AihubError> {
    let profiles = config::sorted_profiles(cfg);
    let options: Vec<String> = profiles.iter().map(|(_, label)| label.to_string()).collect();

    let selected = inquire::Select::new("Select a profile:", options)
        .prompt()
        .map_err(|_| AihubError::SelectionCancelled)?;

    // validate() has already rejected duplicate labels, so this find is unambiguous.
    profiles
        .into_iter()
        .find(|(_, label)| *label == selected.as_str())
        .map(|(name, _)| name.to_string())
        .ok_or_else(|| AihubError::ProfileNotFound(selected))
}
```

- [ ] **Step 5.4: Run the full test suite**

```sh
cd tools/aix && cargo test 2>&1 | tail -20
```

Expected: all tests PASS. In particular:
- `cli_profiles::env_non_interactive_no_profile_errors_clearly` — PASS (error message contains "profile")
- `cli_env::env_errors_when_no_profile_and_no_default` — PASS (same reason: "no profile specified" contains "profile")
- All other existing tests — PASS

- [ ] **Step 5.5: Run clippy and format**

```sh
cd tools/aix && cargo fmt --all && cargo clippy --all-targets -- -D warnings 2>&1 | tail -20
```

Expected: no warnings or errors.

- [ ] **Step 5.6: Commit**

```sh
git add tools/aix/src/commands/env.rs
git commit -m "feat(env): replace NoProfile with interactive selector; non-interactive → NoInteractiveTerminal"
```

---

## Self-Review

**Spec coverage check:**

| Requirement | Task |
|---|---|
| `aix profiles` lists profiles | Task 4 |
| `aix profiles --json` emits profile metadata, no secrets | Task 4 |
| `aix env <profile>` uses supplied profile | Existing — no change needed |
| No profile → use `default_profile` | Task 5 (step 2 of resolver) |
| No profile + TTY → interactive selector | Task 5 (`select_profile_interactively`) |
| No profile + non-interactive → clear error | Task 5 (`NoInteractiveTerminal`) |
| Interactive selector displays labels, not secrets | Task 5 (`options` built from `sorted_profiles`) |
| Duplicate label detection at validate time | Task 2 |
| Tests: explicit profile | `cli_env::env_sh_contains_aix_vars` (existing) |
| Tests: default profile | `cli_env::env_uses_default_profile_when_none_specified` (existing) |
| Tests: non-interactive error | `cli_profiles::env_non_interactive_no_profile_errors_clearly` |
| Tests: duplicate label | `cli_profiles::duplicate_label_causes_failure_for_profiles_command` |
| Tests: JSON profile listing | `cli_profiles::profiles_json_is_valid_json_array_sorted_by_name` |
| No `gum` binary required | Whole plan — only `inquire` used |
