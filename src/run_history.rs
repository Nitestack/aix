use crate::error::AixError;
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub const RUN_SCHEMA_VERSION: u8 = 3;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Running,
    Succeeded,
    Failed,
    Interrupted,
    #[serde(rename = "timed_out")]
    TimedOut,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LeaseCleanupStatus {
    Revoked,
    ExpiredOrUnverified,
    RevokeFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunLeaseRecord {
    pub key_alias: String,
    pub budget: f64,
    pub duration: String,
    pub expires_at: Option<String>,
    pub allowed_models: Vec<String>,
    pub spend: Option<f64>,
    pub cleanup_status: LeaseCleanupStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunPolicyRecord {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_budget: Option<f64>,
    pub effective_duration: String,
    pub effective_allowed_models: Vec<String>,
    pub effective_tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunUsageRecord {
    pub request_count: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub input_tokens_total: Option<u64>,
    pub input_tokens_uncached: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub models: Vec<String>,
    pub protocols: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub schema_version: u8,
    pub run_id: Uuid,
    pub name: Option<String>,
    pub workflow: Option<String>,
    pub task_id: Option<String>,
    pub tags: Vec<String>,
    pub profile: String,
    pub logical_tool_name: Option<String>,
    pub executable_name: String,
    pub started_at_unix_ms: u64,
    pub finished_at_unix_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub process_exit_code: Option<i32>,
    pub status: RunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<RunPolicyRecord>,
    #[serde(default)]
    pub lease: Option<RunLeaseRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<RunUsageRecord>,
}

impl RunRecord {
    pub fn now_unix_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
}

pub struct RunStore {
    runs_dir: PathBuf,
}

pub struct RunHandle {
    directory: PathBuf,
    revision: u64,
}

impl RunStore {
    pub fn from_environment() -> Result<Self, AixError> {
        let state_dir = match std::env::var_os("AIX_STATE_DIR") {
            Some(path) if !path.is_empty() => PathBuf::from(path),
            Some(_) => {
                return Err(AixError::RunHistoryIo {
                    path: PathBuf::from("AIX_STATE_DIR"),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "AIX_STATE_DIR must not be empty",
                    ),
                })
            }
            None => default_state_dir().ok_or_else(|| AixError::RunHistoryIo {
                path: PathBuf::from("aix state directory"),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no platform state directory is available; set AIX_STATE_DIR",
                ),
            })?,
        };

        Ok(Self {
            runs_dir: state_dir.join("runs"),
        })
    }

    pub fn begin(&self, record: &RunRecord) -> Result<RunHandle, AixError> {
        let directory = self.runs_dir.join(record.run_id.to_string());
        create_private_directory(&self.runs_dir)?;
        fs::create_dir(&directory).map_err(|source| AixError::RunHistoryIo {
            path: directory.clone(),
            source,
        })?;
        set_private_directory_permissions(&directory).map_err(|source| AixError::RunHistoryIo {
            path: directory.clone(),
            source,
        })?;

        let mut handle = RunHandle {
            directory,
            revision: 0,
        };
        self.write(&mut handle, record)?;
        Ok(handle)
    }

    pub fn write(&self, handle: &mut RunHandle, record: &RunRecord) -> Result<(), AixError> {
        let revision = handle.revision.saturating_add(1);
        let destination = handle.directory.join(format!("{revision:020}.json"));
        let bytes = serde_json::to_vec_pretty(record)?;
        write_new_snapshot(&destination, &bytes).map_err(|source| AixError::RunHistoryIo {
            path: destination,
            source,
        })?;
        handle.revision = revision;
        Ok(())
    }

    pub fn list(&self, limit: usize) -> Result<Vec<RunRecord>, AixError> {
        if !self.runs_dir.exists() {
            return Ok(Vec::new());
        }

        let entries = fs::read_dir(&self.runs_dir).map_err(|source| AixError::RunHistoryIo {
            path: self.runs_dir.clone(),
            source,
        })?;
        let mut records = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| AixError::RunHistoryIo {
                path: self.runs_dir.clone(),
                source,
            })?;
            if entry
                .file_type()
                .map_err(|source| AixError::RunHistoryIo {
                    path: entry.path(),
                    source,
                })?
                .is_dir()
            {
                if let Some(record) = read_latest_record(&entry.path())? {
                    records.push(record);
                }
            }
        }

        records.sort_by(|left, right| {
            right
                .started_at_unix_ms
                .cmp(&left.started_at_unix_ms)
                .then_with(|| right.run_id.cmp(&left.run_id))
        });
        records.truncate(limit);
        Ok(records)
    }

    pub fn get(&self, run_id: &str) -> Result<RunRecord, AixError> {
        let parsed = Uuid::parse_str(run_id).map_err(|_| AixError::RunNotFound {
            run_id: run_id.to_string(),
        })?;
        let directory = self.runs_dir.join(parsed.to_string());
        read_latest_record(&directory)?.ok_or_else(|| AixError::RunNotFound {
            run_id: run_id.to_string(),
        })
    }
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

