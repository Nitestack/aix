use crate::commands::env;
use crate::commands::run_lease;
use crate::commands::ProfileSelection;
use crate::config;
use crate::error::AixError;
use crate::gateway::LiteLlmAdminClient;
use crate::lease_registry::{self, LeaseRecord, LeaseStatus, LeaseStore, LEASE_SCHEMA_VERSION};
use crate::output;
use crate::secrets::SecretString;
use color_eyre::Result;
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;
use zeroize::Zeroizing;

pub(crate) struct CreateOptions {
    pub(crate) selection: ProfileSelection,
    pub(crate) timeout: Duration,
    pub(crate) config_path: Option<PathBuf>,
    pub(crate) budget: f64,
    pub(crate) duration: Option<String>,
    pub(crate) requested_models: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) output: PathBuf,
}

struct ResolvedLeaseParent {
    profile_name: String,
    api_key: SecretString,
    base_url: SecretString,
    allowed_models: Vec<String>,
}

pub(crate) async fn create(options: CreateOptions) -> Result<()> {
    let CreateOptions {
        selection,
        timeout,
        config_path,
        budget,
        duration,
        requested_models,
        tags,
        output: output_path,
    } = options;

    if tags.iter().any(|tag| tag.trim().is_empty()) {
        return Err(AixError::EmptyLeaseTag.into());
    }
    let (budget, duration) =
        run_lease::validate_options(true, Some(budget), duration, &requested_models, false)?;
    let budget = budget.expect("lease budget was validated");
    let duration = duration.expect("lease duration was defaulted and validated");

    validate_output_path(&output_path)?;
    let store = LeaseStore::from_environment()?;
    let parent = resolve_parent(selection, config_path, &requested_models)?;
    let parent_key = parent.api_key.expose_secret();
    if !parent_key.is_empty()
        && (output_path.to_string_lossy().contains(parent_key)
            || parent.profile_name.contains(parent_key)
            || parent.base_url.expose_secret().contains(parent_key)
            || parent
                .allowed_models
                .iter()
                .any(|model| model.contains(parent_key))
            || tags.iter().any(|tag| tag.contains(parent_key)))
    {
        return Err(AixError::LeaseInputContainsCredential.into());
    }

    let lease_id = Uuid::new_v4().to_string();
    let key_alias = format!("aix-lease-{lease_id}");
    let mut metadata_tags = Vec::with_capacity(tags.len() + 1);
    metadata_tags.push(format!("aix:lease:{lease_id}"));
    metadata_tags.extend(tags);
    let client = LiteLlmAdminClient::with_timeout(
        parent.base_url.expose_secret(),
        parent.api_key.expose_secret(),
        timeout,
    );
    let generated = client
        .generate_virtual_key(
            budget,
            &duration,
            &key_alias,
            &parent.allowed_models,
            &metadata_tags,
        )
        .await?;

    if !parent_key.is_empty() && generated.key.expose_secret().contains(parent_key) {
        return Err(AixError::LeaseKeyContainsParentCredential.into());
    }
    let lease_key = generated.key.expose_secret();
    if !lease_key.is_empty()
        && (lease_id.contains(lease_key)
            || key_alias.contains(lease_key)
            || parent.profile_name.contains(lease_key)
            || parent.base_url.expose_secret().contains(lease_key)
            || duration.contains(lease_key)
            || parent
                .allowed_models
                .iter()
                .any(|model| model.contains(lease_key))
            || metadata_tags.iter().any(|tag| tag.contains(lease_key)))
    {
        warn_if_revoke_failed(&client, &generated.key, &key_alias).await;
        return Err(AixError::LeaseMetadataContainsLeasedCredential.into());
    }
    let expires_at = generated
        .expires_at
        .filter(|expiry| parent_key.is_empty() || !expiry.contains(parent_key));
    let mut record = LeaseRecord {
        schema_version: LEASE_SCHEMA_VERSION,
        lease_id: lease_id.clone(),
        key_alias: key_alias.clone(),
        profile: parent.profile_name.clone(),
        budget,
        duration,
        expires_at: expires_at.clone(),
        allowed_models: parent.allowed_models.clone(),
        tags: metadata_tags,
        created_at: lease_registry::created_at_now(),
        status: LeaseStatus::Active,
    };

    let openai_base_url = SecretString::new(env::append_v1(parent.base_url.expose_secret()));
    let export = LeaseExport {
        schema_version: LEASE_SCHEMA_VERSION,
        lease_id: &lease_id,
        expires_at: expires_at.as_deref(),
        env: LeaseEnvironment {
            profile: &parent.profile_name,
            api_key: &generated.key,
            base_url: &parent.base_url,
            openai_base_url: &openai_base_url,
        },
    };
    let bytes = match serde_json::to_vec_pretty(&export) {
        Ok(bytes) => Zeroizing::new(bytes),
        Err(error) => {
            warn_if_revoke_failed(&client, &generated.key, &key_alias).await;
            return Err(error.into());
        }
    };

    if let Err(error) = store.insert(&record) {
        warn_if_revoke_failed(&client, &generated.key, &key_alias).await;
        return Err(error.into());
    }

    if let Err(source) = write_secret_file(&output_path, &bytes) {
        record.status = cleanup_after_export_failure(&client, &generated.key, &key_alias).await;
        if let Err(registry_error) = store.save(&record) {
            eprintln!("warning: failed to update local lease status after export failure: {registry_error}");
        }
        return Err(AixError::LeaseOutputIo(source).into());
    }

    println!(
        "Created lease {lease_id} (expires {}); secret credential file created.",
        expires_at.as_deref().unwrap_or("not returned by gateway")
    );
    Ok(())
}

