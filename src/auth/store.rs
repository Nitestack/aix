use crate::error::AixError;
use crate::secrets::SecretString;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;
use zeroize::Zeroizing;

pub(super) const RECORD_VERSION: u8 = 1;

#[derive(Clone, Debug)]
pub(crate) struct AuthStore {
    root: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct HostRecord {
    pub version: u8,
    pub host_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct RegistrationRecord {
    pub version: u8,
    pub profile: String,
    pub client_id: String,
    pub subject: String,
    pub email: Option<String>,
    pub scopes: Vec<String>,
    pub id_token: Option<SecretString>,
    pub access_token: Option<SecretString>,
    pub refresh_token: Option<SecretString>,
    pub expires_at: Option<u64>,
    pub earliest_refresh_at: Option<u64>,
}

impl AuthStore {
    pub(crate) fn from_environment() -> Result<Self, AixError> {
        if let Some(root) = std::env::var_os("AIX_AUTH_DIR") {
            return Ok(Self::new(PathBuf::from(root)));
        }
        let dirs = directories::BaseDirs::new().ok_or_else(|| {
            AixError::AuthStoreIo(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "platform data directory is unavailable",
            ))
        })?;
        Ok(Self::new(dirs.data_local_dir().join("aix").join("auth")))
    }

    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn get_or_create_host_id(&self) -> Result<String, AixError> {
        let _lock = self.lock("host")?;
        let path = self.root.join("host.json");
        if path.exists() {
            let record: HostRecord = self.read_record(&path)?;
            if record.version != RECORD_VERSION || !record.host_id.starts_with("urn:uuid:") {
                return Err(AixError::AuthStoreMalformed);
            }
            return Ok(record.host_id);
        }

        let record = HostRecord {
            version: RECORD_VERSION,
            host_id: format!("urn:uuid:{}", Uuid::new_v4()),
        };
        self.write_record(&path, &record)?;
        Ok(record.host_id)
    }

    pub(crate) async fn get_or_create_host_id_async(&self) -> Result<String, AixError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.get_or_create_host_id())
            .await
            .map_err(|_| {
                AixError::AuthStoreIo(std::io::Error::other("host identity lock task failed"))
            })?
    }

    pub(crate) fn load(&self, profile: &str) -> Result<Option<RegistrationRecord>, AixError> {
        self.ensure_root()?;
        let path = self.registration_path(profile);
        if !path.exists() {
            return Ok(None);
        }
        let record: RegistrationRecord = self.read_record(&path)?;
        if record.version != RECORD_VERSION || record.profile != profile {
            return Err(AixError::AuthStoreMalformed);
        }
        Ok(Some(record))
    }

    pub(crate) fn save(&self, record: &RegistrationRecord) -> Result<(), AixError> {
        if record.version != RECORD_VERSION {
            return Err(AixError::AuthStoreMalformed);
        }
        self.write_record(&self.registration_path(&record.profile), record)
    }

    pub(crate) fn lock_profile(&self, profile: &str) -> Result<StoreLock, AixError> {
        self.lock(&format!("profile-{}", hex(profile.as_bytes())))
    }

    pub(crate) async fn lock_profile_async(&self, profile: &str) -> Result<StoreLock, AixError> {
        let store = self.clone();
        let profile = profile.to_owned();
        tokio::task::spawn_blocking(move || store.lock_profile(&profile))
            .await
            .map_err(|_| {
                AixError::AuthStoreIo(std::io::Error::other("profile credential lock task failed"))
            })?
    }

    fn lock(&self, key: &str) -> Result<StoreLock, AixError> {
        self.ensure_root()?;
        let path = self.root.join(format!(".{key}.lock"));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path).map_err(AixError::AuthStoreIo)?;
        set_owner_only(&file)?;
        file.lock_exclusive().map_err(AixError::AuthStoreIo)?;
        Ok(StoreLock { file })
    }

    fn registration_path(&self, profile: &str) -> PathBuf {
        self.root
            .join(format!("profile-{}.json", hex(profile.as_bytes())))
    }

    fn ensure_root(&self) -> Result<(), AixError> {
        fs::create_dir_all(&self.root).map_err(AixError::AuthStoreIo)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))
                .map_err(AixError::AuthStoreIo)?;
        }
        Ok(())
    }

    fn read_record<T: for<'de> Deserialize<'de>>(&self, path: &Path) -> Result<T, AixError> {
        self.ensure_root()?;
        let file = File::open(path).map_err(AixError::AuthStoreIo)?;
        set_owner_only(&file)?;
        let mut content = Zeroizing::new(String::new());
        let mut reader = file;
        reader
            .read_to_string(&mut content)
            .map_err(AixError::AuthStoreIo)?;
        serde_json::from_str(&content).map_err(|_| AixError::AuthStoreMalformed)
    }

    fn write_record<T: Serialize>(&self, path: &Path, record: &T) -> Result<(), AixError> {
        self.ensure_root()?;
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("record");
        let write_result = (|| {
            let mut temporary = tempfile::Builder::new()
                .prefix(&format!(".{file_name}."))
                .tempfile_in(&self.root)
                .map_err(AixError::AuthStoreIo)?;
            serde_json::to_writer(temporary.as_file_mut(), record)
                .map_err(|_| AixError::AuthStoreMalformed)?;
            temporary
                .as_file_mut()
                .write_all(b"\n")
                .map_err(AixError::AuthStoreIo)?;
            temporary
                .as_file()
                .sync_all()
                .map_err(AixError::AuthStoreIo)?;
            set_owner_only(temporary.as_file())?;
            temporary
                .persist(path)
                .map_err(|error| AixError::AuthStoreIo(error.error))?;
            #[cfg(unix)]
            File::open(&self.root)
                .and_then(|directory| directory.sync_all())
                .map_err(AixError::AuthStoreIo)?;
            Ok(())
        })();
        write_result
    }
}

