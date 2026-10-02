use assert_cmd::Command;
use assert_fs::prelude::*;
use serde_json::{json, Value};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PARENT_KEY: &str = "sk-parent-private";
const OTHER_KEY: &str = "sk-other-private";
const LEASE_KEY: &str = "sk-exported-private";

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

fn write_config(base_url: &str, gateway: Option<&str>, extra: &str) -> assert_fs::NamedTempFile {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    let gateway = gateway
        .map(|gateway| format!("gateway = \"{gateway}\""))
        .unwrap_or_default();
    config
        .write_str(&format!(
            r#"
default_profile = "work"

[endpoint]
base_url = "{base_url}"
{gateway}

[profiles.work]
api_key = {{ env = "AIX_PARENT_KEY" }}

[profiles.work.models.aliases]
smart = "provider/model-x"

[profiles.work.env]
PRIVATE_PROFILE_VALUE = "must-not-be-exported"

{extra}
"#
        ))
        .unwrap();
    config
}

async fn mount_generate(server: &MockServer, status: u16, expires: &str) {
    Mock::given(method("POST"))
        .and(path("/key/generate"))
        .and(header("Authorization", format!("Bearer {PARENT_KEY}")))
        .respond_with(ResponseTemplate::new(status).set_body_json(json!({
            "key": LEASE_KEY,
            "expires": expires
        })))
        .expect(1)
        .mount(server)
        .await;
}

async fn mount_delete_by_key(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/key/delete"))
        .and(header("Authorization", format!("Bearer {PARENT_KEY}")))
        .and(body_partial_json(json!({ "keys": [LEASE_KEY] })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "deleted": true })))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
#[cfg(unix)]
async fn create_exports_only_standard_leased_credentials_and_stores_no_secrets() {
    let server = MockServer::start().await;
    mount_generate(&server, 200, "2030-01-02T03:04:05Z").await;
    let config = write_config(&server.uri(), Some("litellm"), "");
    let state = assert_fs::TempDir::new().unwrap();
    let output_path = state.path().join("worker-lease.json");

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .args([
            "--profile",
            "work",
            "lease",
            "create",
            "--budget",
            "2.00",
            "--duration",
            "2h",
            "--allow-model",
            "smart",
            "--tag",
            "task:ABC-123",
            "--output",
        ])
        .arg(&output_path)
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains(PARENT_KEY));
    assert!(!stdout.contains(LEASE_KEY));
    assert!(!stderr.contains(PARENT_KEY));
    assert!(!stderr.contains(LEASE_KEY));

    let export: Value = serde_json::from_slice(&std::fs::read(&output_path).unwrap()).unwrap();
    let lease_id = export["lease_id"].as_str().unwrap();
    assert!(uuid::Uuid::parse_str(lease_id).is_ok());
    assert_eq!(export["schema_version"], 1);
    assert_eq!(export["expires_at"], "2030-01-02T03:04:05Z");
    assert_eq!(export["env"]["AIX_PROFILE"], "work");
    assert_eq!(export["env"]["ANTHROPIC_API_KEY"], LEASE_KEY);
    assert_eq!(export["env"]["OPENAI_API_KEY"], LEASE_KEY);
    assert_eq!(export["env"]["LITELLM_API_KEY"], LEASE_KEY);
    assert_eq!(export["env"]["ANTHROPIC_BASE_URL"], server.uri());
    assert_eq!(
        export["env"]["OPENAI_BASE_URL"],
        format!("{}/v1", server.uri())
    );
    assert_eq!(
        export["env"]["LITELLM_BASE_URL"],
        format!("{}/v1", server.uri())
    );
    assert_eq!(export["env"].as_object().unwrap().len(), 7);
    assert!(!export.to_string().contains(PARENT_KEY));
    assert!(!export.to_string().contains("PRIVATE_PROFILE_VALUE"));
    assert!(!export.to_string().contains("must-not-be-exported"));

    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&output_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let generate: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(generate["max_budget"], 2.0);
    assert_eq!(generate["duration"], "2h");
    assert_eq!(generate["models"], json!(["provider/model-x"]));
    assert_eq!(generate["key_alias"], format!("aix-lease-{lease_id}"));
    assert_eq!(
        generate["metadata"]["tags"],
        json!([format!("aix:lease:{lease_id}"), "task:ABC-123"])
    );

    let registry = std::fs::read_dir(state.path().join("leases"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let registry_text =
        std::fs::read_to_string(registry.join("00000000000000000001.json")).unwrap();
    assert!(!registry_text.contains(PARENT_KEY));
    assert!(!registry_text.contains(LEASE_KEY));
    assert!(!registry_text.contains(&server.uri()));
}

#[tokio::test]
async fn existing_output_path_is_rejected_before_key_creation() {
    let server = MockServer::start().await;
    let config = write_config(&server.uri(), None, "");
    let state = assert_fs::TempDir::new().unwrap();
    let output_path = state.path().join("already-exists.json");
    std::fs::write(&output_path, "keep me").unwrap();

    cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .args(["lease", "create", "--budget", "1", "--output"])
        .arg(&output_path)
        .assert()
        .code(2);

    assert_eq!(std::fs::read_to_string(&output_path).unwrap(), "keep me");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn secret_file_write_failure_best_effort_revokes_created_key() {
    let server = MockServer::start().await;
    mount_generate(&server, 200, "2030-01-02T03:04:05Z").await;
    mount_delete_by_key(&server).await;
    let config = write_config(&server.uri(), None, "");
    let state = assert_fs::TempDir::new().unwrap();

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .args([
            "lease",
            "create",
            "--budget",
            "1",
            "--output",
            "/proc/self/aix-lease-export.json",
        ])
        .assert()
        .failure()
        .get_output()
        .clone();

    let output_text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output_text.contains(PARENT_KEY));
    assert!(!output_text.contains(LEASE_KEY));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].url.path(), "/key/generate");
    assert_eq!(requests[1].url.path(), "/key/delete");
}

