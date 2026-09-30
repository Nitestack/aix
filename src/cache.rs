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

const CACHE_VERSION: u8 = 1;

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    #[serde(default)]
    version: u8,
    fetched_at: u64,
    data: Value,
    #[serde(default)]
    key_suffix: Option<String>,
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
            let suffix = key_suffix(api_key);
            let legacy_suffix = legacy_key_suffix(api_key);
            let user_id = index
                .get(&suffix)
                .or_else(|| {
                    legacy_suffix
                        .as_deref()
                        .filter(|legacy| *legacy != suffix)
                        .and_then(|legacy| index.get(legacy))
                })?
                .clone();
            let content = std::fs::read_to_string(dir.join(format!("{user_id}.json"))).ok()?;
            let entry: CacheEntry = serde_json::from_str(&content).ok()?;
            if entry.version != CACHE_VERSION {
                return None;
            }
            let suffix = key_suffix(api_key);
            // Responses without a keys list are scoped to the key that fetched them.
            // Entries written by older versions have no scope marker, so they miss and
            // refresh instead of reusing a possibly wrong per-key response.
            let entry_matches_key = entry.key_suffix.as_deref() == Some(suffix.as_str())
                || legacy_suffix
                    .as_deref()
                    .is_some_and(|legacy| entry.key_suffix.as_deref() == Some(legacy));
            if data_is_key_scoped(&entry.data) && !entry_matches_key {
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
            let entry = CacheEntry {
                version: CACHE_VERSION,
                fetched_at: now_secs(),
                data: data.clone(),
                key_suffix: key_scoped.then(|| key_suffix(api_key)),
            };
            let json = serde_json::to_string(&entry).map_err(std::io::Error::other)?;
            write_atomic(&dir.join(format!("{user_id}.json")), &json)?;
            index.insert(key_suffix(api_key), user_id.clone());
            if let Some(keys) = data.get("keys").and_then(|v| v.as_array()) {
                for k in keys {
                    if let Some(kn) = k.get("key_name").and_then(|v| v.as_str()) {
                        index.insert(key_suffix_from_name(kn), user_id.clone());
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

    fn endpoint_dir(&self, base_url: &str) -> PathBuf {
        self.base_dir.join(fnv1a_hex(base_url.as_bytes()))
    }
}

pub(crate) fn key_suffix(api_key: &str) -> String {
    let characters = api_key.chars().collect::<Vec<_>>();
    if characters.len() <= 4 {
        return fnv1a_hex(api_key.as_bytes());
    }
    characters[characters.len() - 4..].iter().collect()
}

pub(crate) fn short_key_name(api_key: &str) -> String {
    format!("sk-short-{}", fnv1a_hex(api_key.as_bytes()))
}

fn key_suffix_from_name(key_name: &str) -> String {
    key_name
        .strip_prefix("sk-short-")
        .map(str::to_owned)
        .unwrap_or_else(|| key_suffix(key_name))
}

fn legacy_key_suffix(api_key: &str) -> Option<String> {
    let start = api_key.len().saturating_sub(4);
    api_key
        .is_char_boundary(start)
        .then(|| api_key[start..].to_string())
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
    fn key_suffix_last_4() {
        assert_eq!(key_suffix("sk-abcdefgh"), "efgh");
    }

    #[test]
    fn key_suffix_shorter_than_4() {
        assert_eq!(key_suffix("ab"), fnv1a_hex(b"ab"));
    }

    #[test]
    fn key_suffix_exactly_4() {
        assert_eq!(key_suffix("abcd"), fnv1a_hex(b"abcd"));
    }

    #[test]
    fn short_key_name_preserves_an_opaque_cache_identity() {
        let name = short_key_name("ab");
        assert_eq!(key_suffix_from_name(&name), key_suffix("ab"));
        assert!(!name.contains("ab"));
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
                { "key_name": "sk-...A1B2" },
                { "key_name": "sk-...C3D4" }
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
            "keys": [{ "key_name": short_key_name("xy") }]
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
    fn get_accepts_existing_cache_indexes_for_short_keys() {
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

        assert_eq!(
            cache.get("https://api.example.com", "ab").unwrap().0["spend"],
            1.0
        );
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
