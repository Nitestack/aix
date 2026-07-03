# Design: aix Config Loading (Ticket 03)

**Date:** 2026-07-03
**Status:** Approved
**Scope:** Typed config model, multi-format loading, file discovery, validation, `config path` and `config validate` commands.

---

## Goals

Replace the stub `config.rs` with a real config model that:

- Loads TOML, YAML, JSON, and JSON5 from a platform-appropriate path
- Models secrets (api key, base URL) as a `SecretSource` enum — not resolved yet, just structured
- Validates structural constraints without printing secret values
- Implements `aix config path` and `aix config validate`

RON is explicitly out of scope and will not be supported.

---

## Module layout

```
src/
  config.rs    — Config/Profile/Endpoint types, find_config_path(), load(), validate()
  secrets.rs   — SecretSource type and serde helpers
  error.rs     — extended with new error variants
```

No new files. `config` crate dep is removed; `toml`, `serde_yaml`, and `json5` are added.

---

## Dependency changes

Remove:
```toml
config = ...
```

Add:
```toml
toml      = "0.8"
serde_yaml = "0.9"
json5     = "0.4"
```

`serde_json` is already present and unchanged.

---

## SecretSource (`src/secrets.rs`)

Secrets in the config file can be expressed as either a plain string or a one-key table:

```toml
api_key = "sk-..."                         # Direct
api_key = { env = "AIX_KEY" }            # Env
api_key = { file = "/run/secrets/key" }    # File
api_key = { command = "op read op://..." } # Command
```

### Public type

```rust
pub enum SecretSource {
    Direct(String),
    Env(String),
    File(PathBuf),
    Command(String),
}
```

`SecretSource` implements `Debug` but must not derive `Display` or appear in error message values — secret values must not leak into logs or terminal output.

### Serde implementation

Two private helper types drive deserialization. No hand-written `Deserialize` impl is needed:

```rust
#[derive(Deserialize)]
#[serde(untagged)]
enum SecretSourceDe {
    Direct(String),
    Structured(SecretSourceFields),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]   // unknown keys → immediate parse error
struct SecretSourceFields {
    env: Option<String>,
    file: Option<PathBuf>,
    command: Option<String>,
}
```

`SecretSourceDe` tries `Direct` (plain string) first, then `Structured` (table). Unknown table keys are rejected by `deny_unknown_fields` at parse time.

A table with more than one key set (e.g. `{ env = "X", file = "/y" }`) passes parsing but is caught by `validate()` with `AihubError::AmbiguousSecretSource`.

---

## Config types (`src/config.rs`)

All structs use `#[serde(deny_unknown_fields)]`. Unknown top-level or nested keys are a parse error.

```rust
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
```

`endpoint` is required; absent → serde parse error. `provider`, `gateway`, and `label` are optional.

---

## File discovery (`find_config_path`)

```rust
pub fn find_config_path(explicit: Option<&Path>) -> Result<Option<PathBuf>, AihubError>
```

Precedence:

1. **Explicit path** — from `--config` flag or `AIX_CONFIG` env var. If given and the file does not exist or is unreadable, return an error immediately. Do not fall through.
2. **Platform config dir** — via `directories::ProjectDirs::from("", "", "aix").config_dir()`. Try these filenames in order:
   - `aix.toml`
   - `aix.yaml`
   - `aix.yml`
   - `aix.json`
   - `aix.json5`
3. **Not found** — return `Ok(None)`. Callers that require a config convert this to `AihubError::NoConfigFile`.

---

## Format dispatch (`load`)

```rust
pub fn load(path: &Path) -> Result<Config, AihubError>
```

Reads the file, dispatches on the lowercase extension:

| Extension      | Parser              |
|----------------|---------------------|
| `toml`         | `toml::from_str`    |
| `yaml`, `yml`  | `serde_yaml::from_str` |
| `json`         | `serde_json::from_str` |
| `json5`        | `json5::from_str`   |
| anything else  | `AihubError::UnknownFormat { ext }` |

