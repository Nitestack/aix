# Spend Cache Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a TTL-based disk cache for `aix spend` API responses, keyed by LiteLLM user account, so repeated calls within the same session skip the network — and calls for different API keys that belong to the same user account share a single cache entry.

**Architecture:** A new `src/cache.rs` module handles all disk I/O using a two-level structure: an `index.json` mapping API-key suffixes (last 4 chars) to user IDs, and per-user JSON files storing timestamped responses. On every successful fetch, all key suffixes found in the response's `keys` array are written into the index, warming future calls for sibling keys. FNV-1a provides deterministic stable hashing with no new dependency. Cache reads are silently swallowed on any error (corrupt file, missing dir); cache writes are also silent. `aix cache clear` surfaces I/O errors.

**Tech Stack:** `std::fs`, `std::time::SystemTime`, `serde_json`, `directories` (already in deps), `assert_fs` + `wiremock` for tests. No new crates.

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `src/config.rs` | Modify | Add `CacheConfig` struct and `cache` field to `Config` |
| `src/cache.rs` | Create | Two-level disk cache: `get`, `put`, `clear`, index management |
| `src/cli.rs` | Modify | Add `--no-cache` to `Spend`; add `Cache` subcommand + `CacheAction` enum |
| `src/main.rs` | Modify | Add `mod cache;`; route `Command::Cache`; pass `no_cache` to spend |
| `src/commands/mod.rs` | Modify | Add `pub mod cache;` |
| `src/commands/cache.rs` | Create | `aix cache clear` handler |
| `src/commands/spend.rs` | Modify | Accept `no_cache` flag; consult/write cache |
| `tests/spend_test.rs` | Modify | Set `AIX_CACHE_DIR` in existing tests; add cache integration test |

---

### Task 1: CacheConfig in config.rs

**Files:**
- Modify: `src/config.rs`

- [ ] **Step 1: Write failing tests**

Add to the `#[cfg(test)]` block in `src/config.rs`:

```rust
#[test]
fn cache_config_defaults_when_section_absent() {
    let cfg: Config = toml::from_str(TOML).unwrap();
    assert_eq!(cfg.cache.ttl_secs, 3600);
    assert!(!cfg.cache.disabled);
}

#[test]
fn cache_config_parses_full_section() {
    let toml = r#"
[endpoint]
base_url = { env = "X" }
[profiles.work]
api_key = "sk-test"
[cache]
ttl_secs = 600
disabled = true
"#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert_eq!(cfg.cache.ttl_secs, 600);
    assert!(cfg.cache.disabled);
}

#[test]
fn cache_config_partial_section_uses_field_defaults() {
    let toml = r#"
[endpoint]
base_url = { env = "X" }
[profiles.work]
api_key = "sk-test"
[cache]
disabled = true
"#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert_eq!(cfg.cache.ttl_secs, 3600);
    assert!(cfg.cache.disabled);
}

#[test]
fn cache_config_rejects_unknown_fields() {
    let toml = r#"
[endpoint]
base_url = { env = "X" }
[profiles.work]
api_key = "sk-test"
[cache]
unknown_key = "bad"
"#;
    assert!(toml::from_str::<Config>(toml).is_err());
}
```

- [ ] **Step 2: Run tests to confirm they fail**

```bash
cargo test -q cache_config 2>&1 | head -20
```

Expected: errors about missing `cache` field on `Config`.

- [ ] **Step 3: Add CacheConfig and cache field**

Add directly before `pub fn find_config_path` in `src/config.rs`:

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    #[serde(default = "default_cache_ttl")]
    pub ttl_secs: u64,
    #[serde(default)]
    pub disabled: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self { ttl_secs: default_cache_ttl(), disabled: false }
    }
}

fn default_cache_ttl() -> u64 {
    3600
}
```

Add to the `Config` struct after the `env_files` field:

```rust
#[serde(default)]
pub cache: CacheConfig,
```

- [ ] **Step 4: Run tests to confirm they pass**

```bash
cargo test -q cache_config 2>&1
```

Expected: 4 tests pass, no failures.

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/config.rs
git commit -m "feat(cache): add CacheConfig to config"
```

