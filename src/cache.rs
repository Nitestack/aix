#![allow(dead_code)]

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
        Self {
            base_dir,
            ttl_secs: cfg.ttl_secs,
            disabled: cfg.disabled,
        }
    }

    pub fn get(&self, base_url: &str, api_key: &str) -> Option<Value> {
        if self.disabled {
            return None;
        }
        (|| {
            let dir = self.endpoint_dir(base_url);
            let index = load_index(&dir);
            let user_id = index.get(&key_suffix(api_key))?.clone();
            let content = std::fs::read_to_string(dir.join(format!("{user_id}.json"))).ok()?;
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
            let entry = CacheEntry {
                fetched_at: now_secs(),
                data: data.clone(),
            };
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
        cache.put("https://api.example.com", "sk-A1B2", &data);
        let result = cache.get("https://api.example.com", "sk-C3D4");
        assert!(
            result.is_some(),
            "sibling key from response should be cached"
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
