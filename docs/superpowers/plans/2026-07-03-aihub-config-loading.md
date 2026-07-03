# aix Config Loading Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `config.rs` stub with a real typed config model that loads TOML, YAML, JSON, and JSON5 files, validates structure, and wires up `aix config path` and `aix config validate`.

**Architecture:** `secrets.rs` owns the `SecretSource` enum with a custom serde `Deserialize` impl (string or one-key table). `config.rs` owns `Config`/`Profile`/`Endpoint` structs plus `find_config_path`, `load`, and `validate`. The `config` crate is dropped in favour of per-format `from_str` dispatch.

**Tech Stack:** `toml 0.8`, `serde_yaml 0.9`, `json5 0.4`, `serde_json 1` (existing), `serde 1` (existing), `directories 6` (existing), `assert_fs 1` (dev, existing), `assert_cmd 2` (dev, existing), `predicates 3` (dev, existing).

---

## File map

| File | Action | Responsibility |
|------|--------|----------------|
| `tools/aix/Cargo.toml` | Modify | Swap `config` crate for `toml`, `serde_yaml`, `json5` |
| `tools/aix/src/error.rs` | Modify | Add 5 new error variants |
| `tools/aix/src/secrets.rs` | **Create** | `SecretSource` enum + custom `Deserialize` |
| `tools/aix/src/config.rs` | Replace | `Config`/`Profile`/`Endpoint` structs + `find_config_path` + `load` + `validate` |
| `tools/aix/src/main.rs` | Modify | Add `mod secrets;`; pass `cli.config` to config command |
| `tools/aix/src/commands/config.rs` | Modify | Implement `config path` and `config validate` |
| `tools/aix/tests/cli_help.rs` | Modify | Add 4 command integration tests |

---

## Task 1: Update Cargo.toml

**Files:**
- Modify: `tools/aix/Cargo.toml`

- [ ] **Step 1: Remove `config` crate, add format crates**

Replace the `config` line and add the three new parsers:

```toml
[dependencies]
clap        = { version = "4", features = ["derive", "env"] }
toml        = "0.8"
serde_yaml  = "0.9"
json5       = "0.4"
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

- [ ] **Step 2: Verify it resolves**

```sh
cd tools/aix && cargo check
```

Expected: compiles (config module still stubs, no format imports yet). Any "unused import" warnings are fine at this stage.

- [ ] **Step 3: Commit**

```bash
git add tools/aix/Cargo.toml
git commit -m "chore(aix): swap config crate for toml/serde_yaml/json5"
```

---

## Task 2: Extend error.rs and add `mod secrets` to main.rs

**Files:**
- Modify: `tools/aix/src/error.rs`
- Modify: `tools/aix/src/main.rs`

- [ ] **Step 1: Add new error variants to error.rs**

```rust
// tools/aix/src/error.rs
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

    #[error("ambiguous secret source for {field}: specify exactly one of env / file / command")]
    AmbiguousSecretSource { field: String },

    #[error("no config file found")]
    NoConfigFile,
}
```

- [ ] **Step 2: Add `mod secrets;` to main.rs**

```rust
// tools/aix/src/main.rs
use clap::Parser;
use cli::{Cli, Command};

mod cli;
mod commands;
mod config;
mod error;
mod secrets;

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
        Command::Config { action } => commands::config::run(action, cli.config),
    }
}
```

Note: `commands::config::run` now takes a second argument `cli.config`. The command module will be updated in Task 6.

- [ ] **Step 3: Verify**

```sh
cargo check 2>&1 | head -20
```

Expected: error about `commands::config::run` wrong argument count — that's correct, we fix it in Task 6.

- [ ] **Step 4: Commit**

```bash
git add tools/aix/src/error.rs tools/aix/src/main.rs
git commit -m "feat(aix): add config error variants and secrets module declaration"
```

---

## Task 3: Implement SecretSource (TDD)

**Files:**
- Create: `tools/aix/src/secrets.rs`

- [ ] **Step 1: Write the failing tests**

Create `tools/aix/src/secrets.rs` with tests first:

```rust
// tools/aix/src/secrets.rs
use serde::{Deserialize, Deserializer};
use std::path::PathBuf;