---

### Task 2: src/cache.rs — skeleton, helpers, and unit tests

**Files:**
- Create: `src/cache.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Create src/cache.rs with helpers and failing tests**

Create `src/cache.rs`:

```rust
use crate::config::CacheConfig;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct Cache {
    base_dir: PathBuf,
    ttl_secs: u64,
    disabled: bool,
}

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    fetched_at: u64,
    data: Value,
}

impl Cache {
    pub fn from_config(cfg: &CacheConfig) -> Self {
        let base_dir = std::env::var("AIX_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                ProjectDirs::from("", "", "aix")
                    .map(|d| d.cache_dir().to_path_buf())
                    .unwrap_or_else(|| PathBuf::from(".cache/aix"))
            });
        Self { base_dir, ttl_secs: cfg.ttl_secs, disabled: cfg.disabled }
    }

    pub fn get(&self, _base_url: &str, _api_key: &str) -> Option<Value> {
        todo!()
    }

    pub fn put(&self, _base_url: &str, _api_key: &str, _data: &Value) {
        todo!()
    }

    pub fn clear(&self) -> std::io::Result<usize> {
        todo!()
    }

    fn endpoint_dir(&self, base_url: &str) -> PathBuf {
        self.base_dir.join(fnv1a_hex(base_url.as_bytes()))
    }
}

fn key_suffix(api_key: &str) -> String {
    let len = api_key.len();
    api_key[len.saturating_sub(4)..].to_string()
}

fn derive_user_id(api_key: &str, data: &Value) -> String {
    if let Some(uid) = data.get("user_id").and_then(|v| v.as_str()) {
        return uid.to_string();
    }
    format!("key:{}", fnv1a_hex(api_key.as_bytes()))
}