#[tokio::test]
async fn listing_and_show_are_offline_and_report_expiry_as_unverified() {
    let server = MockServer::start().await;
    mount_generate(&server, 200, "2000-01-02T03:04:05Z").await;
    let config = write_config(&server.uri(), None, "");
    let state = assert_fs::TempDir::new().unwrap();
    let output_path = state.path().join("lease.json");

    cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .args(["lease", "create", "--budget", "1", "--output"])
        .arg(&output_path)
        .assert()
        .success();
    let requests = server.received_requests().await.unwrap();
    let generate: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(generate["duration"], "2h");
    assert!(generate.get("models").is_none());
    let export: Value = serde_json::from_slice(&std::fs::read(&output_path).unwrap()).unwrap();
    let lease_id = export["lease_id"].as_str().unwrap();

    let listed = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["leases", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let listed: Value = serde_json::from_slice(&listed).unwrap();
    assert_eq!(listed["command"], "leases");
    assert_eq!(listed["data"][0]["lease_id"], lease_id);
    assert_eq!(listed["data"][0]["status"], "expired_or_unknown");
    assert!(!listed.to_string().contains(PARENT_KEY));
    assert!(!listed.to_string().contains(LEASE_KEY));
    assert!(!listed.to_string().contains(&server.uri()));

    let shown = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["lease", "show", lease_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let shown: Value = serde_json::from_slice(&shown).unwrap();
    assert_eq!(shown["command"], "lease show");
    assert_eq!(shown["data"]["status"], "expired_or_unknown");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn revoke_uses_recorded_profile_and_alias_without_touching_export_file() {
    let server = MockServer::start().await;
    mount_generate(&server, 200, "2030-01-02T03:04:05Z").await;
    Mock::given(method("POST"))
        .and(path("/key/delete"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "deleted": true })))
        .expect(1)
        .mount(&server)
        .await;
    let config = write_config(
        &server.uri(),
        None,
        r#"
[profiles.other]
api_key = { env = "AIX_OTHER_KEY" }
"#,
    );
    let state = assert_fs::TempDir::new().unwrap();
    let output_path = state.path().join("lease.json");

    cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .args([
            "--profile",
            "work",
            "lease",
            "create",
            "--budget",
            "1",
            "--output",
        ])
        .arg(&output_path)
        .assert()
        .success();
    let export: Value = serde_json::from_slice(&std::fs::read(&output_path).unwrap()).unwrap();
    let lease_id = export["lease_id"].as_str().unwrap();
    let original_file = std::fs::read(&output_path).unwrap();

    cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .env("AIX_OTHER_KEY", OTHER_KEY)
        .args(["--profile", "other", "lease", "revoke", lease_id])
        .assert()
        .success()
        .stdout(predicates::str::contains("remove"));

    assert_eq!(std::fs::read(&output_path).unwrap(), original_file);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1]
        .headers
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .contains(PARENT_KEY));
    let delete: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(
        delete["key_aliases"],
        json!([format!("aix-lease-{lease_id}")])
    );
    assert!(delete.get("keys").is_none());

    let shown = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["lease", "show", lease_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let shown: Value = serde_json::from_slice(&shown).unwrap();
    assert_eq!(shown["data"]["status"], "revoked");
}