#[derive(Debug)]
pub enum SecretSource {
    Direct(String),
    Env(String),
    File(PathBuf),
    Command(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: deserialize SecretSource from a TOML string like `value = ...`
    #[derive(Deserialize)]
    struct W {
        value: SecretSource,
    }
    fn from_toml(s: &str) -> Result<SecretSource, toml::de::Error> {
        toml::from_str::<W>(s).map(|w| w.value)
    }

    #[test]
    fn test_direct() {
        let src = from_toml("value = \"sk-test\"").unwrap();
        assert!(matches!(src, SecretSource::Direct(ref s) if s == "sk-test"));
    }

    #[test]
    fn test_env() {
        let src = from_toml("value = { env = \"MY_KEY\" }").unwrap();
        assert!(matches!(src, SecretSource::Env(ref s) if s == "MY_KEY"));
    }

    #[test]
    fn test_file() {
        let src = from_toml("value = { file = \"/run/secrets/key\" }").unwrap();
        assert!(matches!(src, SecretSource::File(ref p) if p == std::path::Path::new("/run/secrets/key")));
    }

    #[test]
    fn test_command() {
        let src = from_toml("value = { command = \"op read op://Work/key\" }").unwrap();
        assert!(matches!(src, SecretSource::Command(ref s) if s == "op read op://Work/key"));
    }

    #[test]
    fn test_ambiguous_multiple_keys() {
        let result = from_toml("value = { env = \"X\", file = \"/y\" }");
        assert!(result.is_err(), "expected error for ambiguous source");
    }

    #[test]
    fn test_unknown_key_rejected() {
        let result = from_toml("value = { keyring = \"X\" }");
        assert!(result.is_err(), "expected error for unknown key");
    }

    #[test]
    fn test_empty_table_rejected() {
        let result = from_toml("value = {}");
        assert!(result.is_err(), "expected error for empty table");
    }
}
```

- [ ] **Step 2: Run to confirm all tests fail**

```sh
cargo test -p aix secrets 2>&1 | tail -20
```

Expected: compile error — `SecretSource` doesn't implement `Deserialize`.

- [ ] **Step 3: Implement the custom Deserialize**

Add the implementation below the `SecretSource` definition in `secrets.rs`:

```rust
#[derive(Deserialize)]
#[serde(untagged)]
enum SecretSourceDe {
    Direct(String),
    Structured(SecretSourceFields),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretSourceFields {
    env: Option<String>,
    file: Option<PathBuf>,
    command: Option<String>,
}

impl<'de> Deserialize<'de> for SecretSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match SecretSourceDe::deserialize(deserializer)? {
            SecretSourceDe::Direct(s) => Ok(SecretSource::Direct(s)),
            SecretSourceDe::Structured(f) => match (f.env, f.file, f.command) {
                (Some(v), None, None) => Ok(SecretSource::Env(v)),
                (None, Some(p), None) => Ok(SecretSource::File(p)),
                (None, None, Some(c)) => Ok(SecretSource::Command(c)),
                (None, None, None) => Err(serde::de::Error::custom(
                    "secret source table must specify one of: env, file, command",
                )),
                _ => Err(serde::de::Error::custom(
                    "ambiguous secret source: specify exactly one of env / file / command",
                )),
            },
        }
    }
}
```

- [ ] **Step 4: Run the tests**

```sh
cargo test -p aix secrets 2>&1
```

Expected: all 7 tests pass.

- [ ] **Step 5: Lint**

```sh
cargo clippy --all-targets -- -D warnings 2>&1 | grep -v "^warning\[" | head -20
```

Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add tools/aix/src/secrets.rs
git commit -m "feat(aix): implement SecretSource with multi-format serde deserialization"
```

---

## Task 4: Implement Config types with format coverage tests (TDD)

**Files:**
- Modify: `tools/aix/src/config.rs`

- [ ] **Step 1: Write the failing format coverage tests**

