use assert_cmd::Command;
use assert_fs::prelude::*;
use serde_json::{json, Value};
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PARENT_KEY: &str = "sk-parent-private";
const LEASE_KEY: &str = "sk-leased-private";

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

{extra}
"#
        ))
        .unwrap();
    config
}

async fn mount_generate(server: &MockServer, key: &str, status: u16) {
    Mock::given(method("POST"))
        .and(path("/key/generate"))
        .and(header("Authorization", format!("Bearer {PARENT_KEY}")))
        .respond_with(ResponseTemplate::new(status).set_body_json(json!({
            "key": key,
            "expires": "2030-01-02T03:04:05Z"
        })))
        .expect(1)
        .mount(server)
        .await;
}

async fn mount_info(server: &MockServer, key: &str, status: u16, response: Value) {
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .and(header("Authorization", format!("Bearer {PARENT_KEY}")))
        .and(query_param("key", key))
        .respond_with(ResponseTemplate::new(status).set_body_json(response))
        .expect(1)
        .mount(server)
        .await;
}

async fn mount_delete(server: &MockServer, key: &str, status: u16) {
    Mock::given(method("POST"))
        .and(path("/key/delete"))
        .and(header("Authorization", format!("Bearer {PARENT_KEY}")))
        .and(body_partial_json(json!({ "keys": [key] })))
        .respond_with(ResponseTemplate::new(status).set_body_json(json!({ "deleted": true })))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(unix)]
async fn leased_run_uses_temporary_key_and_records_cleanup_without_secrets() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/key/generate"))
        .and(header("Authorization", format!("Bearer {PARENT_KEY}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "key": LEASE_KEY,
            "expires": "2030-01-02T03:04:05Z"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .and(header("Authorization", format!("Bearer {PARENT_KEY}")))
        .and(query_param("key", LEASE_KEY))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "info": { "spend": 0.75 }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/key/delete"))
        .and(header("Authorization", format!("Bearer {PARENT_KEY}")))
        .and(body_partial_json(json!({ "keys": [LEASE_KEY] })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "deleted": true })))
        .expect(1)
        .mount(&server)
        .await;

    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(&format!(
            r#"
[endpoint]
base_url = "{}"
gateway = "litellm"

[profiles.work]
api_key = {{ env = "AIX_PARENT_KEY" }}

[profiles.work.models.aliases]
smart = "provider/model-x"

[profiles.work.env]
CUSTOM_PARENT_KEY = "prefix {PARENT_KEY} suffix"

[tools.review]
command = "sh"
api_format = "openai"

[tools.review.env]
OPENAI_API_KEY = "tool-secret-override"
OPENAI_BASE_URL = "https://tool.invalid"
"#,
            server.uri()
        ))
        .unwrap();
    let state = assert_fs::TempDir::new().unwrap();
    let script = format!(
        "test \"$OPENAI_API_KEY\" = {LEASE_KEY} && test \"$LITELLM_API_KEY\" = {LEASE_KEY} && test \"$OPENAI_BASE_URL\" = \"{}/v1\" && test \"$LITELLM_BASE_URL\" = \"{}/v1\" && test -z \"${{AIX_PARENT_KEY+x}}\" && test -z \"${{CUSTOM_PARENT_KEY+x}}\" && printf '%s\\n' \"$AIX_RUN_ID\"",
        server.uri(),
        server.uri()
    );

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .args([
            "run",
            "--profile",
            "work",
            "--lease",
            "--budget",
            "2.00",
            "--duration",
            "2h",
            "--allow-model",
            "smart",
            "--tag",
            "issue:123",
            "--",
            "review",
            "-c",
            script.as_str(),
        ])
        .assert()
        .success()
        .get_output()
        .clone();
    let run_id = String::from_utf8(output.stdout.clone())
        .unwrap()
        .trim()
        .to_string();
    assert!(uuid::Uuid::parse_str(&run_id).is_ok());
    let output_text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output_text.contains(PARENT_KEY));
    assert!(!output_text.contains(LEASE_KEY));

    let record_output = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", &run_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let envelope: Value = serde_json::from_slice(&record_output).unwrap();
    let record = &envelope["data"];
    assert_eq!(record["lease"]["key_alias"], format!("aix-run-{run_id}"));
    assert_eq!(record["lease"]["budget"], 2.0);
    assert_eq!(record["lease"]["duration"], "2h");
    assert_eq!(record["lease"]["expires_at"], "2030-01-02T03:04:05Z");
    assert_eq!(
        record["lease"]["allowed_models"],
        json!(["provider/model-x"])
    );
    assert_eq!(record["lease"]["spend"], 0.75);
    assert_eq!(record["lease"]["cleanup_status"], "revoked");
    let serialized = envelope.to_string();
    assert!(!serialized.contains(PARENT_KEY));
    assert!(!serialized.contains(LEASE_KEY));

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].url.path(), "/key/generate");
    assert_eq!(requests[1].url.path(), "/key/info");
    assert_eq!(requests[2].url.path(), "/key/delete");
    let generate: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(generate["max_budget"], 2.0);
    assert_eq!(generate["duration"], "2h");
    assert_eq!(generate["models"], json!(["provider/model-x"]));
    assert_eq!(generate["key_alias"], format!("aix-run-{run_id}"));
    assert_eq!(
        generate["metadata"]["tags"],
        json!([format!("aix:run:{run_id}"), "issue:123"])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(unix)]