pub(crate) fn list(json: bool) -> Result<()> {
    let records = LeaseStore::from_environment()?
        .list()?
        .into_iter()
        .map(|record| record.for_display(lease_registry::now_unix_ms()))
        .collect::<Vec<_>>();
    if json {
        output::print_json("leases", records)?;
    } else {
        print_list(&records);
    }
    Ok(())
}

pub(crate) fn show(lease_id: &str, json: bool) -> Result<()> {
    let record = LeaseStore::from_environment()?
        .get(lease_id)?
        .for_display(lease_registry::now_unix_ms());
    if json {
        output::print_json("lease show", record)?;
    } else {
        print_record(&record);
    }
    Ok(())
}

pub(crate) async fn revoke(
    lease_id: &str,
    config_path: Option<PathBuf>,
    timeout: Duration,
) -> Result<()> {
    let store = LeaseStore::from_environment()?;
    let mut record = store.get(lease_id)?;
    if record.status == LeaseStatus::Revoked {
        println!("Lease {} is already marked revoked.", record.lease_id);
        println!(
            "If an exported credential file exists, remove it manually; aix leaves it untouched."
        );
        return Ok(());
    }

    let client = admin_client_for_profile(&record.profile, config_path, timeout)?;
    client.delete_virtual_key_alias(&record.key_alias).await?;
    record.status = LeaseStatus::Revoked;
    store.save(&record)?;
    println!("Revoked lease {}.", record.lease_id);
    println!("If an exported credential file exists, remove it manually; aix leaves it untouched.");
    Ok(())
}

fn resolve_parent(
    selection: ProfileSelection,
    config_path: Option<PathBuf>,
    requested_models: &[String],
) -> Result<ResolvedLeaseParent, AixError> {
    let cfg = load_config(config_path.as_deref())?;
    if matches!(
        cfg.endpoint.gateway.as_ref(),
        Some(config::Gateway::Custom(_))
    ) {
        return Err(AixError::LeaseNotLiteLlm);
    }
    let profile_name = env::resolve_profile(selection, &cfg)?;
    let profile = cfg
        .profiles
        .get(&profile_name)
        .ok_or(AixError::LeaseParentProfileNotFound)?;
    let allowed_models = requested_models
        .iter()
        .map(|model| config::resolve_model(Some(model), &cfg, profile))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ResolvedLeaseParent {
        profile_name,
        api_key: profile.api_key.resolve()?,
        base_url: config::resolve_base_url(profile, &cfg.endpoint)?,
        allowed_models,
    })
}

