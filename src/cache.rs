use crate::config::CacheConfig;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct Cache {
    base_dir: PathBuf,
    ttl_secs: u64,
    disabled: bool,
}

const CACHE_VERSION: u8 = 2;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheEntry {
    version: u8,
    fetched_at: u64,
    data: Value,
    key_hash: String,
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
            let identity = key_identity(api_key);
            let user_id = index.get(&identity)?.clone();
            let content = std::fs::read_to_string(dir.join(format!("{user_id}.json"))).ok()?;
            let entry: CacheEntry = serde_json::from_str(&content).ok()?;
            if entry.version != CACHE_VERSION || entry.key_hash != identity {
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
            create_private_dir_all(&self.base_dir)?;
            create_private_dir_all(&dir)?;
            let identity = key_identity(api_key);
            let user_id = format!("key-{identity}");
            // Load index BEFORE writing data file to narrow the TOCTOU window
            let mut index = load_index(&dir);
            let entry = CacheEntry {
                version: CACHE_VERSION,
                fetched_at: now_secs(),
                data: data.clone(),
                key_hash: identity.clone(),
            };
            let json = serde_json::to_string(&entry).map_err(std::io::Error::other)?;
            write_atomic(&dir.join(format!("{user_id}.json")), &json)?;
            index.insert(identity, user_id);
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
        create_private_dir_all(&self.base_dir)?;

        let probe_path = self.base_dir.join(format!(
            ".aix-doctor-write-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let mut file = open_private_file(&probe_path)?;
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
    let digest = Sha256::digest(api_key.as_bytes());
    const HEX: &[u8; 16] = b"0123456789abcdef";
    digest
        .iter()
        .flat_map(|byte| [HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 0x0f)]])
        .map(char::from)
        .collect()
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
    write_atomic_with(path, content, |file, bytes| file.write_all(bytes))
}

fn write_atomic_with(
    path: &Path,
    content: &str,
    write_content: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
) -> io::Result<()> {
    let temporary = path.with_file_name(format!(".aix-cache-{}.tmp", uuid::Uuid::new_v4()));
    let mut created_temp = false;
    let result = (|| {
        let mut file = open_private_file(&temporary)?;
        created_temp = true;
        write_content(&mut file, content.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    if created_temp && result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn open_private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn create_private_dir_all(path: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
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

    const BASE_URL: &str = "https://api.example.com";

    fn test_cache(dir: &TempDir) -> Cache {
        Cache {
            base_dir: dir.path().to_path_buf(),
            ttl_secs: 3600,
            disabled: false,
        }
    }

    fn sample_data(spend: f64) -> Value {
        serde_json::json!({
            "key_spent_usd": spend,
            "remaining_usd": null,
            "limited_by": null,
            "layers": [],
            "expires_at": null
        })
    }

    fn cache_file(cache: &Cache, base_url: &str, api_key: &str) -> PathBuf {
        let dir = cache.endpoint_dir(base_url);
        let user_id = load_index(&dir)
            .get(&key_identity(api_key))
            .unwrap()
            .clone();
        dir.join(format!("{user_id}.json"))
    }

    #[test]
    fn failed_atomic_write_preserves_the_previous_cache_entry() {
        let dir = TempDir::new().unwrap();
        let cache = test_cache(&dir);
        let api_key = "sk-preserve-entry";
        let previous = sample_data(12.5);
        cache.put(BASE_URL, api_key, &previous);

        let result = write_atomic_with(
            &cache_file(&cache, BASE_URL, api_key),
            "partial replacement",
            |file, _| {
                file.write_all(b"partial")?;
                Err(std::io::Error::other("simulated cache write failure"))
            },
        );

        assert!(result.is_err());
        assert_eq!(cache.get(BASE_URL, api_key).unwrap().0, previous);
    }

    #[cfg(unix)]
    #[test]
    fn cache_files_and_directories_have_restrictive_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let cache = test_cache(&dir);
        cache.put(BASE_URL, "sk-private-cache", &sample_data(1.0));

        let endpoint_dir = cache.endpoint_dir(BASE_URL);
        assert_eq!(
            std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&endpoint_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        for entry in std::fs::read_dir(endpoint_dir).unwrap() {
            assert_eq!(
                entry.unwrap().metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn key_identity_hashes_the_full_key_without_persisting_a_suffix() {
        let first = "sk-first-1234";
        let second = "sk-second-1234";
        let first_identity = key_identity(first);
        let second_identity = key_identity(second);

        assert_eq!(first_identity.len(), 64);
        assert_ne!(first_identity, second_identity);
        assert!(!first_identity.contains("1234"));
        assert!(!first_identity.contains(first));
        assert!(!key_identity("abcd").contains("abcd"));
    }

    #[test]
    fn put_then_get_returns_data_and_fetch_time() {
        let dir = TempDir::new().unwrap();
        let cache = test_cache(&dir);
        let data = sample_data(2.5);

        cache.put(BASE_URL, "sk-X1Y2", &data);
        let (result, fetched_at) = cache.get(BASE_URL, "sk-X1Y2").unwrap();
        assert_eq!(result, data);
        assert!(fetched_at > 0);
    }

    #[test]
    fn cache_filenames_use_only_portable_characters() {
        let dir = TempDir::new().unwrap();
        let cache = test_cache(&dir);
        let api_key = "sk-portable-cache";
        let data = sample_data(2.5);
        cache.put(BASE_URL, api_key, &data);

        assert_eq!(cache.get(BASE_URL, api_key).unwrap().0, data);
        for entry in std::fs::read_dir(cache.endpoint_dir(BASE_URL)).unwrap() {
            let name = entry.unwrap().file_name();
            let name = name.to_str().unwrap();
            assert!(
                name.bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte)),
                "cache filename is not portable: {name}"
            );
        }
    }

    #[test]
    fn keys_with_the_same_suffix_never_share_cached_budget_data() {
        let dir = TempDir::new().unwrap();
        let cache = test_cache(&dir);
        let first_key = "sk-first-1111";
        let second_key = "sk-second-1111";
        let first = sample_data(1.0);
        let second = sample_data(2.0);

        cache.put(BASE_URL, first_key, &first);
        cache.put(BASE_URL, second_key, &second);

        assert_eq!(cache.get(BASE_URL, first_key).unwrap().0, first);
        assert_eq!(cache.get(BASE_URL, second_key).unwrap().0, second);
    }

    #[test]
    fn cache_files_do_not_persist_raw_key_or_upstream_key_metadata() {
        let dir = TempDir::new().unwrap();
        let cache = test_cache(&dir);
        let api_key = "sk-private-credential-6789";
        cache.put(BASE_URL, api_key, &sample_data(1.0));

        for entry in std::fs::read_dir(cache.endpoint_dir(BASE_URL)).unwrap() {
            let content = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            for private in [api_key, "6789", "key_id", "key_name", "metadata"] {
                assert!(!content.contains(private), "persisted {private}: {content}");
            }
        }
    }

    #[test]
    fn legacy_cache_versions_and_suffix_indexes_are_not_reused() {
        let dir = TempDir::new().unwrap();
        let cache = test_cache(&dir);
        let api_key = "sk-legacy-cache-1234";
        let endpoint_dir = cache.endpoint_dir(BASE_URL);
        std::fs::create_dir_all(&endpoint_dir).unwrap();
        let mut index = serde_json::Map::new();
        index.insert("1234".to_string(), serde_json::json!("legacy"));
        index.insert(key_identity(api_key), serde_json::json!("legacy"));
        std::fs::write(
            endpoint_dir.join("index.json"),
            Value::Object(index).to_string(),
        )
        .unwrap();
        std::fs::write(
            endpoint_dir.join("legacy.json"),
            serde_json::json!({
                "version": 1,
                "fetched_at": now_secs(),
                "data": { "spend": 1.0, "max_budget": 5.0 },
                "key_suffix": "1234"
            })
            .to_string(),
        )
        .unwrap();

        assert!(cache.get(BASE_URL, api_key).is_none());
    }

    #[test]
    fn cache_rejects_entries_scoped_to_a_different_key() {
        let dir = TempDir::new().unwrap();
        let cache = test_cache(&dir);
        let first_key = "sk-first-1234";
        let second_key = "sk-second-1234";
        cache.put(BASE_URL, first_key, &sample_data(1.0));

        let endpoint_dir = cache.endpoint_dir(BASE_URL);
        let mut index = load_index(&endpoint_dir);
        index.insert(
            key_identity(second_key),
            index[&key_identity(first_key)].clone(),
        );
        save_index(&endpoint_dir, &index).unwrap();

        assert!(cache.get(BASE_URL, second_key).is_none());
    }

    #[test]
    fn get_returns_none_when_ttl_expired() {
        let dir = TempDir::new().unwrap();
        let mut cache = test_cache(&dir);
        cache.ttl_secs = 1;
        let api_key = "sk-old-key";
        cache.put(BASE_URL, api_key, &sample_data(0.0));
        let entry_path = cache_file(&cache, BASE_URL, api_key);
        let mut raw: Value =
            serde_json::from_str(&std::fs::read_to_string(&entry_path).unwrap()).unwrap();
        raw["fetched_at"] = serde_json::json!(0u64);
        std::fs::write(&entry_path, serde_json::to_string(&raw).unwrap()).unwrap();

        assert!(cache.get(BASE_URL, api_key).is_none());
    }

    #[test]
    fn ttl_zero_means_never_expires() {
        let dir = TempDir::new().unwrap();
        let mut cache = test_cache(&dir);
        cache.ttl_secs = 0;
        let api_key = "sk-infinite-ttl";
        cache.put(BASE_URL, api_key, &sample_data(0.0));
        let entry_path = cache_file(&cache, BASE_URL, api_key);
        let mut raw: Value =
            serde_json::from_str(&std::fs::read_to_string(&entry_path).unwrap()).unwrap();
        raw["fetched_at"] = serde_json::json!(0u64);
        std::fs::write(&entry_path, serde_json::to_string(&raw).unwrap()).unwrap();

        assert!(cache.get(BASE_URL, api_key).is_some());
    }

    #[test]
    fn get_returns_none_when_disabled() {
        let dir = TempDir::new().unwrap();
        let mut cache = test_cache(&dir);
        let api_key = "sk-disabled-cache";
        cache.put(BASE_URL, api_key, &sample_data(0.0));
        cache.disabled = true;

        assert!(cache.get(BASE_URL, api_key).is_none());
    }

    #[test]
    fn put_is_noop_when_disabled() {
        let dir = TempDir::new().unwrap();
        let mut cache = test_cache(&dir);
        cache.disabled = true;
        cache.put(BASE_URL, "sk-disabled-cache", &sample_data(0.0));

        assert!(!cache.endpoint_dir(BASE_URL).exists());
    }

    #[test]
    fn clear_removes_all_files_and_returns_count() {
        let dir = TempDir::new().unwrap();
        let cache = test_cache(&dir);
        cache.put(BASE_URL, "sk-clear-one", &sample_data(0.0));
        cache.put(
            "https://other.example.com",
            "sk-clear-two",
            &sample_data(0.0),
        );

        assert_eq!(cache.clear().unwrap(), 4);
        assert!(dir.path().read_dir().unwrap().next().is_none());
    }

    #[test]
    fn clear_on_empty_dir_returns_zero() {
        let dir = TempDir::new().unwrap();
        let cache = test_cache(&dir);
        assert_eq!(cache.clear().unwrap(), 0);
    }
}
