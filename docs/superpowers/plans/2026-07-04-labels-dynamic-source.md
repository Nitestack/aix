# Labels Dynamic Source Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow profile `label` to be read from an env var, file, or command — same syntax as `api_key` — so label text can be kept out of the world-readable config file.

**Architecture:** Extract a private `SourceKind` enum shared by `SecretSource` and the new `DynamicValue` type. Both are thin wrappers that delegate to `SourceKind::resolve_raw()`, differing only in output type (`SecretString` vs `String`). `Profile.label` changes from `Option<String>` to `Option<DynamicValue>`.

**Tech Stack:** Rust, serde, toml/serde_yaml/serde_json/json5, Nix (home-manager)

---

## File Map

| File | Change |
|---|---|
| `src/secrets.rs` | Extract `SourceKind`; add `DynamicValue`; refactor `SecretSource` to wrap `SourceKind` |
| `src/config.rs` | `Profile.label: Option<DynamicValue>`; update `sorted_profiles`, `validate`, `format_available_profiles` |
| `src/commands/profiles.rs` | Handle `sorted_profiles` returning `Result<Vec<(&str, String)>>` |
| `src/commands/env.rs` | Handle `sorted_profiles` returning `Result` in `select_profile_interactively` |
| `nix/home-manager.nix` | `label` option type → `nullOr secretSourceType`; `mkProfile` encodes label via `encodeSecretSource` |

---

### Task 1: Extract `SourceKind` and refactor `SecretSource` to wrap it

This is a pure refactor — no behaviour change. All existing tests must pass after.

**Files:**
- Modify: `src/secrets.rs`

- [ ] **Step 1: Run the existing secrets tests to establish a baseline**

```bash
cargo test -- secrets 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 2: Replace the `SecretSource` enum with a private `SourceKind` enum + public wrapper struct**

In `src/secrets.rs`, replace the entire block from the `SecretSource` enum definition through the end of `SecretSource`'s `Deserialize` impl with the following (keep everything above — `SecretString` and its impls — unchanged, and keep everything below — `Helpers` section and tests — unchanged for now):

```rust
// ---------------------------------------------------------------------------
// SourceKind — private inner type shared by SecretSource and DynamicValue
// ---------------------------------------------------------------------------

pub(crate) enum SourceKind {
    Direct(String),
    Env(String),
    File(PathBuf),
    Command(String),
}

impl SourceKind {
    fn resolve_raw(&self) -> Result<String, AixError> {
        match self {
            SourceKind::Direct(s) => Ok(s.clone()),

            SourceKind::Env(name) => std::env::var(name)
                .map_err(|_| AixError::SecretMissingEnvVar { name: name.clone() }),

            SourceKind::File(raw_path) => {
                let expanded = shellexpand::tilde(&raw_path.to_string_lossy()).into_owned();
                let path = PathBuf::from(expanded);
                let content = std::fs::read_to_string(&path)
                    .map_err(|source| AixError::SecretFileRead { path, source })?;
                Ok(strip_one_trailing_newline(content))
            }

            SourceKind::Command(cmd) => {
                let output = run_command(cmd)?;
                Ok(strip_one_trailing_newline(output))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shared deserialization helpers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(untagged)]
enum SourceKindDe {
    Direct(String),
    Structured(SourceKindFields),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceKindFields {
    env: Option<String>,
    file: Option<PathBuf>,
    command: Option<String>,
}

fn try_from_de<E: serde::de::Error>(de: SourceKindDe) -> Result<SourceKind, E> {
    match de {
        SourceKindDe::Direct(s) => Ok(SourceKind::Direct(s)),
        SourceKindDe::Structured(f) => match (f.env, f.file, f.command) {
            (Some(v), None, None) => Ok(SourceKind::Env(v)),
            (None, Some(p), None) => Ok(SourceKind::File(p)),
            (None, None, Some(c)) => Ok(SourceKind::Command(c)),
            (None, None, None) => Err(serde::de::Error::custom(
                "secret source table must specify one of: env, file, command",
            )),
            _ => Err(serde::de::Error::custom(
                "ambiguous secret source: specify exactly one of env / file / command",
            )),
        },
    }
}

// ---------------------------------------------------------------------------
// SecretSource — describes where to fetch a secret from
// ---------------------------------------------------------------------------

pub struct SecretSource(pub(crate) SourceKind);

impl fmt::Debug for SecretSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            SourceKind::Direct(_) => f.write_str("SecretSource::Direct([redacted])"),
            SourceKind::Env(name) => write!(f, "SecretSource::Env({name:?})"),
            SourceKind::File(path) => write!(f, "SecretSource::File({path:?})"),
            SourceKind::Command(cmd) => write!(f, "SecretSource::Command({cmd:?})"),
        }
    }
}

impl SecretSource {
    pub fn resolve(&self) -> Result<SecretString, AixError> {
        self.0.resolve_raw().map(SecretString::new)
    }
}

impl<'de> Deserialize<'de> for SecretSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let de = SourceKindDe::deserialize(deserializer)?;
        try_from_de(de).map(SecretSource)
    }
}
```

- [ ] **Step 3: Update the `secrets.rs` deserialization tests to match the new struct shape**

The tests use `matches!(src, SecretSource::Direct(...))` — now `SecretSource` is a struct, so match on `src.0` instead. In `src/secrets.rs` inside `mod tests`, update the four deserialization tests:

```rust
#[test]
fn deser_direct() {
    let src = from_toml("value = \"sk-test\"").unwrap();
    assert!(matches!(src.0, SourceKind::Direct(ref s) if s == "sk-test"));
}