pub(crate) struct StoreLock {
    file: File,
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn set_owner_only(file: &File) -> Result<(), AixError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(AixError::AuthStoreIo)?;
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;

    fn record(profile: &str) -> RegistrationRecord {
        RegistrationRecord {
            version: RECORD_VERSION,
            profile: profile.into(),
            client_id: format!("{profile}-issued-client"),
            subject: format!("{profile}-verified-subject"),
            email: Some("user@example.test".into()),
            scopes: vec!["chatgpt.tokens.use.direct".into()],
            id_token: Some(SecretString::new("id-token-secret".into())),
            access_token: Some(SecretString::new("access-token-secret".into())),
            refresh_token: Some(SecretString::new("refresh-token-secret".into())),
            expires_at: Some(1_800_000_000),
            earliest_refresh_at: Some(1_700_000_000),
        }
    }

    #[test]
    fn host_id_is_stable_and_registration_records_are_profile_scoped() {
        let dir = TempDir::new().unwrap();
        let store = AuthStore::new(dir.path().to_path_buf());
        let host_id = store.get_or_create_host_id().unwrap();
        assert!(host_id.starts_with("urn:uuid:"));
        assert_eq!(store.get_or_create_host_id().unwrap(), host_id);

        store.save(&record("personal")).unwrap();
        store.save(&record("work")).unwrap();
        assert_eq!(
            store.load("personal").unwrap().unwrap().client_id,
            "personal-issued-client"
        );
        assert_eq!(
            store.load("work").unwrap().unwrap().subject,
            "work-verified-subject"
        );
    }

    #[cfg(unix)]
    #[test]
    fn credential_files_and_auth_directory_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let store = AuthStore::new(dir.path().join("auth"));
        store.save(&record("personal")).unwrap();
        let path = store.registration_path("personal");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&store.root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(fs::read_to_string(path)
            .unwrap()
            .contains("refresh-token-secret"));
    }

    #[test]
    fn secret_tokens_are_redacted_from_debug_output() {
        let rendered = format!("{:?}", record("personal"));
        assert!(!rendered.contains("access-token-secret"));
        assert!(!rendered.contains("refresh-token-secret"));
        assert!(!rendered.contains("id-token-secret"));
    }
}