fn read_latest_record(directory: &Path) -> Result<Option<RunRecord>, AixError> {
    if !directory.exists() {
        return Ok(None);
    }

    let entries = fs::read_dir(directory).map_err(|source| AixError::RunHistoryIo {
        path: directory.to_path_buf(),
        source,
    })?;
    let mut latest: Option<(u64, PathBuf)> = None;
    for entry in entries {
        let entry = entry.map_err(|source| AixError::RunHistoryIo {
            path: directory.to_path_buf(),
            source,
        })?;
        if !entry
            .file_type()
            .map_err(|source| AixError::RunHistoryIo {
                path: entry.path(),
                source,
            })?
            .is_file()
        {
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

    let Some((_, path)) = latest else {
        return Ok(None);
    };
    let content = fs::read(&path).map_err(|source| AixError::RunHistoryIo {
        path: path.clone(),
        source,
    })?;
    let record = serde_json::from_slice(&content).map_err(|source| AixError::RunHistoryIo {
        path,
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
    })?;
    Ok(Some(record))
}

fn write_new_snapshot(destination: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if destination.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "run record revision already exists",
        ));
    }

    let directory = destination
        .parent()
        .expect("snapshot has a parent directory");
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
    fs::create_dir_all(path).map_err(|source| AixError::RunHistoryIo {
        path: path.to_path_buf(),
        source,
    })?;
    set_private_directory_permissions(path).map_err(|source| AixError::RunHistoryIo {
        path: path.to_path_buf(),
        source,
    })
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

    fn record(run_id: &Uuid, started_at_unix_ms: u64) -> RunRecord {
        RunRecord {
            schema_version: RUN_SCHEMA_VERSION,
            run_id: *run_id,
            name: None,
            workflow: None,
            task_id: None,
            tags: Vec::new(),
            profile: "work".to_string(),
            logical_tool_name: None,
            executable_name: "sh".to_string(),
            started_at_unix_ms,
            finished_at_unix_ms: None,
            duration_ms: None,
            process_exit_code: None,
            status: RunStatus::Running,
            policy: None,
            lease: None,
            usage: None,
        }
    }

    #[test]
    fn list_sorts_newest_first_and_applies_limit() {
        let temp = assert_fs::TempDir::new().unwrap();
        let store = RunStore {
            runs_dir: temp.path().join("runs"),
        };
        let ids = [Uuid::new_v4(), Uuid::new_v4()];
        for (id, started) in ids.iter().zip([10, 20]) {
            store.begin(&record(id, started)).unwrap();
        }

        let listed = store.list(1).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].started_at_unix_ms, 20);
    }

    #[test]
    fn failed_snapshot_does_not_hide_the_previous_valid_record() {
        let temp = assert_fs::TempDir::new().unwrap();
        let store = RunStore {
            runs_dir: temp.path().join("runs"),
        };
        let id = Uuid::new_v4();
        let mut handle = store.begin(&record(&id, 10)).unwrap();
        let blocked_revision = handle.directory.join("00000000000000000002.json");
        fs::create_dir(&blocked_revision).unwrap();

        let mut updated = record(&id, 10);
        updated.status = RunStatus::Succeeded;
        assert!(store.write(&mut handle, &updated).is_err());
        assert_eq!(
            store.get(&id.to_string()).unwrap().status,
            RunStatus::Running
        );
    }
}