#[test]
fn deser_env() {
    let src = from_toml("value = { env = \"MY_KEY\" }").unwrap();
    assert!(matches!(src.0, SourceKind::Env(ref s) if s == "MY_KEY"));
}

#[test]
fn deser_file() {
    let src = from_toml("value = { file = \"/run/secrets/key\" }").unwrap();
    assert!(
        matches!(src.0, SourceKind::File(ref p) if p == std::path::Path::new("/run/secrets/key"))
    );
}

#[test]
fn deser_command() {
    let src = from_toml("value = { command = \"op read op://Work/key\" }").unwrap();
    assert!(matches!(src.0, SourceKind::Command(ref s) if s == "op read op://Work/key"));
}
```

Also update the `secret_source_debug_hides_direct_value` test which constructs `SecretSource::Direct(...)` directly:

```rust
#[test]
fn secret_source_debug_hides_direct_value() {
    let src = SecretSource(SourceKind::Direct("sk-direct-value".to_string()));
    let dbg = format!("{src:?}");
    assert!(!dbg.contains("sk-direct-value"));
    assert!(dbg.contains("redacted"));
}

#[test]
fn secret_source_debug_shows_env_name() {
    let src = SecretSource(SourceKind::Env("MY_VAR".to_string()));
    let dbg = format!("{src:?}");
    assert!(dbg.contains("MY_VAR"));
}
```

- [ ] **Step 4: Update the `config.rs` tests that pattern-match on `SecretSource` variants**

In `src/config.rs` inside `mod tests`, add the import and update `assert_standard`:

```rust
// Add at top of mod tests:
use crate::secrets::SourceKind;

fn assert_standard(cfg: &Config) {
    assert_eq!(cfg.default_profile.as_deref(), Some("work"));
    assert_eq!(cfg.endpoint.api_format, ApiFormat::Anthropic);
    assert_eq!(
        cfg.endpoint.provider,
        Some(Provider::Known(KnownProvider::LiteLlm))
    );
    assert!(matches!(cfg.endpoint.base_url.0, SourceKind::Env(_)));
    assert!(cfg.profiles.contains_key("work"));
    assert!(cfg.profiles.contains_key("local"));
    assert!(matches!(cfg.profiles["work"].api_key.0, SourceKind::Env(_)));
    assert!(matches!(cfg.profiles["local"].api_key.0, SourceKind::Direct(_)));
}
```

- [ ] **Step 5: Run all tests**

```bash
cargo test 2>&1 | tail -30
```

Expected: all tests pass.

- [ ] **Step 6: Run clippy**

```bash
cargo clippy --all-targets -- -D warnings 2>&1 | tail -20
```

Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
git add src/secrets.rs src/config.rs
git commit -m "refactor(secrets): extract SourceKind, wrap SecretSource around it"
```

---

### Task 2: Add `DynamicValue` type

