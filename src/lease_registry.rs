use crate::error::AixError;
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

pub const LEASE_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LeaseStatus {
    Active,
    Revoked,
    ExpiredOrUnknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseRecord {
    pub schema_version: u8,
    pub lease_id: String,
    pub key_alias: String,
    pub profile: String,
    pub budget: f64,
    pub duration: String,
    pub expires_at: Option<String>,
    pub allowed_models: Vec<String>,
    pub tags: Vec<String>,
    pub created_at: String,
    pub status: LeaseStatus,
}

impl LeaseRecord {
    pub fn display_status(&self, now_unix_ms: u64) -> LeaseStatus {
        if self.status != LeaseStatus::Active {
            return self.status;
        }

        match self.expires_at.as_deref().and_then(expiry_unix_ms) {
            Some(expires_at) if expires_at > now_unix_ms => LeaseStatus::Active,
            Some(_) | None => LeaseStatus::ExpiredOrUnknown,
        }
    }

    pub fn for_display(&self, now_unix_ms: u64) -> Self {
        let mut record = self.clone();
        record.status = self.display_status(now_unix_ms);
        record
    }
}

pub struct LeaseStore {
    leases_dir: PathBuf,
}

impl LeaseStore {
    pub fn from_environment() -> Result<Self, AixError> {
        let state_dir = match std::env::var_os("AIX_STATE_DIR") {
            Some(path) if !path.is_empty() => PathBuf::from(path),
            Some(_) => {
                return Err(registry_io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "AIX_STATE_DIR must not be empty",
                )))
            }
            None => default_state_dir().ok_or_else(|| {
                registry_io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no platform state directory is available; set AIX_STATE_DIR",
                ))
            })?,
        };

        Ok(Self {
            leases_dir: state_dir.join("leases"),
        })
    }

    pub fn insert(&self, record: &LeaseRecord) -> Result<(), AixError> {
        create_private_directory(&self.leases_dir)?;
        let directory = self.lease_directory(&record.lease_id)?;
        fs::create_dir(&directory).map_err(registry_io)?;
        if let Err(source) = set_private_directory_permissions(&directory) {
            let _ = fs::remove_dir(&directory);
            return Err(registry_io(source));
        }

        if let Err(error) = self.write_revision(&directory, 1, record) {
            let _ = fs::remove_dir_all(&directory);
            return Err(error);
        }
        Ok(())
    }

    pub fn save(&self, record: &LeaseRecord) -> Result<(), AixError> {
        let directory = self.lease_directory(&record.lease_id)?;
        let (latest_revision, _) =
            read_latest_record(&directory)?.ok_or(AixError::LeaseNotFound)?;
        self.write_revision(&directory, latest_revision.saturating_add(1), record)
    }

    pub fn list(&self) -> Result<Vec<LeaseRecord>, AixError> {
        if !self.leases_dir.exists() {
            return Ok(Vec::new());
        }

        let entries = fs::read_dir(&self.leases_dir).map_err(registry_io)?;
        let mut records = Vec::new();
        for entry in entries {
            let entry = entry.map_err(registry_io)?;
            if entry.file_type().map_err(registry_io)?.is_dir() {
                if let Some((_, record)) = read_latest_record(&entry.path())? {
                    records.push(record);
                }
            }
        }

        records.sort_by(|left, right| {
            created_at_unix_ms(&right.created_at)
                .cmp(&created_at_unix_ms(&left.created_at))
                .then_with(|| right.lease_id.cmp(&left.lease_id))
        });
        Ok(records)
    }

    pub fn get(&self, lease_id: &str) -> Result<LeaseRecord, AixError> {
        let directory = self.lease_directory(lease_id)?;
        read_latest_record(&directory)?
            .map(|(_, record)| record)
            .ok_or(AixError::LeaseNotFound)
    }

    fn lease_directory(&self, lease_id: &str) -> Result<PathBuf, AixError> {
        let parsed = Uuid::parse_str(lease_id).map_err(|_| AixError::LeaseNotFound)?;
        Ok(self.leases_dir.join(parsed.to_string()))
    }

    fn write_revision(
        &self,
        directory: &Path,
        revision: u64,
        record: &LeaseRecord,
    ) -> Result<(), AixError> {
        let destination = directory.join(format!("{revision:020}.json"));
        let bytes =
            serde_json::to_vec_pretty(record).map_err(AixError::LeaseRegistrySerialization)?;
        write_new_snapshot(&destination, &bytes).map_err(registry_io)
    }
}