fn load_index(dir: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(dir.join("index.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_index(dir: &Path, index: &HashMap<String, String>) -> std::io::Result<()> {
    let json = serde_json::to_string(index).map_err(std::io::Error::other)?;
    write_atomic(&dir.join("index.json"), &json)
}

fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

fn fnv1a_hex(data: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in data {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;

    fn test_cache(dir: &TempDir) -> Cache {
        Cache {
            base_dir: dir.path().to_path_buf(),
            ttl_secs: 3600,
            disabled: false,
        }
    }

    #[test]
    fn key_suffix_last_4() {
        assert_eq!(key_suffix("sk-abcdefgh"), "efgh");
    }

    #[test]
    fn key_suffix_shorter_than_4() {
        assert_eq!(key_suffix("ab"), "ab");
    }

    #[test]
    fn key_suffix_exactly_4() {
        assert_eq!(key_suffix("abcd"), "abcd");
    }

    #[test]
    fn derive_user_id_uses_response_field() {
        let data = serde_json::json!({ "user_id": "u-123", "spend": 1.0 });
        assert_eq!(derive_user_id("sk-any", &data), "u-123");
    }

    #[test]
    fn derive_user_id_falls_back_to_deterministic_hash() {
        let data = serde_json::json!({ "spend": 1.0 });
        let id = derive_user_id("sk-mykey", &data);
        assert!(id.starts_with("key:"), "got: {id}");
        assert_eq!(derive_user_id("sk-mykey", &data), id, "must be deterministic");
        assert_ne!(derive_user_id("sk-other", &data), id, "different key → different id");
    }

    #[test]
    fn fnv1a_hex_is_deterministic_and_unique() {
        assert_eq!(fnv1a_hex(b"hello"), fnv1a_hex(b"hello"));
        assert_ne!(fnv1a_hex(b"hello"), fnv1a_hex(b"world"));
        assert_eq!(fnv1a_hex(b"hello").len(), 16);
    }
}
```

- [ ] **Step 2: Register the module in src/main.rs**

Add `mod cache;` to the module declarations at the top of `src/main.rs`, alongside `mod config;`:

```rust
mod cache;
```

- [ ] **Step 3: Run the helper tests**

```bash
cargo test -q 'cache::tests::key_suffix' 'cache::tests::derive_user_id' 'cache::tests::fnv1a_hex' 2>&1
```

Expected: 5 tests pass. Ignore the `todo!()` panics — we're not running `get`/`put`/`clear` yet.

- [ ] **Step 4: Lint and commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/cache.rs src/main.rs
git commit -m "feat(cache): add cache module skeleton and helpers"
```

---

### Task 3: Cache::get and Cache::put

**Files:**
- Modify: `src/cache.rs`

- [ ] **Step 1: Write failing tests**

Add to the `tests` block in `src/cache.rs`:

```rust
#[test]
fn get_returns_none_on_empty_cache() {
    let dir = TempDir::new().unwrap();
    let cache = test_cache(&dir);
    assert!(cache.get("https://api.example.com", "sk-abc123").is_none());
}

#[test]
fn put_then_get_returns_data() {
    let dir = TempDir::new().unwrap();
    let cache = test_cache(&dir);
    let data = serde_json::json!({
        "user_id": "u-test",
        "spend": 2.5,
        "keys": [{ "key_name": "sk-...X1Y2" }]
    });
    cache.put("https://api.example.com", "sk-X1Y2", &data);
    let result = cache.get("https://api.example.com", "sk-X1Y2").unwrap();
    assert_eq!(result["user_id"], "u-test");
    assert_eq!(result["spend"], 2.5);
}

#[test]
fn put_warms_sibling_keys_from_response() {
    let dir = TempDir::new().unwrap();
    let cache = test_cache(&dir);
    let data = serde_json::json!({
        "user_id": "u-shared",
        "spend": 1.0,
        "keys": [
            { "key_name": "sk-...A1B2" },
            { "key_name": "sk-...C3D4" }
        ]
    });
    // Call put with the first key only
    cache.put("https://api.example.com", "sk-A1B2", &data);
    // Second key was in the response's keys array — should be a warm hit
    let result = cache.get("https://api.example.com", "sk-C3D4");
    assert!(result.is_some(), "sibling key from response should be cached");
}

#[test]
fn get_returns_none_when_ttl_expired() {
    let dir = TempDir::new().unwrap();
    let mut cache = test_cache(&dir);
    cache.ttl_secs = 1;
    let data = serde_json::json!({ "user_id": "u-old", "spend": 0.0, "keys": [] });
    cache.put("https://api.example.com", "sk-old1", &data);
    // Backdate the entry to simulate expiry
    let entry_path = cache.endpoint_dir("https://api.example.com").join("u-old.json");
    let mut raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&entry_path).unwrap()).unwrap();
    raw["fetched_at"] = serde_json::json!(0u64);
    std::fs::write(&entry_path, serde_json::to_string(&raw).unwrap()).unwrap();
    assert!(cache.get("https://api.example.com", "sk-old1").is_none());
}

#[test]
fn ttl_zero_means_never_expires() {
    let dir = TempDir::new().unwrap();
    let mut cache = test_cache(&dir);
    cache.ttl_secs = 0;
    let data = serde_json::json!({ "user_id": "u-inf", "spend": 0.0, "keys": [] });
    cache.put("https://api.example.com", "sk-inf1", &data);
    // Backdate to epoch
    let entry_path = cache.endpoint_dir("https://api.example.com").join("u-inf.json");
    let mut raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&entry_path).unwrap()).unwrap();
    raw["fetched_at"] = serde_json::json!(0u64);
    std::fs::write(&entry_path, serde_json::to_string(&raw).unwrap()).unwrap();
    // ttl_secs == 0 → no expiry check
    assert!(cache.get("https://api.example.com", "sk-inf1").is_some());
}

#[test]
fn get_returns_none_when_disabled() {
    let dir = TempDir::new().unwrap();
    let mut cache = test_cache(&dir);
    let data = serde_json::json!({ "user_id": "u-dis", "spend": 0.0, "keys": [] });
    // Put with disabled=false so the file actually gets written
    cache.put("https://api.example.com", "sk-dis1", &data);
    cache.disabled = true;
    assert!(cache.get("https://api.example.com", "sk-dis1").is_none());
}

#[test]
fn put_is_noop_when_disabled() {
    let dir = TempDir::new().unwrap();
    let mut cache = test_cache(&dir);
    cache.disabled = true;
    let data = serde_json::json!({ "user_id": "u-skip", "spend": 0.0, "keys": [] });
    cache.put("https://api.example.com", "sk-skip", &data);
    let endpoint_dir = cache.endpoint_dir("https://api.example.com");
    assert!(!endpoint_dir.exists(), "no files should be written when disabled");
}
```

- [ ] **Step 2: Run failing tests**

```bash
cargo test -q 'cache::tests::get_' 'cache::tests::put_' 'cache::tests::ttl_' 2>&1 | head -30
```

Expected: panics with "not yet implemented".

- [ ] **Step 3: Implement get and put**

Replace the `todo!()` stubs in `src/cache.rs`:

```rust
pub fn get(&self, base_url: &str, api_key: &str) -> Option<Value> {
    if self.disabled {
        return None;
    }
    (|| {
        let dir = self.endpoint_dir(base_url);
        let index = load_index(&dir);
        let user_id = index.get(&key_suffix(api_key))?.clone();
        let content =
            std::fs::read_to_string(dir.join(format!("{user_id}.json"))).ok()?;
        let entry: CacheEntry = serde_json::from_str(&content).ok()?;
        if self.ttl_secs > 0 {
            let age = now_secs().saturating_sub(entry.fetched_at);
            if age >= self.ttl_secs {
                return None;
            }
        }
        Some(entry.data)
    })()
}

pub fn put(&self, base_url: &str, api_key: &str, data: &Value) {
    if self.disabled {
        return;
    }
    let _ = (|| -> std::io::Result<()> {
        let dir = self.endpoint_dir(base_url);
        std::fs::create_dir_all(&dir)?;
        let user_id = derive_user_id(api_key, data);
        let entry = CacheEntry { fetched_at: now_secs(), data: data.clone() };
        let json = serde_json::to_string(&entry).map_err(std::io::Error::other)?;
        write_atomic(&dir.join(format!("{user_id}.json")), &json)?;
        let mut index = load_index(&dir);
        index.insert(key_suffix(api_key), user_id.clone());
        if let Some(keys) = data.get("keys").and_then(|v| v.as_array()) {
            for k in keys {
                if let Some(kn) = k.get("key_name").and_then(|v| v.as_str()) {
                    index.insert(key_suffix(kn), user_id.clone());
                }
            }
        }
        save_index(&dir, &index)
    })();
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -q 'cache::tests' 2>&1
```

Expected: all tests in `cache::tests` pass (including the helpers from Task 2).

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/cache.rs
git commit -m "feat(cache): implement Cache::get and Cache::put"
```

---

### Task 4: Cache::clear

**Files:**
- Modify: `src/cache.rs`

- [ ] **Step 1: Write failing tests**

Add to the `tests` block in `src/cache.rs`:

```rust
#[test]
fn clear_removes_all_files_and_returns_count() {
    let dir = TempDir::new().unwrap();
    let cache = test_cache(&dir);
    let data = serde_json::json!({ "user_id": "u-clr", "spend": 0.0, "keys": [] });
    cache.put("https://api.example.com", "sk-clr1", &data);
    cache.put("https://other.example.com", "sk-clr2", &data);
    // Two endpoints → two subdirs, each with index.json + u-clr.json = 4 files
    let count = cache.clear().unwrap();
    assert_eq!(count, 4);
    assert!(
        dir.path().read_dir().unwrap().next().is_none(),
        "base_dir should be empty after clear"
    );
}

#[test]
fn clear_on_empty_dir_returns_zero() {
    let dir = TempDir::new().unwrap();
    let cache = test_cache(&dir);
    assert_eq!(cache.clear().unwrap(), 0);
}
```

- [ ] **Step 2: Run failing tests**

```bash
cargo test -q 'cache::tests::clear' 2>&1
```

Expected: panics with "not yet implemented".

- [ ] **Step 3: Implement clear**

Replace the `todo!()` stub:

```rust
pub fn clear(&self) -> std::io::Result<usize> {
    if !self.base_dir.exists() {
        return Ok(0);
    }
    let mut count = 0;
    for entry in std::fs::read_dir(&self.base_dir)? {
        let path = entry?.path();
        if path.is_dir() {
            for file in std::fs::read_dir(&path)? {
                std::fs::remove_file(file?.path())?;
                count += 1;
            }
            let _ = std::fs::remove_dir(&path);
        }
    }
    Ok(count)
}
```

- [ ] **Step 4: Run all cache tests**

```bash
cargo test -q 'cache::tests' 2>&1
```

Expected: all pass.

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/cache.rs
git commit -m "feat(cache): implement Cache::clear"
```

---

### Task 5: CLI changes, command handler, and stubs

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/main.rs`
- Modify: `src/commands/mod.rs`
- Modify: `src/commands/spend.rs`
- Create: `src/commands/cache.rs`

This task adds CLI shape and enough stubs to keep the project compiling throughout.

- [ ] **Step 1: Update cli.rs**

In `src/cli.rs`, change the `Spend` variant:

```rust
/// Show spend and budget info for the selected profile (LiteLLM only)
Spend {
    /// Profile name (positional; overrides the global --profile flag)
    profile: Option<String>,
    /// Output raw JSON instead of formatted text
    #[arg(long)]
    json: bool,
    /// Always fetch fresh data, bypassing the cache (result is still cached)
    #[arg(long)]
    no_cache: bool,
},
```

Add a new `Cache` variant to `Command` after `Spend`:

```rust
/// Manage the local response cache
Cache {
    #[command(subcommand)]
    action: CacheAction,
},
```

Add a new enum after `ConfigAction`:

```rust
#[derive(Subcommand)]
pub enum CacheAction {
    /// Delete all cached response files
    Clear,
}
```

- [ ] **Step 2: Update main.rs**

In `src/main.rs`, update the `use` import:

```rust
use cli::{CacheAction, Cli, Command};
```

Change the `Command::Spend` arm to pass `no_cache`:

```rust
Command::Spend { profile, json, no_cache } => {
    let effective_profile = profile.or(global_profile);
    commands::spend::run(effective_profile, config_path, json, no_cache).await
}
```

Add a `Command::Cache` arm:

```rust
Command::Cache { action } => commands::cache::run(action),
```

- [ ] **Step 3: Add spend.rs stub parameter**

In `src/commands/spend.rs`, add `_no_cache: bool` to the function signature (full wiring happens in Task 6):

```rust
pub async fn run(
    positional_profile: Option<String>,
    config_path: Option<PathBuf>,
    json: bool,
    _no_cache: bool,
) -> Result<()> {
```

- [ ] **Step 4: Create commands/cache.rs stub and register it**

Create `src/commands/cache.rs`:

```rust
use crate::cache::Cache;
use crate::cli::CacheAction;
use crate::config::CacheConfig;
use color_eyre::Result;

pub fn run(action: CacheAction) -> Result<()> {
    match action {
        CacheAction::Clear => {
            let cache = Cache::from_config(&CacheConfig::default());
            let count = cache
                .clear()
                .map_err(|e| color_eyre::eyre::eyre!("failed to clear cache: {e}"))?;
            if count == 0 {
                println!("Cache already empty.");
            } else {
                println!("Cleared {count} cached file(s).");
            }
            Ok(())
        }
    }
}
```

Add to `src/commands/mod.rs`:

```rust
pub mod cache;
```

- [ ] **Step 5: Verify clean build**

```bash
cargo build 2>&1
```

Expected: no errors.

- [ ] **Step 6: Smoke-test the new subcommand**

```bash
cargo run -- cache clear 2>&1
```

Expected: `Cache already empty.` (or a count if `~/.cache/aix/` has real data).

- [ ] **Step 7: Lint and commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/cli.rs src/main.rs src/commands/mod.rs src/commands/cache.rs src/commands/spend.rs
git commit -m "feat(cache): CLI --no-cache flag, aix cache clear, command stubs"
```

---

### Task 6: Wire cache into spend command

**Files:**
- Modify: `src/commands/spend.rs`

- [ ] **Step 1: Replace spend.rs body with cache-aware implementation**

Replace the entire contents of `src/commands/spend.rs`:

```rust
use crate::cache::Cache;
use crate::client::LiteLlmClient;
use crate::commands::env::resolve_profile;
use crate::config;
use crate::error::AixError;
use color_eyre::Result;
use std::path::PathBuf;

pub async fn run(
    positional_profile: Option<String>,
    config_path: Option<PathBuf>,
    json: bool,
    no_cache: bool,
) -> Result<()> {
    let path = config::find_config_path(config_path.as_deref())?.ok_or(AixError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;
    config::load_env_files(&cfg)?;

    let profile_name = resolve_profile(positional_profile, &cfg)?;
    let profile = cfg
        .profiles
        .get(&profile_name)
        .ok_or_else(|| AixError::ProfileNotFound {
            name: profile_name.clone(),
            available_hint: config::format_available_profiles(&cfg),
        })?;

    let base_url = cfg.endpoint.base_url.resolve()?;
    let api_key = profile.api_key.resolve()?;
    let cache = Cache::from_config(&cfg.cache);

    let data = if !no_cache {
        cache.get(base_url.expose_secret(), api_key.expose_secret())
    } else {
        None
    };

    let data = match data {
        Some(cached) => cached,
        None => {
            let client =
                LiteLlmClient::new(base_url.expose_secret(), api_key.expose_secret());
            let fresh = client.user_info().await?;
            cache.put(base_url.expose_secret(), api_key.expose_secret(), &fresh);
            fresh
        }
    };

    if json {
        let mut out = serde_json::to_string_pretty(&data)?;
        out.push('\n');
        print!("{out}");
    } else {
        print_human(&data, api_key.expose_secret());
    }
    Ok(())
}

fn find_matching_key<'a>(
    keys: &'a [serde_json::Value],
    api_key: &str,
) -> Option<&'a serde_json::Value> {
    let suffix = &api_key[api_key.len().saturating_sub(4)..];
    keys.iter().find(|k| {
        k.get("key_name")
            .and_then(|v| v.as_str())
            .is_some_and(|kn| kn.ends_with(suffix))
    })
}

fn print_human(data: &serde_json::Value, api_key: &str) {
    let empty = vec![];
    let keys = data
        .get("keys")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);

    let (spend, budget) = if let Some(key) = find_matching_key(keys, api_key) {
        let spend = key.get("spend").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let budget = key.get("max_budget").and_then(|v| v.as_f64());
        (spend, budget)
    } else {
        let spend = data.get("spend").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let budget = data.get("max_budget").and_then(|v| v.as_f64());
        (spend, budget)
    };

    if let Some(b) = budget {
        let remaining = b - spend;
        let pct_used = (spend / b * 100.0).clamp(0.0, 100.0);
        let pct_remaining = 100.0 - pct_used;

        const BAR: usize = 40;
        let filled = (pct_used / 100.0 * BAR as f64).round() as usize;
        let bar = format!("[{}{}]", "█".repeat(filled), "░".repeat(BAR - filled));

        println!("${spend:.2} of ${b:.2}  ·  ${remaining:.2} available ({pct_remaining:.0}%)");
        println!("{bar}  {pct_used:.0}% used");
    } else {
        println!("${spend:.2} spent  (no budget set)");
    }
}
```

- [ ] **Step 2: Build and verify**

```bash
cargo build 2>&1
```

Expected: clean build.

- [ ] **Step 3: Lint and commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/commands/spend.rs
git commit -m "feat(cache): wire cache into spend command"
```

---

### Task 7: Integration tests

**Files:**
- Modify: `tests/spend_test.rs`

- [ ] **Step 1: Add AIX_CACHE_DIR isolation to existing tests**

The existing tests write to the real `~/.cache/aix/` because they don't set `AIX_CACHE_DIR`. Mock servers use random ports so they won't poison each other, but they'll litter the user's real cache dir with test data. Add `.env("AIX_CACHE_DIR", cache_dir.path())` to every `Command::cargo_bin("aix")` call in the three existing tests.

In `spend_shows_matching_key_by_suffix`, add a `TempDir` for the cache and set the env var:

```rust
let cache_dir = TempDir::new().unwrap();

Command::cargo_bin("aix")
    .unwrap()
    .env("AIX_CACHE_DIR", cache_dir.path())  // ← add this line
    .args(["--config", config.to_str().unwrap(), "spend", "test"])
    // ... rest unchanged
```

Apply the same change to `spend_falls_back_to_user_totals_when_no_key_match` and `spend_json_returns_full_user_info`.

- [ ] **Step 2: Write the cache integration test**

Add to `tests/spend_test.rs`:

```rust
#[tokio::test]
async fn spend_second_call_uses_cache() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/user/info"))
        .and(header("Authorization", "Bearer sk-testT3S4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "user_id": "u-cached",
            "spend": 3.14,
            "max_budget": 10.0,
            "keys": [{ "key_name": "sk-...T3S4", "spend": 3.14, "max_budget": 10.0 }]
        })))
        .expect(1)  // exactly one real request
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), "sk-testT3S4");

    // First call — hits the server
    Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args(["--config", config.to_str().unwrap(), "spend", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("3.14"));

    // Second call — must come from cache (mock expects exactly 1 total request)
    Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args(["--config", config.to_str().unwrap(), "spend", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("3.14"));

    // wiremock verifies .expect(1) on server drop
}

#[tokio::test]
async fn spend_no_cache_flag_bypasses_read_but_still_writes() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/user/info"))
        .and(header("Authorization", "Bearer sk-testN0C1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "user_id": "u-nocache",
            "spend": 1.0,
            "max_budget": 5.0,
            "keys": [{ "key_name": "sk-...N0C1", "spend": 1.0, "max_budget": 5.0 }]
        })))
        .expect(2)  // --no-cache forces a real request each time
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), "sk-testN0C1");

    for _ in 0..2 {
        Command::cargo_bin("aix")
            .unwrap()
            .env("AIX_CACHE_DIR", cache_dir.path())
            .args(["--config", config.to_str().unwrap(), "spend", "test", "--no-cache"])
            .assert()
            .success();
    }
}
```

- [ ] **Step 3: Run all tests**

```bash
cargo test --all-targets 2>&1
```

Expected: all pass. The `spend_second_call_uses_cache` test will fail if the cache isn't wired in (would see mock expectation violated on drop). The `spend_no_cache_flag_bypasses_read_but_still_writes` test verifies the `--no-cache` path forces two real requests.

- [ ] **Step 4: Lint and commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add tests/spend_test.rs
git commit -m "test(cache): integration tests for cache hit, --no-cache bypass"
```

