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

    // get/put/clear tests added in Tasks 3 & 4
    #[allow(dead_code)]
    fn _uses_test_cache(dir: &TempDir) {
        let _ = test_cache(dir);
    }
}