fn admin_client_for_profile(
    profile_name: &str,
    config_path: Option<PathBuf>,
    timeout: Duration,
) -> Result<LiteLlmAdminClient, AixError> {
    let cfg = load_config(config_path.as_deref())?;
    if matches!(
        cfg.endpoint.gateway.as_ref(),
        Some(config::Gateway::Custom(_))
    ) {
        return Err(AixError::LeaseNotLiteLlm);
    }
    let profile = cfg
        .profiles
        .get(profile_name)
        .ok_or(AixError::LeaseParentProfileNotFound)?;
    let api_key = profile.api_key.resolve()?;
    let base_url = config::resolve_base_url(profile, &cfg.endpoint)?;
    Ok(LiteLlmAdminClient::with_timeout(
        base_url.expose_secret(),
        api_key.expose_secret(),
        timeout,
    ))
}

fn load_config(config_path: Option<&Path>) -> Result<config::Config, AixError> {
    let path = config::find_config_path(config_path)?.ok_or(AixError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;
    config::load_env_files(&cfg)?;
    Ok(cfg)
}

fn validate_output_path(path: &Path) -> Result<(), AixError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if path.file_name().is_none() || !parent.is_dir() {
        return Err(AixError::LeaseOutputIo(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "output parent directory must exist and the path must name a file",
        )));
    }
    match fs::symlink_metadata(path) {
        Ok(_) => Err(AixError::LeaseOutputExists),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(AixError::LeaseOutputIo(source)),
    }
}

/// Publish a complete temporary file atomically when hard links are supported.
/// On non-Unix filesystems without hard-link support, create-new still refuses
/// overwrites but the final file can be observed while its contents are written;
/// its ACL is inherited because Rust has no portable ACL-setting API.
fn write_secret_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let temporary = parent.join(format!(".aix-lease-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = new_private_file(&temporary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);

        match fs::hard_link(&temporary, path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(error),
            #[cfg(not(unix))]
            Err(_) => write_secret_file_create_new(path, bytes),
            Err(error) => Err(error),
        }
    })();
    let _ = fs::remove_file(&temporary);
    result
}

#[cfg(not(unix))]
fn write_secret_file_create_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
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

async fn warn_if_revoke_failed(client: &LiteLlmAdminClient, key: &SecretString, alias: &str) {
    if client
        .delete_virtual_key(key.expose_secret())
        .await
        .is_err()
    {
        eprintln!(
            "warning: lease {alias} cleanup not confirmed (revocation failed); expiry remains the safety bound"
        );
    }
}

async fn cleanup_after_export_failure(
    client: &LiteLlmAdminClient,
    key: &SecretString,
    alias: &str,
) -> LeaseStatus {
    match client.delete_virtual_key(key.expose_secret()).await {
        Ok(()) => LeaseStatus::Revoked,
        Err(AixError::GatewayError {
            status: 404 | 410, ..
        })
        | Err(AixError::GatewayRequestFailed { status: 404 | 410 }) => {
            LeaseStatus::ExpiredOrUnknown
        }
        Err(_) => {
            eprintln!(
                "warning: lease {alias} cleanup not confirmed (revocation failed); expiry remains the safety bound"
            );
            LeaseStatus::Active
        }
    }
}

#[derive(Serialize)]
struct LeaseExport<'a> {
    schema_version: u8,
    lease_id: &'a str,
    expires_at: Option<&'a str>,
    env: LeaseEnvironment<'a>,
}

struct LeaseEnvironment<'a> {
    profile: &'a str,
    api_key: &'a SecretString,
    base_url: &'a SecretString,
    openai_base_url: &'a SecretString,
}

impl Serialize for LeaseEnvironment<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(7))?;
        map.serialize_entry("AIX_PROFILE", self.profile)?;
        map.serialize_entry("ANTHROPIC_API_KEY", self.api_key.expose_secret())?;
        map.serialize_entry("ANTHROPIC_BASE_URL", self.base_url.expose_secret())?;
        map.serialize_entry("OPENAI_API_KEY", self.api_key.expose_secret())?;
        map.serialize_entry("OPENAI_BASE_URL", self.openai_base_url.expose_secret())?;
        map.serialize_entry("LITELLM_API_KEY", self.api_key.expose_secret())?;
        map.serialize_entry("LITELLM_BASE_URL", self.openai_base_url.expose_secret())?;
        map.end()
    }
}