---

## Self-Review

**Spec coverage check:**

| Spec requirement | Covered by |
|------------------|-----------|
| Two-level cache layout (index + user files) | Tasks 2–3 |
| TTL default 3600, configurable via `[cache]` | Tasks 1, 3 |
| `ttl_secs = 0` means no expiry | Task 3 test `ttl_zero_means_never_expires` |
| `user_id` from response; synthetic fallback | Task 2 `derive_user_id` |
| All key suffixes from response warmed in index | Task 3 `put_warms_sibling_keys_from_response` |
| `disabled = true` skips read and write | Task 3 tests |
| `--no-cache` skips read but still writes | Tasks 5–6, Task 7 test |
| `aix cache clear` subcommand | Tasks 4–5 |
| Cache read/write errors silently swallowed | Implementation design in Tasks 3–4 |
| `aix cache clear` surfaces I/O errors | Task 5 `commands/cache.rs` |
| No new dependencies | `fnv1a_hex` inline in Task 2 |
| `AIX_CACHE_DIR` for test isolation | Tasks 2, 7 |
| Integration test: second call uses cache | Task 7 |

**Placeholder scan:** No TBDs, TODOs, or incomplete steps.

**Type consistency:**
- `Cache::get` → `Option<Value>` — used in spend.rs Task 6 ✓
- `Cache::put` → `()` — used in spend.rs Task 6 ✓
- `Cache::clear` → `std::io::Result<usize>` — used in commands/cache.rs Task 5 ✓
- `Cache::from_config(cfg: &CacheConfig)` — called with `&cfg.cache` in spend.rs and `&CacheConfig::default()` in commands/cache.rs ✓
- `CacheAction::Clear` — matched in commands/cache.rs Task 5, matches cli.rs Task 5 ✓
