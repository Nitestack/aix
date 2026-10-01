use crate::config::CacheConfig;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct Cache {
    base_dir: PathBuf,
    ttl_secs: u64,
    disabled: bool,
}

const CACHE_VERSION: u8 = 1;
const KEY_DATA_SANITIZATION_VERSION: u8 = 1;

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    #[serde(default)]
    version: u8,
    #[serde(default)]
    key_data_version: u8,
    fetched_at: u64,
    data: Value,
    #[serde(default, rename = "key_suffix")]
    key_identity: Option<String>,
}

impl Cache {
    pub fn from_config(cfg: &CacheConfig) -> Self {
        let base_dir = std::env::var("AIX_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                ProjectDirs::from("", "", "aix")
                    .map(|d| d.cache_dir().to_path_buf())
                    .unwrap_or_else(|| {
                        std::env::var("HOME")
                            .map(|h| PathBuf::from(h).join(".cache").join("aix"))
                            .unwrap_or_else(|_| std::env::temp_dir().join("aix-cache"))
                    })
            });
        Self {
            base_dir,
            ttl_secs: cfg.ttl_secs,
            disabled: cfg.disabled,
        }
    }

    /// Returns `(data, fetched_at_unix_secs)` when a fresh-enough entry exists.
    pub fn get(&self, base_url: &str, api_key: &str) -> Option<(Value, u64)> {
        if self.disabled {
            return None;
        }
        (|| {
            let dir = self.endpoint_dir(base_url);
            let index = load_index(&dir);
            let identities = key_identities(api_key);
            let user_id = identities
                .iter()
                .find_map(|identity| index.get(identity))?
                .clone();
            let content = std::fs::read_to_string(dir.join(format!("{user_id}.json"))).ok()?;
            let entry: CacheEntry = serde_json::from_str(&content).ok()?;
            if entry.version != CACHE_VERSION {
                return None;
            }
            if entry.key_data_version < KEY_DATA_SANITIZATION_VERSION
                && data_has_key_entries(&entry.data)
            {
                // Old cached key lists could contain the full value of a short key.
                // Refresh once to store the new opaque key names instead.
                return None;
            }
            // Responses without a keys list are scoped to the key that fetched them.
            // Entries written by older versions have no scope marker, so they miss and
            // refresh instead of reusing a possibly wrong per-key response.
            if data_is_key_scoped(&entry.data)
                && !entry
                    .key_identity
                    .as_deref()
                    .is_some_and(|cached| identities.iter().any(|identity| identity == cached))
            {
                return None;
            }
            if self.ttl_secs > 0 {
                let age = now_secs().saturating_sub(entry.fetched_at);
                if age >= self.ttl_secs {
                    return None;
                }
            }
            Some((entry.data, entry.fetched_at))
        })()
    }

    pub fn put(&self, base_url: &str, api_key: &str, data: &Value) {
        if self.disabled {
            return;
        }
        let _ = (|| -> std::io::Result<()> {
            let dir = self.endpoint_dir(base_url);
            std::fs::create_dir_all(&dir)?;
            let key_scoped = data_is_key_scoped(data);
            let user_id = if key_scoped {
                format!("key:{}", fnv1a_hex(api_key.as_bytes()))
            } else {
                derive_user_id(api_key, data)
            };
            // Load index BEFORE writing data file to narrow the TOCTOU window
            let mut index = load_index(&dir);
            let identity = key_identity(api_key);
            let entry = CacheEntry {
                version: CACHE_VERSION,
                key_data_version: KEY_DATA_SANITIZATION_VERSION,
                fetched_at: now_secs(),
                data: data.clone(),
                key_identity: key_scoped.then_some(identity),
            };
            let json = serde_json::to_string(&entry).map_err(std::io::Error::other)?;
            write_atomic(&dir.join(format!("{user_id}.json")), &json)?;
            for identity in key_identities(api_key) {
                index.insert(identity, user_id.clone());
            }
            if let Some(keys) = data.get("keys").and_then(|v| v.as_array()) {
                for k in keys {
                    if let Some(kn) = k.get("key_name").and_then(|v| v.as_str()) {
                        index.insert(key_identity_from_name(kn), user_id.clone());
                    }
                }
            }
            save_index(&dir, &index)
        })();
    }

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
                // Non-empty dirs fail here; that's acceptable — files inside were already removed above
                let _ = std::fs::remove_dir(&path);
            }
        }
        Ok(count)
    }

    /// Create and remove a uniquely named probe file to verify the cache directory is writable.
    /// Existing cache data is never opened, changed, or deleted.
    pub fn check_writable(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.base_dir)?;

        let probe_path = self.base_dir.join(format!(
            ".aix-doctor-write-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe_path)?;
        let write_result = file.write_all(b"aix doctor writable check\n");
        drop(file);

        let cleanup_result = std::fs::remove_file(&probe_path);
        write_result?;
        cleanup_result
    }

    fn endpoint_dir(&self, base_url: &str) -> PathBuf {
        self.base_dir.join(fnv1a_hex(base_url.as_bytes()))
    }
}