Replace `tools/aix/src/config.rs` entirely:

```rust
// tools/aix/src/config.rs
use crate::error::AihubError;
use crate::secrets::SecretSource;
use directories::ProjectDirs;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub default_profile: Option<String>,
    pub endpoint: Endpoint,
    pub profiles: HashMap<String, Profile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub base_url: SecretSource,
    pub api_format: String,
    pub provider: Option<String>,
    pub gateway: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub label: Option<String>,
    pub api_key: SecretSource,
}

pub fn find_config_path(_explicit: Option<&Path>) -> Result<Option<PathBuf>, AihubError> {
    todo!()
}

pub fn load(_path: &Path) -> Result<Config, AihubError> {
    todo!()
}

pub fn validate(_config: &Config) -> Result<(), AihubError> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Equivalent config expressed in each supported format.
    // All four should parse to the same logical structure.
    const TOML: &str = r#"
default_profile = "work"

[endpoint]
base_url = { env = "AIX_BASE_URL" }
api_format = "anthropic"
provider = "litellm"

[profiles.work]
label = "Work"
api_key = { env = "AIX_API_KEY" }

[profiles.local]
label = "Local"
api_key = "sk-local-key"
"#;

    const YAML: &str = r#"
default_profile: work
endpoint:
  base_url:
    env: AIX_BASE_URL
  api_format: anthropic
  provider: litellm
profiles:
  work:
    label: Work
    api_key:
      env: AIX_API_KEY
  local:
    label: Local
    api_key: sk-local-key
"#;

    const JSON: &str = r#"{
  "default_profile": "work",
  "endpoint": {
    "base_url": { "env": "AIX_BASE_URL" },
    "api_format": "anthropic",
    "provider": "litellm"
  },
  "profiles": {
    "work": { "label": "Work", "api_key": { "env": "AIX_API_KEY" } },
    "local": { "label": "Local", "api_key": "sk-local-key" }
  }
}"#;

    const JSON5: &str = r#"{
  default_profile: "work",
  endpoint: {
    base_url: { env: "AIX_BASE_URL" },
    api_format: "anthropic",
    provider: "litellm",
  },
  profiles: {
    work: { label: "Work", api_key: { env: "AIX_API_KEY" } },
    local: { label: "Local", api_key: "sk-local-key" },
  },
}"#;

    fn assert_standard(cfg: &Config) {
        assert_eq!(cfg.default_profile.as_deref(), Some("work"));
        assert_eq!(cfg.endpoint.api_format, "anthropic");
        assert_eq!(cfg.endpoint.provider.as_deref(), Some("litellm"));
        assert!(matches!(cfg.endpoint.base_url, SecretSource::Env(_)));
        assert!(cfg.profiles.contains_key("work"));
        assert!(cfg.profiles.contains_key("local"));
        assert!(matches!(cfg.profiles["work"].api_key, SecretSource::Env(_)));
        assert!(matches!(cfg.profiles["local"].api_key, SecretSource::Direct(_)));
    }

    #[test]
    fn parse_toml() {
        let cfg: Config = toml::from_str(TOML).unwrap();
        assert_standard(&cfg);
    }

    #[test]
    fn parse_yaml() {
        let cfg: Config = serde_yaml::from_str(YAML).unwrap();
        assert_standard(&cfg);
    }

    #[test]
    fn parse_json() {
        let cfg: Config = serde_json::from_str(JSON).unwrap();
        assert_standard(&cfg);
    }

    #[test]
    fn parse_json5() {
        let cfg: Config = json5::from_str(JSON5).unwrap();
        assert_standard(&cfg);
    }

    #[test]
    fn parse_toml_unknown_field_rejected() {
        let bad = TOML.to_string() + "\nunknown_key = true\n";
        assert!(toml::from_str::<Config>(&bad).is_err());
    }
}
```

- [ ] **Step 2: Run to confirm tests fail**

```sh
cargo test -p aix config 2>&1 | tail -10
```