Parse errors are wrapped as `AihubError::ParseError { path, source }`. The path is included so the user knows which file failed.

---

## Validation (`validate`)

```rust
pub fn validate(config: &Config) -> Result<(), AihubError>
```

Three checks, in order:

1. **default_profile exists** — if `Some(name)`, `name` must be a key in `profiles`. Error: `AihubError::UnknownDefaultProfile(name)`.
2. **api_key uniqueness** — for every profile, the `SecretSourceFields` variant (if used) must have exactly one field set. Error: `AihubError::AmbiguousSecretSource { field: "profiles.<name>.api_key" }`.
3. **base_url uniqueness** — same check on `endpoint.base_url`. Error: `AihubError::AmbiguousSecretSource { field: "endpoint.base_url" }`.

No secret values appear in any error message. Errors reference field names only.

---

## Error variants (additions to `src/error.rs`)

```rust
#[error("unknown config format: {ext}")]
UnknownFormat { ext: String },

#[error("failed to parse {path}: {source}")]
ParseError {
    path: PathBuf,
    #[source] source: Box<dyn std::error::Error + Send + Sync>,
},

#[error("default_profile \"{0}\" is not defined in profiles")]
UnknownDefaultProfile(String),

#[error("ambiguous secret source for {field}: specify exactly one of env / file / command")]
AmbiguousSecretSource { field: String },

#[error("no config file found")]
NoConfigFile,
```

---

## Commands

### `aix config path`

```
find_config_path(cli.config) →
  Some(path) → println!("{}", path.display())
  None       → println!("no config file found")
```

Always exits 0. Prints to stdout.

### `aix config validate`

```
find_config_path(cli.config) →
  None → Err(AihubError::NoConfigFile)
  Some(path) →
    load(&path) → propagate ParseError
    validate(&config) → propagate validation error
    println!("Config is valid.")
```

On success: prints `Config is valid.` to stdout, exits 0.
On error: `color-eyre` formats the error to stderr, exits non-zero. No secret values in output.

---

## Testing

### Format coverage — `tests/config.rs`

Four tests using `assert_fs` to write a temp config file, each format containing an equivalent config with `default_profile`, `endpoint` (with `base_url` as env source), and two profiles (one env api_key, one direct api_key):

```
test_load_toml
test_load_yaml
test_load_json
test_load_json5
```

### SecretSource variants — inline unit tests in `src/secrets.rs`

```
test_secret_source_direct          plain string → SecretSource::Direct
test_secret_source_env             { env = "X" } → SecretSource::Env
test_secret_source_file            { file = "/p" } → SecretSource::File
test_secret_source_command         { command = "..." } → SecretSource::Command
test_secret_source_ambiguous       { env = "X", file = "/y" } → fails validation
test_secret_source_unknown_key     { keyring = "X" } → parse error
```

### Validation — inline unit tests in `src/config.rs`

```
test_validate_passes                    well-formed config passes
test_validate_missing_default_profile   default_profile names nonexistent profile → error
```

### Command integration — in `tests/cli_help.rs`

```
test_config_path_no_config     no config present → exit 0, "no config file found"
test_config_validate_valid     valid toml in temp dir → exit 0, "Config is valid."
test_config_validate_invalid   malformed toml → exit non-zero, error to stderr
```

No live endpoints. No env var mutation. No secret resolution.

---

## Acceptance criteria

- `cargo test` passes
- `cargo fmt --check` passes
- `cargo clippy --all-targets -- -D warnings` passes
- TOML, YAML, JSON, JSON5 configs load and parse to identical `Config` values
- `aix config path` prints the resolved path or "no config file found"
- `aix config validate` validates structure without printing secrets
- Unknown config fields fail with a parse error
- Ambiguous SecretSource (multiple keys) fails validation

---

## Out of scope

- RON format
- Secret resolution (`SecretSource::resolve()`) — deferred to a later ticket
- Interactive profile picker
- Local project fallback (`.env` auto-load)
- Keychain integration