**Files:**
- Modify: `src/secrets.rs`

- [ ] **Step 1: Write the failing tests for `DynamicValue`**

In `src/secrets.rs` inside `mod tests`, add after the existing `SecretSource` tests:

```rust
// ---------------------------------------------------------------------------
// DynamicValue tests
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct WDyn {
    value: DynamicValue,
}

fn from_toml_dyn(s: &str) -> Result<DynamicValue, toml::de::Error> {
    toml::from_str::<WDyn>(s).map(|w| w.value)
}

#[test]
fn dynamic_value_deser_direct() {
    let dv = from_toml_dyn("value = \"Work\"").unwrap();
    assert!(matches!(dv.0, SourceKind::Direct(ref s) if s == "Work"));
}

#[test]
fn dynamic_value_deser_env() {
    let dv = from_toml_dyn("value = { env = \"MY_LABEL\" }").unwrap();
    assert!(matches!(dv.0, SourceKind::Env(ref s) if s == "MY_LABEL"));
}

#[test]
fn dynamic_value_deser_file() {
    let dv = from_toml_dyn("value = { file = \"/run/labels/work\" }").unwrap();
    assert!(
        matches!(dv.0, SourceKind::File(ref p) if p == std::path::Path::new("/run/labels/work"))
    );
}

#[test]
fn dynamic_value_deser_command() {
    let dv = from_toml_dyn("value = { command = \"pass show label\" }").unwrap();
    assert!(matches!(dv.0, SourceKind::Command(ref s) if s == "pass show label"));
}

#[test]
fn dynamic_value_deser_empty_table_rejected() {
    assert!(from_toml_dyn("value = {}").is_err());
}

#[test]
fn dynamic_value_deser_ambiguous_rejected() {
    assert!(from_toml_dyn("value = { env = \"X\", file = \"/y\" }").is_err());
}

#[test]
fn dynamic_value_resolve_direct() {
    let dv = from_toml_dyn("value = \"Work account\"").unwrap();
    assert_eq!(dv.resolve().unwrap(), "Work account");
}

#[test]
fn dynamic_value_resolve_env_set() {
    let var = "AIX_TEST_DYN_RESOLVE_ENV_V1Q2";
    std::env::set_var(var, "env-label");
    let dv = from_toml_dyn(&format!("value = {{ env = \"{var}\" }}")).unwrap();
    let result = dv.resolve();
    std::env::remove_var(var);
    assert_eq!(result.unwrap(), "env-label");
}

#[test]
fn dynamic_value_resolve_env_missing() {
    let var = "AIX_TEST_DYN_RESOLVE_ENV_MISSING_Z9W8";
    std::env::remove_var(var);
    let dv = from_toml_dyn(&format!("value = {{ env = \"{var}\" }}")).unwrap();
    let err = dv.resolve().unwrap_err();
    assert!(matches!(err, AixError::SecretMissingEnvVar { ref name } if name == var));
}

#[test]
fn dynamic_value_resolve_file() {
    use assert_fs::prelude::*;
    let tmp = assert_fs::NamedTempFile::new("label").unwrap();
    tmp.write_str("Work account\n").unwrap();
    let dv = DynamicValue(SourceKind::File(tmp.path().to_path_buf()));
    assert_eq!(dv.resolve().unwrap(), "Work account");
}

#[test]
fn dynamic_value_debug_shows_direct_value() {
    let dv = from_toml_dyn("value = \"Work\"").unwrap();
    let dbg = format!("{dv:?}");
    assert!(dbg.contains("Work"), "direct label value must be visible in debug: {dbg}");
}

#[test]
fn dynamic_value_debug_shows_env_name() {
    let dv = from_toml_dyn("value = { env = \"MY_LABEL\" }").unwrap();
    let dbg = format!("{dv:?}");
    assert!(dbg.contains("MY_LABEL"));
}
```

- [ ] **Step 2: Run to confirm the tests fail**

```bash
cargo test dynamic_value 2>&1 | tail -10
```

Expected: compile error — `DynamicValue` not defined.

- [ ] **Step 3: Implement `DynamicValue`**

In `src/secrets.rs`, add after the `SecretSource` impl block (before the `Helpers` section):