async fn child_failure_still_reads_spend_and_revokes_when_spend_lookup_fails() {
    let server = MockServer::start().await;
    mount_generate(&server, LEASE_KEY, 200).await;
    mount_info(
        &server,
        LEASE_KEY,
        500,
        json!({ "message": format!("failed for {LEASE_KEY}") }),
    )
    .await;
    mount_delete(&server, LEASE_KEY, 200).await;
    let config = write_config(&server.uri(), None, "");
    let state = assert_fs::TempDir::new().unwrap();

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .args([
            "run",
            "--lease",
            "--budget",
            "1",
            "--",
            "sh",
            "-c",
            "printf '%s\\n' \"$AIX_RUN_ID\"; exit 42",
        ])
        .assert()
        .code(42)
        .get_output()
        .clone();
    let run_id = String::from_utf8(output.stdout).unwrap().trim().to_string();
    assert!(!String::from_utf8_lossy(&output.stderr).contains(LEASE_KEY));

    let record_output = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", &run_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let envelope: Value = serde_json::from_slice(&record_output).unwrap();
    assert_eq!(envelope["data"]["status"], "failed");
    assert_eq!(envelope["data"]["process_exit_code"], 42);
    assert!(envelope["data"]["lease"]["spend"].is_null());
    assert_eq!(envelope["data"]["lease"]["cleanup_status"], "revoked");
    assert!(!envelope.to_string().contains(PARENT_KEY));
    assert!(!envelope.to_string().contains(LEASE_KEY));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(unix)]
async fn successful_child_with_failed_revoke_returns_infrastructure_failure() {
    let server = MockServer::start().await;
    mount_generate(&server, LEASE_KEY, 200).await;
    mount_info(
        &server,
        LEASE_KEY,
        200,
        json!({ "info": { "spend": 0.25 } }),
    )
    .await;
    mount_delete(&server, LEASE_KEY, 503).await;
    let config = write_config(&server.uri(), Some("litellm"), "");
    let state = assert_fs::TempDir::new().unwrap();

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .args([
            "run",
            "--lease",
            "--budget",
            "1",
            "--",
            "sh",
            "-c",
            "printf '%s\\n' \"$AIX_RUN_ID\"",
        ])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let run_id = String::from_utf8(output.stdout).unwrap().trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning: lease aix-run-"));
    assert!(stderr.contains("revocation failed"));
    assert!(!stderr.contains(PARENT_KEY));
    assert!(!stderr.contains(LEASE_KEY));

    let record_output = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", &run_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let envelope: Value = serde_json::from_slice(&record_output).unwrap();
    assert_eq!(envelope["data"]["lease"]["spend"], 0.25);
    assert_eq!(envelope["data"]["lease"]["cleanup_status"], "revoke_failed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(unix)]
async fn generation_authorization_failure_prevents_launch_and_keeps_error_secret_free() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/key/generate"))
        .and(header("Authorization", format!("Bearer {PARENT_KEY}")))
        .respond_with(
            ResponseTemplate::new(403).set_body_string(format!("denied {PARENT_KEY} {LEASE_KEY}")),
        )
        .expect(1)
        .mount(&server)
        .await;
    let config = write_config(&server.uri(), None, "");
    let state = assert_fs::TempDir::new().unwrap();

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .args([
            "run",
            "--lease",
            "--budget",
            "1",
            "--",
            "sh",
            "-c",
            "printf CHILD_LAUNCHED",
        ])
        .assert()
        .code(4)
        .get_output()
        .clone();
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("denied virtual-key generation"));
    assert!(!stderr.contains("CHILD_LAUNCHED"));
    assert!(!stderr.contains(PARENT_KEY));
    assert!(!stderr.contains(LEASE_KEY));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);

    let runs = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let runs: Value = serde_json::from_slice(&runs).unwrap();
    assert_eq!(runs["data"][0]["status"], "failed");
    assert!(runs["data"][0]["lease"].is_null());
    assert!(!runs.to_string().contains(PARENT_KEY));
    assert!(!runs.to_string().contains(LEASE_KEY));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(unix)]