pub(crate) fn key_identity(api_key: &str) -> String {
    let characters = api_key.chars().collect::<Vec<_>>();
    if is_short_key(api_key) {
        return hashed_identity("short", api_key);
    }
    characters[characters.len() - 4..].iter().collect()
}

pub(crate) fn sanitized_key_name(api_key: &str) -> String {
    let masked_suffix = api_key.strip_prefix("sk-...");
    let identity_source = masked_suffix.unwrap_or(api_key);
    let identity = if masked_suffix.is_some() && is_short_key(identity_source) {
        hashed_identity("suffix", identity_source)
    } else {
        key_identity(identity_source)
    };
    if is_short_key(identity_source) {
        format!("sk-short-{identity}")
    } else {
        format!("sk-...{identity}")
    }
}

pub(crate) fn key_identity_matches_name(api_key: &str, key_name: &str) -> bool {
    if let Some(name_identity) = key_name.strip_prefix("sk-short-") {
        if is_short_key(api_key) {
            name_identity == key_identity(api_key)
                || name_identity == hashed_identity("suffix", api_key)
        } else {
            name_identity == hashed_identity("suffix", &key_identity(api_key))
        }
    } else {
        key_name.ends_with(&key_identity(api_key))
    }
}

fn key_identity_from_name(key_name: &str) -> String {
    key_name
        .strip_prefix("sk-short-")
        .map(str::to_owned)
        .unwrap_or_else(|| key_identity(key_name))
}

fn key_identities(api_key: &str) -> Vec<String> {
    let identity = key_identity(api_key);
    if is_short_key(api_key) {
        return vec![identity, hashed_identity("suffix", api_key)];
    }
    vec![identity.clone(), hashed_identity("suffix", &identity)]
}

fn hashed_identity(domain: &str, value: &str) -> String {
    let mut input = Vec::with_capacity(domain.len() + value.len() + 1);
    input.extend_from_slice(domain.as_bytes());
    input.push(b':');
    input.extend_from_slice(value.as_bytes());
    fnv1a_hex(&input)
}

fn is_short_key(api_key: &str) -> bool {
    api_key.chars().count() <= 4
}

fn derive_user_id(api_key: &str, data: &Value) -> String {
    if let Some(uid) = data.get("user_id").and_then(|v| v.as_str()) {
        return uid.to_string();
    }
    format!("key:{}", fnv1a_hex(api_key.as_bytes()))
}

fn data_is_key_scoped(data: &Value) -> bool {
    data.get("keys").and_then(|v| v.as_array()).is_none()
}