Expected: `parse_toml`, `parse_yaml`, `parse_json`, `parse_json5` fail because `find_config_path`, `load`, `validate` are `todo!()` — but the parse tests don't call those, so they may actually pass at this step! That's fine: the types are what's being tested here.

- [ ] **Step 3: Verify all four parse tests pass**

```sh
cargo test -p aix "config::tests::parse" 2>&1
```

Expected: all 4 pass (types + serde derives are correct). The `todo!()` stubs don't run.

- [ ] **Step 4: Commit**

```bash
git add tools/aix/src/config.rs
git commit -m "feat(aix): add Config/Profile/Endpoint types with multi-format parse tests"
```

---

## Task 5: Implement `find_config_path` and `load` (TDD)

**Files:**
- Modify: `tools/aix/src/config.rs`

- [ ] **Step 1: Write failing tests for `find_config_path` and `load`**

Add these tests to the `#[cfg(test)]` block in `config.rs` (after the existing tests):

```rust
    #[test]
    fn find_explicit_path_returns_it() {
        // find_config_path always returns an explicit path as-is (no existence check)
        let p = std::path::Path::new("/any/path.toml");
        let result = find_config_path(Some(p)).unwrap();
        assert_eq!(result, Some(p.to_path_buf()));
    }

    #[test]
    fn find_none_with_no_platform_config() {
        // When no explicit path and platform dir has no aix config, returns None.
        // Fully tested by CLI integration test; just verify it doesn't panic.
        let _ = find_config_path(None);
    }

    #[test]
    fn load_toml_file() {
        use assert_fs::prelude::*;
        let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
        file.write_str(TOML).unwrap();
        let cfg = load(file.path()).unwrap();
        assert_eq!(cfg.default_profile.as_deref(), Some("work"));
    }

    #[test]
    fn load_yaml_file() {
        use assert_fs::prelude::*;
        let file = assert_fs::NamedTempFile::new("aix.yaml").unwrap();
        file.write_str(YAML).unwrap();
        let cfg = load(file.path()).unwrap();
        assert_eq!(cfg.default_profile.as_deref(), Some("work"));
    }

    #[test]
    fn load_json_file() {
        use assert_fs::prelude::*;
        let file = assert_fs::NamedTempFile::new("aix.json").unwrap();
        file.write_str(JSON).unwrap();
        let cfg = load(file.path()).unwrap();
        assert_eq!(cfg.default_profile.as_deref(), Some("work"));
    }

    #[test]
    fn load_json5_file() {
        use assert_fs::prelude::*;
        let file = assert_fs::NamedTempFile::new("aix.json5").unwrap();
        file.write_str(JSON5).unwrap();
        let cfg = load(file.path()).unwrap();
        assert_eq!(cfg.default_profile.as_deref(), Some("work"));
    }

    #[test]
    fn load_unknown_extension_errors() {
        use assert_fs::prelude::*;
        let file = assert_fs::NamedTempFile::new("aix.xyz").unwrap();
        file.write_str("").unwrap();
        let err = load(file.path()).unwrap_err();
        assert!(matches!(err, AihubError::UnknownFormat { .. }));
    }

    #[test]
    fn load_missing_file_errors() {
        let result = load(std::path::Path::new("/nonexistent/aix.toml"));
        assert!(matches!(result, Err(AihubError::ParseError { .. })));
    }
```

- [ ] **Step 2: Run to confirm new tests fail**

```sh
cargo test -p aix "config::tests::load\|config::tests::find" 2>&1 | tail -15
```

Expected: panics on `todo!()`.

- [ ] **Step 3: Implement `find_config_path` and `load`**

Replace the two `todo!()` stubs in `config.rs`:

```rust
pub fn find_config_path(explicit: Option<&Path>) -> Result<Option<PathBuf>, AihubError> {
    if let Some(path) = explicit {
        return Ok(Some(path.to_path_buf()));
    }

    let Some(dirs) = ProjectDirs::from("", "", "aix") else {
        return Ok(None);
    };

    let config_dir = dirs.config_dir();
    for name in &["aix.toml", "aix.yaml", "aix.yml", "aix.json", "aix.json5"] {
        let candidate = config_dir.join(name);
        if candidate.exists() {
            return Ok(Some(candidate));
        }
    }

    Ok(None)
}

pub fn load(path: &Path) -> Result<Config, AihubError> {
    let content = std::fs::read_to_string(path).map_err(|e| AihubError::ParseError {
        path: path.to_path_buf(),
        source: Box::new(e),
    })?;

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "toml" => toml::from_str(&content).map_err(|e| AihubError::ParseError {
            path: path.to_path_buf(),
            source: Box::new(e),
        }),
        "yaml" | "yml" => serde_yaml::from_str(&content).map_err(|e| AihubError::ParseError {
            path: path.to_path_buf(),
            source: Box::new(e),
        }),
        "json" => serde_json::from_str(&content).map_err(|e| AihubError::ParseError {
            path: path.to_path_buf(),
            source: Box::new(e),
        }),
        "json5" => json5::from_str(&content).map_err(|e| AihubError::ParseError {
            path: path.to_path_buf(),
            source: Box::new(e),
        }),
        _ => Err(AihubError::UnknownFormat { ext }),
    }
}
```

- [ ] **Step 4: Run all config tests**

```sh
cargo test -p aix config 2>&1
```

Expected: all pass (parse tests + load tests + find tests). `validate` still `todo!()` but its tests aren't written yet.

- [ ] **Step 5: Lint**

```sh
cargo clippy --all-targets -- -D warnings 2>&1 | grep "^error" | head -10
```

Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add tools/aix/src/config.rs
git commit -m "feat(aix): implement find_config_path and load with format dispatch"
```

---

## Task 6: Implement `validate` (TDD)

**Files:**
- Modify: `tools/aix/src/config.rs`

- [ ] **Step 1: Write the failing validate tests**

Add to the `#[cfg(test)]` block in `config.rs`:

```rust
    #[test]
    fn validate_passes_on_well_formed_config() {
        let cfg: Config = toml::from_str(TOML).unwrap();
        assert!(validate(&cfg).is_ok());
    }

    #[test]
    fn validate_fails_when_default_profile_missing() {
        let bad = r#"
default_profile = "nonexistent"
[endpoint]
base_url = { env = "X" }
api_format = "anthropic"
[profiles.work]
api_key = "sk-test"
"#;
        let cfg: Config = toml::from_str(bad).unwrap();
        let err = validate(&cfg).unwrap_err();
        assert!(
            matches!(err, AihubError::UnknownDefaultProfile(ref name) if name == "nonexistent"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_passes_when_no_default_profile() {
        let no_default = r#"
[endpoint]
base_url = { env = "X" }
api_format = "anthropic"
[profiles.work]
api_key = "sk-test"
"#;
        let cfg: Config = toml::from_str(no_default).unwrap();
        assert!(validate(&cfg).is_ok());
    }
```

- [ ] **Step 2: Run to confirm they fail**

```sh
cargo test -p aix "config::tests::validate" 2>&1 | tail -10
```

Expected: all three panic on `todo!()`.

- [ ] **Step 3: Implement `validate`**

Replace the `validate` stub in `config.rs`:

```rust
pub fn validate(config: &Config) -> Result<(), AihubError> {
    if let Some(ref name) = config.default_profile {
        if !config.profiles.contains_key(name.as_str()) {
            return Err(AihubError::UnknownDefaultProfile(name.clone()));
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run all config tests**

```sh
cargo test -p aix config 2>&1
```

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add tools/aix/src/config.rs
git commit -m "feat(aix): implement config validation"
```

---

## Task 7: Implement config commands and CLI integration tests (TDD)

**Files:**
- Modify: `tools/aix/src/commands/config.rs`
- Modify: `tools/aix/tests/cli_help.rs`

- [ ] **Step 1: Write failing CLI integration tests**

Add to `tools/aix/tests/cli_help.rs`:

```rust
use assert_fs::prelude::*;
use predicates::prelude::*;

// Minimal valid config for integration tests.
const VALID_TOML: &str = r#"
[endpoint]
base_url = { env = "AIX_BASE_URL" }
api_format = "anthropic"

[profiles.work]
api_key = { env = "AIX_API_KEY" }
"#;

#[test]
fn config_path_prints_explicit_path() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(VALID_TOML).unwrap();
    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["config", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            file.path().to_string_lossy().as_ref(),
        ));
}

#[test]
fn config_path_no_config_found() {
    let home = assert_fs::TempDir::new().unwrap();
    cmd()
        .env("HOME", home.path())
        .env_remove("AIX_CONFIG")
        .env_remove("XDG_CONFIG_HOME")
        .args(["config", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no config file found"));
}

#[test]
fn config_validate_valid_config() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(VALID_TOML).unwrap();
    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["config", "validate"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Config is valid."));
}

#[test]
fn config_validate_invalid_config() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    // Missing required `endpoint` section → serde parse error
    file.write_str("[profiles.work]\napi_key = \"sk-test\"\n")
        .unwrap();
    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["config", "validate"])
        .assert()
        .failure();
}
```

- [ ] **Step 2: Run to confirm the four new tests fail**

```sh
cargo test -p aix --test cli_help "config_path\|config_validate" 2>&1 | tail -15
```

Expected: failures — `commands::config::run` still has the wrong signature and returns `NotImplemented`.

- [ ] **Step 3: Implement config commands**

Replace `tools/aix/src/commands/config.rs`:

```rust
use crate::cli::ConfigAction;
use crate::config;
use crate::error::AihubError;
use color_eyre::Result;
use std::path::PathBuf;

pub fn run(action: ConfigAction, config_path: Option<PathBuf>) -> Result<()> {
    match action {
        ConfigAction::Path => {
            match config::find_config_path(config_path.as_deref())? {
                Some(path) => println!("{}", path.display()),
                None => println!("no config file found"),
            }
            Ok(())
        }
        ConfigAction::Validate => {
            let path = config::find_config_path(config_path.as_deref())?
                .ok_or(AihubError::NoConfigFile)?;
            let cfg = config::load(&path)?;
            config::validate(&cfg)?;
            println!("Config is valid.");
            Ok(())
        }
    }
}
```

- [ ] **Step 4: Run the full test suite**

```sh
cargo test -p aix 2>&1
```

Expected: all tests pass — including the 4 new integration tests, all config unit tests, all secrets unit tests, and all existing CLI help tests.

- [ ] **Step 5: Format and lint**

```sh
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

Expected: no warnings, no errors.

- [ ] **Step 6: Commit**

```bash
git add tools/aix/src/commands/config.rs tools/aix/tests/cli_help.rs
git commit -m "feat(aix): implement config path and config validate commands"
```

---

## Self-Review

**Spec coverage check:**

| Spec requirement | Task |
|-----------------|------|
| TOML loads | Task 4, 5 |
| YAML loads | Task 4, 5 |
| JSON loads | Task 4, 5 |
| JSON5 loads | Task 4, 5 |
| RON not supported | Not implemented (intentional) |
| `config path` prints path | Task 7 |
| `config validate` validates without printing secrets | Task 7 |
| Config discovery precedence (explicit → env → platform) | Task 5 (`find_config_path`) |
| SecretSource: Direct, Env, File, Command | Task 3 |
| Unknown config fields fail | Task 4 (`deny_unknown_fields`) |
| `default_profile` must exist in profiles | Task 6 |
| No secret values in errors | `SecretSource` has no `Display`; errors reference field names only |
| Platform config dir via `directories` | Task 5 |
| 5 filenames tried in order | Task 5 |

**Placeholder scan:** None found.

**Type consistency:**
- `SecretSource` defined in Task 3, used identically in Task 4 (`config.rs` imports `crate::secrets::SecretSource`)
- `find_config_path` signature in Task 5 matches call site in Task 7
- `load` and `validate` signatures in Tasks 5–6 match call sites in Task 7
- `commands::config::run(action, config_path)` in Task 7 matches `main.rs` call site added in Task 2