async fn dry_run_resolves_policy_without_network_child_or_run_record() {
    let server = MockServer::start().await;
    let config = write_config(
        &server.uri(),
        None,
        r#"
[tools.review]
command = "sh"
api_format = "openai"

[tools.review.env]
DEFERRED_SECRET = { env = "AIX_MISSING_TOOL_SECRET" }
"#,
    );
    let state = assert_fs::TempDir::new().unwrap();

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .args([
            "run",
            "--lease",
            "--budget",
            "2",
            "--allow-model",
            "smart",
            "--dry-run",
            "--",
            "review",
            "-c",
            "printf CHILD_LAUNCHED",
        ])
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Would create LiteLLM virtual-key lease"));
    assert!(stderr.contains("budget: $2.00"));
    assert!(stderr.contains("duration: 2h"));
    assert!(stderr.contains("provider/model-x"));
    assert!(stderr.contains("OPENAI_API_KEY"));
    assert!(stderr.contains("DEFERRED_SECRET"));
    assert!(!stderr.contains(PARENT_KEY));
    assert!(!stderr.contains(LEASE_KEY));
    assert!(!stderr.contains("CHILD_LAUNCHED"));
    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(std::fs::read_dir(state.path()).unwrap().count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(unix)]
async fn unset_gateway_metadata_uses_default_duration_and_omits_model_restriction() {
    let server = MockServer::start().await;
    mount_generate(&server, LEASE_KEY, 200).await;
    mount_info(&server, LEASE_KEY, 200, json!({ "spend": 0.0 })).await;
    mount_delete(&server, LEASE_KEY, 200).await;
    let config = write_config(&server.uri(), None, "");
    let state = assert_fs::TempDir::new().unwrap();

    cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .args([
            "run", "--lease", "--budget", "0.5", "--", "sh", "-c", "true",
        ])
        .assert()
        .success();

    let requests = server.received_requests().await.unwrap();
    let generate: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(generate["duration"], "2h");
    assert_eq!(generate["max_budget"], 0.5);
    assert!(generate.get("models").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(unix)]
async fn explicit_non_litellm_gateway_rejects_lease_before_network_or_launch() {
    let server = MockServer::start().await;
    let config = write_config(&server.uri(), Some("other"), "");
    let state = assert_fs::TempDir::new().unwrap();

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .args([
            "run",
            "--lease",
            "--budget",
            "1",
            "--",
            "sh",
            "-c",
            "printf CHILD_LAUNCHED",
        ])
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("LiteLLM-compatible gateway"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(unix)]
async fn abrupt_parent_termination_leaves_a_finite_durable_lease_record() {
    let server = MockServer::start().await;
    mount_generate(&server, LEASE_KEY, 200).await;
    let config = write_config(&server.uri(), Some("litellm"), "");
    let state = assert_fs::TempDir::new().unwrap();

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .args([
            "run",
            "--lease",
            "--budget",
            "1",
            "--duration",
            "30s",
            "--",
            "sh",
            "-c",
            "printf '%s\\n' \"$AIX_RUN_ID\"; kill -9 \"$PPID\"; sleep 0.05",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let run_id = String::from_utf8(output)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();

    let record_output = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", &run_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let envelope: Value = serde_json::from_slice(&record_output).unwrap();
    assert_eq!(envelope["data"]["status"], "running");
    assert_eq!(envelope["data"]["lease"]["duration"], "30s");
    assert_eq!(
        envelope["data"]["lease"]["cleanup_status"],
        "expired_or_unverified"
    );
    assert!(!envelope.to_string().contains(PARENT_KEY));
    assert!(!envelope.to_string().contains(LEASE_KEY));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(unix)]
async fn lease_rejects_parent_secret_in_metadata_or_child_arguments() {
    let server = MockServer::start().await;
    let config = write_config(&server.uri(), None, "");
    let state = assert_fs::TempDir::new().unwrap();

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .args([
            "run", "--lease", "--budget", "1", "--tag", PARENT_KEY, "--", "sh", "-c", "true",
        ])
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("must not contain the selected profile credential"));
    assert!(!stderr.contains(PARENT_KEY));
    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(std::fs::read_dir(state.path()).unwrap().count(), 0);
}

#[test]
fn lease_cli_validates_budget_and_duration_before_execution() {
    for args in [
        vec!["run", "--lease", "--", "true"],
        vec!["run", "--lease", "--budget", "0", "--", "true"],
        vec!["run", "--lease", "--budget", "NaN", "--", "true"],
        vec![
            "run",
            "--lease",
            "--budget",
            "1",
            "--duration",
            "forever",
            "--",
            "true",
        ],
    ] {
        let output = cmd().args(args).assert().code(2).get_output().clone();
        assert!(output.stdout.is_empty());
    }
}