pub fn created_at_now() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown".to_string())
}

pub fn now_unix_ms() -> u64 {
    OffsetDateTime::now_utc()
        .unix_timestamp_nanos()
        .div_euclid(1_000_000)
        .try_into()
        .unwrap_or(u64::MAX)
}

fn expiry_unix_ms(value: &str) -> Option<u64> {
    if let Ok(timestamp) = value.parse::<i128>() {
        let millis = if timestamp >= 100_000_000_000 || timestamp <= -100_000_000_000 {
            timestamp
        } else {
            timestamp.checked_mul(1_000)?
        };
        return u64::try_from(millis).ok();
    }

    let timestamp = chrono::DateTime::parse_from_rfc3339(value).ok()?;
    u64::try_from(timestamp.timestamp_millis()).ok()
}

fn created_at_unix_ms(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.timestamp_millis())
}

fn registry_io(source: std::io::Error) -> AixError {
    AixError::LeaseRegistryIo(source)
}

fn default_state_dir() -> Option<PathBuf> {
    let base_dirs = BaseDirs::new()?;
    Some(
        base_dirs
            .state_dir()
            .unwrap_or_else(|| base_dirs.data_local_dir())
            .join("aix"),
    )
}

fn read_latest_record(directory: &Path) -> Result<Option<(u64, LeaseRecord)>, AixError> {
    if !directory.exists() {
        return Ok(None);
    }

    let entries = fs::read_dir(directory).map_err(registry_io)?;
    let mut latest: Option<(u64, PathBuf)> = None;
    for entry in entries {
        let entry = entry.map_err(registry_io)?;
        if !entry.file_type().map_err(registry_io)?.is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(revision) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<u64>().ok())
        else {
            continue;
        };
        if latest
            .as_ref()
            .is_none_or(|(current, _)| revision > *current)
        {
            latest = Some((revision, path));
        }
    }

    let Some((revision, path)) = latest else {
        return Ok(None);
    };
    let content = fs::read(&path).map_err(registry_io)?;
    let record: LeaseRecord = serde_json::from_slice(&content).map_err(|source| {
        registry_io(std::io::Error::new(std::io::ErrorKind::InvalidData, source))
    })?;
    if record.schema_version != LEASE_SCHEMA_VERSION {
        return Err(registry_io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unsupported lease registry schema version",
        )));
    }
    Ok(Some((revision, record)))
}

fn write_new_snapshot(destination: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let directory = destination
        .parent()
        .expect("lease registry snapshot has a parent directory");
    let temporary = directory.join(format!(".{}.tmp", Uuid::new_v4()));
    let mut file = new_private_file(&temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn create_private_directory(path: &Path) -> Result<(), AixError> {
    fs::create_dir_all(path).map_err(registry_io)?;
    set_private_directory_permissions(path).map_err(registry_io)
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn new_private_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(created_at: &str) -> LeaseRecord {
        LeaseRecord {
            schema_version: LEASE_SCHEMA_VERSION,
            lease_id: Uuid::new_v4().to_string(),
            key_alias: format!("aix-lease-{}", Uuid::new_v4()),
            profile: "work".to_string(),
            budget: 1.0,
            duration: "2h".to_string(),
            expires_at: None,
            allowed_models: Vec::new(),
            tags: Vec::new(),
            created_at: created_at.to_string(),
            status: LeaseStatus::Active,
        }
    }

    #[test]
    fn list_sorts_newest_first_by_timestamp() {
        let temporary = assert_fs::TempDir::new().unwrap();
        let store = LeaseStore {
            leases_dir: temporary.path().join("leases"),
        };
        store.insert(&record("2026-10-02T12:00:00Z")).unwrap();
        store.insert(&record("2026-10-02T12:00:00.001Z")).unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].created_at, "2026-10-02T12:00:00.001Z");
        assert_eq!(listed[1].created_at, "2026-10-02T12:00:00Z");
    }

    #[test]
    fn active_lease_without_a_parseable_expiry_is_displayed_as_unknown() {
        let mut lease = record("2026-10-02T12:00:00Z");
        lease.expires_at = Some("2030-01-02T03:04:05Z".to_string());
        assert_eq!(lease.display_status(0), LeaseStatus::Active);

        lease.expires_at = None;
        assert_eq!(lease.display_status(0), LeaseStatus::ExpiredOrUnknown);

        lease.expires_at = Some("not-a-timestamp".to_string());
        assert_eq!(lease.display_status(0), LeaseStatus::ExpiredOrUnknown);
    }
}