fn print_list(records: &[LeaseRecord]) {
    if records.is_empty() {
        println!("(no leases)");
        return;
    }
    println!("LEASE ID  PROFILE  BUDGET  EXPIRES AT  STATUS");
    for record in records {
        println!(
            "{}  {:?}  ${:.2}  {}  {}",
            record.lease_id,
            record.profile,
            record.budget,
            record.expires_at.as_deref().unwrap_or("(not returned)"),
            status_name(record.status),
        );
    }
}

fn print_record(record: &LeaseRecord) {
    println!("Schema version: {}", record.schema_version);
    println!("Lease ID: {}", record.lease_id);
    println!("Key alias: {}", record.key_alias);
    println!("Profile: {:?}", record.profile);
    println!("Budget: ${:.2}", record.budget);
    println!("Duration: {:?}", record.duration);
    println!(
        "Expires at: {}",
        record.expires_at.as_deref().unwrap_or("(not returned)")
    );
    println!("Allowed models: {:?}", record.allowed_models);
    println!("Tags: {:?}", record.tags);
    println!("Created at: {}", record.created_at);
    println!("Status: {}", status_name(record.status));
}

fn status_name(status: LeaseStatus) -> &'static str {
    match status {
        LeaseStatus::Active => "active",
        LeaseStatus::Revoked => "revoked",
        LeaseStatus::ExpiredOrUnknown => "expired_or_unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_environment_contains_exactly_the_standard_gateway_contract() {
        let key = SecretString::new("leased-key".to_string());
        let base_url = SecretString::new("https://gateway.example".to_string());
        let openai_base_url = SecretString::new("https://gateway.example/v1".to_string());
        let export = LeaseExport {
            schema_version: LEASE_SCHEMA_VERSION,
            lease_id: "lease-id",
            expires_at: Some("2030-01-02T03:04:05Z"),
            env: LeaseEnvironment {
                profile: "work",
                api_key: &key,
                base_url: &base_url,
                openai_base_url: &openai_base_url,
            },
        };
        let value: serde_json::Value = serde_json::to_value(export).unwrap();
        assert_eq!(value["env"].as_object().unwrap().len(), 7);
        assert_eq!(value["env"]["ANTHROPIC_API_KEY"], "leased-key");
        assert_eq!(
            value["env"]["OPENAI_BASE_URL"],
            "https://gateway.example/v1"
        );
        assert_eq!(value["env"]["LITELLM_API_KEY"], "leased-key");
    }

    #[test]
    fn export_path_refuses_existing_files_and_directories() {
        let directory = assert_fs::TempDir::new().unwrap();
        let path = directory.path().join("existing");
        fs::write(&path, b"existing").unwrap();
        assert!(matches!(
            validate_output_path(&path),
            Err(AixError::LeaseOutputExists)
        ));
        assert!(matches!(
            validate_output_path(directory.path()),
            Err(AixError::LeaseOutputExists)
        ));
    }

    #[test]
    fn expiry_status_only_changes_active_records_and_never_claims_revocation() {
        let record = LeaseRecord {
            schema_version: LEASE_SCHEMA_VERSION,
            lease_id: Uuid::new_v4().to_string(),
            key_alias: "aix-lease-test".to_string(),
            profile: "work".to_string(),
            budget: 1.0,
            duration: "2h".to_string(),
            expires_at: Some("2000-01-02T03:04:05Z".to_string()),
            allowed_models: Vec::new(),
            tags: Vec::new(),
            created_at: "2000-01-01T00:00:00Z".to_string(),
            status: LeaseStatus::Active,
        };
        assert_eq!(
            record.display_status(u64::MAX),
            LeaseStatus::ExpiredOrUnknown
        );
        assert_eq!(record.status, LeaseStatus::Active);
        let mut revoked = record;
        revoked.status = LeaseStatus::Revoked;
        assert_eq!(revoked.display_status(u64::MAX), LeaseStatus::Revoked);
    }
}