#[tokio::test]
async fn failed_revoke_keeps_the_local_lease_status_unchanged() {
    let server = MockServer::start().await;
    mount_generate(&server, 200, "2030-01-02T03:04:05Z").await;
    Mock::given(method("POST"))
        .and(path("/key/delete"))
        .respond_with(ResponseTemplate::new(403).set_body_string("denied"))
        .expect(1)
        .mount(&server)
        .await;
    let config = write_config(&server.uri(), None, "");
    let state = assert_fs::TempDir::new().unwrap();
    let output_path = state.path().join("lease.json");

    cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .args(["lease", "create", "--budget", "1", "--output"])
        .arg(&output_path)
        .assert()
        .success();
    let export: Value = serde_json::from_slice(&std::fs::read(&output_path).unwrap()).unwrap();
    let lease_id = export["lease_id"].as_str().unwrap();

    cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .args(["lease", "revoke", lease_id])
        .assert()
        .code(4);

    let shown = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["lease", "show", lease_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let shown: Value = serde_json::from_slice(&shown).unwrap();
    assert_eq!(shown["data"]["status"], "active");
}

#[tokio::test]
async fn non_litellm_gateway_fails_before_creating_an_export_or_key() {
    let server = MockServer::start().await;
    let config = write_config(&server.uri(), Some("other"), "");
    let state = assert_fs::TempDir::new().unwrap();
    let output_path = state.path().join("lease.json");

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .args(["lease", "create", "--budget", "1", "--output"])
        .arg(&output_path)
        .assert()
        .code(2)
        .get_output()
        .clone();

    assert!(String::from_utf8_lossy(&output.stderr).contains("LiteLLM-compatible gateway"));
    assert!(!output_path.exists());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn parent_key_in_output_path_is_never_printed_or_sent_to_gateway() {
    let server = MockServer::start().await;
    let config = write_config(&server.uri(), None, "");
    let state = assert_fs::TempDir::new().unwrap();
    let output_path = state.path().join(PARENT_KEY);

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .args(["lease", "create", "--budget", "1", "--output"])
        .arg(&output_path)
        .assert()
        .code(2)
        .get_output()
        .clone();

    assert!(!String::from_utf8_lossy(&output.stdout).contains(PARENT_KEY));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(PARENT_KEY));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test]
fn raw_credential_used_as_lease_id_is_not_echoed() {
    let state = assert_fs::TempDir::new().unwrap();
    let output = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["lease", "show", LEASE_KEY])
        .assert()
        .failure()
        .get_output()
        .clone();

    assert!(!String::from_utf8_lossy(&output.stdout).contains(LEASE_KEY));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(LEASE_KEY));
}