fn data_has_key_entries(data: &Value) -> bool {
    data.get("keys")
        .and_then(Value::as_array)
        .is_some_and(|keys| !keys.is_empty())
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
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

pub(crate) fn now_secs() -> u64 {
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
    fn key_identity_uses_last_4_characters() {
        assert_eq!(key_identity("sk-abcdefgh"), "efgh");
    }

    #[test]
    fn short_key_identity_is_hashed() {
        assert_eq!(key_identity("ab"), fnv1a_hex(b"short:ab"));
    }

    #[test]
    fn four_character_key_identity_is_hashed() {
        assert_eq!(key_identity("abcd"), fnv1a_hex(b"short:abcd"));
    }

    #[test]
    fn sanitized_short_key_name_preserves_a_stable_cache_identity() {
        let name = sanitized_key_name("ab");
        assert_eq!(key_identity_from_name(&name), key_identity("ab"));
        assert!(!name.contains("ab"));
    }

    #[test]
    fn masked_short_key_names_are_sanitized_and_match_short_and_long_credentials() {
        let name = sanitized_key_name("sk-...abcd");

        assert_ne!(name, sanitized_key_name("abcd"));
        assert!(!name.contains("abcd"));
        assert!(key_identity_matches_name("sk-user-abcd", &name));
        assert!(key_identity_matches_name("abcd", &name));
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
        assert_eq!(
            derive_user_id("sk-mykey", &data),
            id,
            "must be deterministic"
        );
        assert_ne!(
            derive_user_id("sk-other", &data),
            id,
            "different key → different id"
        );
    }

    #[test]
    fn fnv1a_hex_is_deterministic_and_unique() {
        assert_eq!(fnv1a_hex(b"hello"), fnv1a_hex(b"hello"));
        assert_ne!(fnv1a_hex(b"hello"), fnv1a_hex(b"world"));
        assert_eq!(fnv1a_hex(b"hello").len(), 16);
    }

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
        let (result, fetched_at) = cache.get("https://api.example.com", "sk-X1Y2").unwrap();
        assert_eq!(result["user_id"], "u-test");
        assert_eq!(result["spend"], 2.5);
        assert!(fetched_at > 0);
    }

    #[test]
    fn key_scoped_entries_do_not_share_a_user_file() {
        let dir = TempDir::new().unwrap();
        let cache = test_cache(&dir);
        let first = serde_json::json!({ "user_id": "u-shared", "spend": 1.0 });
        let second = serde_json::json!({ "user_id": "u-shared", "spend": 2.0 });

        cache.put("https://api.example.com", "sk-first1", &first);
        cache.put("https://api.example.com", "sk-second", &second);

        assert_eq!(
            cache.get("https://api.example.com", "sk-first1").unwrap().0["spend"],
            1.0
        );
        assert_eq!(
            cache.get("https://api.example.com", "sk-second").unwrap().0["spend"],
            2.0
        );
    }

    #[test]
    fn legacy_key_scoped_entry_without_marker_is_not_reused() {
        let dir = TempDir::new().unwrap();
        let cache = test_cache(&dir);
        let endpoint_dir = cache.endpoint_dir("https://api.example.com");
        std::fs::create_dir_all(&endpoint_dir).unwrap();
        std::fs::write(
            endpoint_dir.join("index.json"),
            serde_json::json!({ "1111": "shared", "2222": "shared" }).to_string(),
        )
        .unwrap();
        std::fs::write(
            endpoint_dir.join("shared.json"),
            serde_json::json!({
                "fetched_at": now_secs(),
                "data": { "user_id": "u-shared", "spend": 1.0 }
            })
            .to_string(),
        )
        .unwrap();

        assert!(cache
            .get("https://api.example.com", "sk-first1111")
            .is_none());
        assert!(cache
            .get("https://api.example.com", "sk-second2222")
            .is_none());
    }

    #[test]
    fn put_warms_sibling_keys_from_response() {
        let dir = TempDir::new().unwrap();
        let cache = test_cache(&dir);
        let data = serde_json::json!({
            "user_id": "u-shared",
            "spend": 1.0,
            "keys": [
                { "key_name": sanitized_key_name("sk-...A1B2") },
                { "key_name": sanitized_key_name("sk-...C3D4") }
            ]
        });
        cache.put("https://api.example.com", "sk-A1B2", &data);
        assert!(
            cache.get("https://api.example.com", "sk-C3D4").is_some(),
            "sibling key from response should be cached"
        );
    }

    #[test]
    fn put_warms_short_sibling_keys_without_storing_them_raw() {
        let dir = TempDir::new().unwrap();
        let cache = test_cache(&dir);
        let data = serde_json::json!({
            "user_id": "u-short-keys",
            "spend": 1.0,
            "keys": [{ "key_name": sanitized_key_name("xy") }]
        });
        cache.put("https://api.example.com", "sk-source", &data);

        assert!(cache.get("https://api.example.com", "xy").is_some());
        let content = std::fs::read_to_string(
            cache
                .endpoint_dir("https://api.example.com")
                .join("u-short-keys.json"),
        )
        .unwrap();
        assert!(!content.contains("xy"));
    }

    #[test]
    fn put_warms_masked_short_suffix_for_short_key_lookup() {
        let dir = TempDir::new().unwrap();
        let cache = test_cache(&dir);
        let data = serde_json::json!({
            "user_id": "u-masked-short-key",
            "spend": 1.0,
            "keys": [{ "key_name": sanitized_key_name("sk-...abcd") }]
        });
        cache.put("https://api.example.com", "sk-source", &data);

        assert!(cache.get("https://api.example.com", "abcd").is_some());
    }

    #[test]
    fn get_rejects_legacy_aggregate_key_lists_that_may_contain_raw_short_keys() {
        let dir = TempDir::new().unwrap();
        let cache = test_cache(&dir);
        let endpoint_dir = cache.endpoint_dir("https://api.example.com");
        std::fs::create_dir_all(&endpoint_dir).unwrap();
        let current_key = "sk-current-1234";
        let mut index = serde_json::Map::new();
        index.insert(
            key_identity(current_key),
            Value::String("legacy-aggregate".to_string()),
        );
        std::fs::write(
            endpoint_dir.join("index.json"),
            Value::Object(index).to_string(),
        )
        .unwrap();
        std::fs::write(
            endpoint_dir.join("legacy-aggregate.json"),
            serde_json::json!({
                "version": CACHE_VERSION,
                "fetched_at": now_secs(),
                "data": {
                    "user_id": "u-legacy",
                    "keys": [{ "key_name": "sk-...abcd", "spend": 1.0 }]
                }
            })
            .to_string(),
        )
        .unwrap();

        assert!(cache.get("https://api.example.com", current_key).is_none());
    }

    #[test]
    fn get_does_not_reuse_legacy_short_key_indexes() {
        let dir = TempDir::new().unwrap();
        let cache = test_cache(&dir);
        let endpoint_dir = cache.endpoint_dir("https://api.example.com");
        std::fs::create_dir_all(&endpoint_dir).unwrap();
        std::fs::write(
            endpoint_dir.join("index.json"),
            serde_json::json!({ "ab": "legacy-short-key" }).to_string(),
        )
        .unwrap();
        std::fs::write(
            endpoint_dir.join("legacy-short-key.json"),
            serde_json::json!({
                "version": CACHE_VERSION,
                "fetched_at": now_secs(),
                "data": { "spend": 1.0 },
                "key_suffix": "ab"
            })
            .to_string(),
        )
        .unwrap();

        assert!(cache.get("https://api.example.com", "ab").is_none());
    }

    #[test]
    fn get_returns_none_when_ttl_expired() {
        let dir = TempDir::new().unwrap();
        let mut cache = test_cache(&dir);
        cache.ttl_secs = 1;
        let data = serde_json::json!({ "user_id": "u-old", "spend": 0.0, "keys": [] });
        cache.put("https://api.example.com", "sk-old1", &data);
        let entry_path = cache
            .endpoint_dir("https://api.example.com")
            .join("u-old.json");
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
        let entry_path = cache
            .endpoint_dir("https://api.example.com")
            .join("u-inf.json");
        let mut raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&entry_path).unwrap()).unwrap();
        raw["fetched_at"] = serde_json::json!(0u64);
        std::fs::write(&entry_path, serde_json::to_string(&raw).unwrap()).unwrap();
        assert!(cache.get("https://api.example.com", "sk-inf1").is_some());
    }

    #[test]
    fn get_returns_none_when_disabled() {
        let dir = TempDir::new().unwrap();
        let mut cache = test_cache(&dir);
        let data = serde_json::json!({ "user_id": "u-dis", "spend": 0.0, "keys": [] });
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
        assert!(
            !endpoint_dir.exists(),
            "no files should be written when disabled"
        );
    }

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
}