```rust
// ---------------------------------------------------------------------------
// DynamicValue — dynamic but non-secret string source
// ---------------------------------------------------------------------------

pub struct DynamicValue(pub(crate) SourceKind);

impl fmt::Debug for DynamicValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            SourceKind::Direct(s) => write!(f, "DynamicValue::Direct({s:?})"),
            SourceKind::Env(name) => write!(f, "DynamicValue::Env({name:?})"),
            SourceKind::File(path) => write!(f, "DynamicValue::File({path:?})"),
            SourceKind::Command(cmd) => write!(f, "DynamicValue::Command({cmd:?})"),
        }
    }
}

impl DynamicValue {
    pub fn resolve(&self) -> Result<String, AixError> {
        self.0.resolve_raw()
    }
}

impl<'de> Deserialize<'de> for DynamicValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let de = SourceKindDe::deserialize(deserializer)?;
        try_from_de(de).map(DynamicValue)
    }
}
```

- [ ] **Step 4: Run the new tests**

```bash
cargo test dynamic_value 2>&1 | tail -20
```

Expected: all `dynamic_value_*` tests pass.

- [ ] **Step 5: Run all tests and clippy**

```bash
cargo test 2>&1 | tail -10
cargo clippy --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: all pass, no warnings.

- [ ] **Step 6: Commit**

```bash
git add src/secrets.rs
git commit -m "feat(secrets): add DynamicValue type for non-secret dynamic string sources"
```

---

### Task 3: Update `Profile.label` in `config.rs`

**Files:**
- Modify: `src/config.rs`

- [ ] **Step 1: Write the failing tests**

In `src/config.rs` inside `mod tests`, add after the existing `sorted_profiles` tests:

```rust
#[test]
fn sorted_profiles_with_env_label() {
    let var = "AIX_TEST_SORTED_PROFILES_LABEL_A1B2";
    std::env::set_var(var, "My Work Label");
    let cfg: Config = toml::from_str(&format!(
        r#"
[endpoint]
base_url = {{ env = "X" }}
api_format = "anthropic"
[profiles.work]
label = {{ env = "{var}" }}
api_key = "sk-test"
"#
    ))
    .unwrap();
    let pairs = sorted_profiles(&cfg);
    std::env::remove_var(var);
    let pairs = pairs.unwrap();
    assert_eq!(pairs[0].1, "My Work Label");
}

#[test]
fn sorted_profiles_missing_env_label_returns_err() {
    let var = "AIX_TEST_SORTED_PROFILES_LABEL_MISSING_C3D4";
    std::env::remove_var(var);
    let cfg: Config = toml::from_str(&format!(
        r#"
[endpoint]
base_url = {{ env = "X" }}
api_format = "anthropic"
[profiles.work]
label = {{ env = "{var}" }}
api_key = "sk-test"
"#
    ))
    .unwrap();
    assert!(sorted_profiles(&cfg).is_err());
}

#[test]
fn validate_surfaces_bad_label_source() {
    let var = "AIX_TEST_VALIDATE_LABEL_MISSING_E5F6";
    std::env::remove_var(var);
    let cfg: Config = toml::from_str(&format!(
        r#"
[endpoint]
base_url = {{ env = "X" }}
api_format = "anthropic"
[profiles.work]
label = {{ env = "{var}" }}
api_key = "sk-test"
"#
    ))
    .unwrap();
    let err = validate(&cfg).unwrap_err();
    assert!(
        matches!(err, AixError::SecretMissingEnvVar { .. }),
        "expected SecretMissingEnvVar, got: {err}"
    );
}

#[test]
fn format_available_profiles_falls_back_on_bad_label() {
    let var = "AIX_TEST_FORMAT_PROFILES_LABEL_BAD_G7H8";
    std::env::remove_var(var);
    let cfg: Config = toml::from_str(&format!(
        r#"
[endpoint]
base_url = {{ env = "X" }}
api_format = "anthropic"
[profiles.work]
label = {{ env = "{var}" }}
api_key = "sk-test"
"#
    ))
    .unwrap();
    let out = format_available_profiles(&cfg);
    assert!(out.contains("work"), "should fall back to profile name: {out}");
}
```

- [ ] **Step 2: Run to confirm the new tests fail**

```bash
cargo test "sorted_profiles_with_env_label|sorted_profiles_missing|validate_surfaces_bad_label|format_available_profiles_falls_back" 2>&1 | tail -10
```

Expected: compile errors — `label` field type mismatch.

- [ ] **Step 3: Update `Profile` struct and imports in `config.rs`**

Add `DynamicValue` to the import at the top of `src/config.rs`:

```rust
use crate::secrets::{DynamicValue, SecretSource};
```

Change `Profile.label`:

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub label: Option<DynamicValue>,
    pub api_key: SecretSource,
}
```

- [ ] **Step 4: Update `sorted_profiles`**

Replace the current `sorted_profiles` function:

```rust
pub fn sorted_profiles(cfg: &Config) -> Result<Vec<(&str, String)>, AixError> {
    let mut pairs: Vec<(&str, String)> = Vec::new();
    for (name, profile) in &cfg.profiles {
        let label = match &profile.label {
            None => name.to_string(),
            Some(dv) => dv.resolve()?,
        };
        pairs.push((name.as_str(), label));
    }
    pairs.sort_by_key(|(name, _)| *name);
    Ok(pairs)
}
```

- [ ] **Step 5: Update `validate` to resolve labels early**

Replace the label-checking loop inside `validate`:

```rust
let mut seen: std::collections::HashMap<String, &str> = std::collections::HashMap::new();
for name in sorted_names {
    let effective_label: String = match &config.profiles[name].label {
        None => name.to_string(),
        Some(dv) => dv.resolve()?,
    };
    if let Some(first) = seen.get(effective_label.as_str()) {
        return Err(AixError::DuplicateLabel {
            label: effective_label.clone(),
            first: first.to_string(),
            second: name.to_string(),
        });
    }
    seen.insert(effective_label, name);
}
```

- [ ] **Step 6: Update `format_available_profiles` (best-effort, no `Result`)**

Replace the current `format_available_profiles` function:

```rust
pub fn format_available_profiles(cfg: &Config) -> String {
    let mut pairs: Vec<(&str, String)> = cfg
        .profiles
        .iter()
        .map(|(name, profile)| {
            let label = profile
                .label
                .as_ref()
                .and_then(|dv| dv.resolve().ok())
                .unwrap_or_else(|| name.to_string());
            (name.as_str(), label)
        })
        .collect();
    pairs.sort_by_key(|(name, _)| *name);
    if pairs.is_empty() {
        return "  (no profiles defined)".to_string();
    }
    pairs
        .iter()
        .map(|(name, label)| {
            if *name == label.as_str() {
                format!("  {name}")
            } else {
                format!("  {name} ({label})")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
```

- [ ] **Step 7: Update existing `config.rs` tests that call `sorted_profiles`**

Every existing test that calls `sorted_profiles(&cfg)` and uses the result directly must add `.unwrap()`. Find and update all occurrences. The tests that need this are: `sorted_profiles_sorted_by_name`, `sorted_profiles_label_falls_back_to_name`, `sorted_profiles_uses_label_when_set`, `format_available_profiles_single_space_before_label_paren`, `format_available_profiles_omits_parens_when_label_equals_name`, `format_available_profiles_omits_parens_when_no_label`.

For example:
```rust
#[test]
fn sorted_profiles_sorted_by_name() {
    // ... (cfg setup unchanged)
    let pairs = sorted_profiles(&cfg).unwrap();  // added .unwrap()
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].0, "aaa");
    assert_eq!(pairs[1].0, "zzz");
}

#[test]
fn sorted_profiles_label_falls_back_to_name() {
    // ... (cfg setup unchanged)
    let pairs = sorted_profiles(&cfg).unwrap();  // added .unwrap()
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].0, "myprofile");
    assert_eq!(pairs[0].1, "myprofile");
}

#[test]
fn sorted_profiles_uses_label_when_set() {
    // ... (cfg setup unchanged)
    let pairs = sorted_profiles(&cfg).unwrap();  // added .unwrap()
    assert_eq!(pairs[0].1, "Work account");
}
```

- [ ] **Step 8: Run all tests**

```bash
cargo test 2>&1 | tail -30
```

Expected: all tests pass.

- [ ] **Step 9: Run clippy and fmt**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: no warnings.

- [ ] **Step 10: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): Profile.label accepts DynamicValue (env/file/command/direct)"
```

---

### Task 4: Update `commands/profiles.rs`

**Files:**
- Modify: `src/commands/profiles.rs`

- [ ] **Step 1: Update `run`, `print_json`, and `print_text` for the new `sorted_profiles` signature**

Replace the entire file content:

```rust
use crate::config;
use crate::error::AixError;
use color_eyre::Result;
use serde::Serialize;
use std::path::PathBuf;

pub fn run(config_path: Option<PathBuf>, json: bool) -> Result<()> {
    let path = config::find_config_path(config_path.as_deref())?.ok_or(AixError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;

    let profiles = config::sorted_profiles(&cfg)?;

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

fn print_json(profiles: &[(&str, String)]) -> Result<()> {
    let entries: Vec<ProfileEntry<'_>> = profiles
        .iter()
        .map(|(name, label)| ProfileEntry { name, label })
        .collect();
    let mut out = serde_json::to_string_pretty(&entries)?;
    out.push('\n');
    print!("{out}");
    Ok(())
}

fn print_text(profiles: &[(&str, String)]) {
    for (name, label) in profiles {
        if *name == label.as_str() {
            println!("{name}");
        } else {
            println!("{name}  ({label})");
        }
    }
}
```

- [ ] **Step 2: Run all tests**

```bash
cargo test 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/commands/profiles.rs
git commit -m "fix(commands/profiles): handle sorted_profiles returning Result"
```

---

### Task 5: Update `commands/env.rs`

**Files:**
- Modify: `src/commands/env.rs`

- [ ] **Step 1: Update `select_profile_interactively`**

Replace the `select_profile_interactively` function:

```rust
fn select_profile_interactively(cfg: &config::Config) -> Result<String, AixError> {
    let profiles = config::sorted_profiles(cfg)?;
    let options: Vec<String> = profiles
        .iter()
        .map(|(_, label)| label.clone())
        .collect();

    let selected = inquire::Select::new("Select a profile:", options)
        .prompt()
        .map_err(|_| AixError::SelectionCancelled)?;

    // validate() has already rejected duplicate labels, so this find is unambiguous.
    profiles
        .into_iter()
        .find(|(_, label)| label == selected.as_str())
        .map(|(name, _)| name.to_string())
        .ok_or_else(|| AixError::ProfileNotFound {
            name: selected,
            available_hint: config::format_available_profiles(cfg),
        })
}
```

- [ ] **Step 2: Run all tests**

```bash
cargo test 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 3: Run clippy and fmt**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: no warnings.

- [ ] **Step 4: Commit**

```bash
git add src/commands/env.rs
git commit -m "fix(commands/env): handle sorted_profiles returning Result"
```

---

### Task 6: Update `nix/home-manager.nix`

**Files:**
- Modify: `nix/home-manager.nix`

- [ ] **Step 1: Update the `label` option type**

In `nix/home-manager.nix`, find the `label` option inside `profiles` and replace it:

```nix
label = lib.mkOption {
  type = lib.types.nullOr secretSourceType;
  default = null;
  apply = v: if v != null then validateSecretSource v else null;
  example = lib.literalExpression ''{ file = "/run/secrets/aix/work-label"; }'';
  description = ''
    Display label shown in the interactive profile picker.
    Accepts a plain string, or { env = "VAR"; }, { file = "/path"; }, { command = "cmd"; }.
    Use a non-literal source to avoid leaking account names into the Nix store.
  '';
};
```

- [ ] **Step 2: Update `mkProfile` to encode the label via `encodeSecretSource`**

Replace the `mkProfile` let-binding:

```nix
mkProfile =
  _name: profile:
  {
    api_key = encodeSecretSource profile.apiKey;
  }
  // lib.optionalAttrs (profile.label != null) {
    label = encodeSecretSource profile.label;
  };
```

- [ ] **Step 3: Run `nixfmt`**

```bash
nixfmt nix/home-manager.nix
```

Expected: file formatted with no errors.

- [ ] **Step 4: Commit**

```bash
git add nix/home-manager.nix
git commit -m "feat(nix): profile label accepts secret source (env/file/command)"
```
