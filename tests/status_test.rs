use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const API_KEY: &str = "sk-status-secret-1234";

fn write_config(dir: &TempDir, base_url: &str) -> std::path::PathBuf {
    let file = dir.child("aix.toml");
    file.write_str(&format!(
        r#"
[endpoint]
base_url = "{base_url}"
gateway = "litellm"
provider = "litellm"

[profiles.work]
label = "Work"
api_key = "{API_KEY}"
"#
    ))
    .unwrap();
    file.path().to_path_buf()
}

async fn mount_healthy_gateway(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": "example-model" }]
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "info": {
                "spend": 41.53,
                "max_budget": 500.0,
                "keys": [{
                    "key_name": "sk-...1234",
                    "spend": 41.53,
                    "max_budget": 500.0
                }]
            }
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn status_shows_profile_gateway_key_suffix_and_budget_without_secrets() {
    let server = MockServer::start().await;
    mount_healthy_gateway(&server).await;

    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args(["--config", config.to_str().unwrap(), "status", "work"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("work · Work"),
        "unexpected output: {stdout}"
    );
    assert!(stdout.contains("litellm"), "unexpected output: {stdout}");
    assert!(stdout.contains("reachable"), "unexpected output: {stdout}");
    assert!(stdout.contains("ms"), "unexpected output: {stdout}");
    assert!(stdout.contains("sk-...1234"), "unexpected output: {stdout}");
    assert!(stdout.contains("$41.53"), "unexpected output: {stdout}");
    assert!(stdout.contains("$500.00"), "unexpected output: {stdout}");
    assert!(stdout.contains("$458.47"), "unexpected output: {stdout}");
    assert!(stdout.contains("8% used"), "unexpected output: {stdout}");
    assert!(!stdout.contains(API_KEY), "API key leaked: {stdout}");
    assert!(!stdout.contains(&server.uri()), "base URL leaked: {stdout}");
    assert!(output.stderr.is_empty());
}

#[tokio::test]
async fn status_json_uses_the_shared_envelope_and_stable_fields() {
    let server = MockServer::start().await;
    mount_healthy_gateway(&server).await;

    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args([
            "--config",
            config.to_str().unwrap(),
            "status",
            "work",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["command"], "status");
    assert_eq!(json["data"]["profile"]["name"], "work");
    assert_eq!(json["data"]["profile"]["label"], "Work");
    assert_eq!(json["data"]["gateway"]["gateway"], "litellm");
    assert_eq!(json["data"]["gateway"]["provider"], "litellm");
    assert_eq!(json["data"]["gateway"]["status"], "reachable");
    assert_eq!(
        json["data"]["gateway"]["authentication_status"],
        "authenticated"
    );
    assert!(json["data"]["gateway"]["latency_ms"].is_u64());
    assert_eq!(json["data"]["key_suffix"], "1234");
    assert_eq!(json["data"]["spend"]["status"], "available");
    assert_eq!(json["data"]["spend"]["spend"], 41.53);
    assert_eq!(json["data"]["spend"]["max_budget"], 500.0);
    assert_eq!(json["data"]["spend"]["remaining_budget"], 458.47);
    assert_eq!(json["data"]["spend"]["percent_used"], 41.53 / 500.0 * 100.0);
    assert_eq!(json["data"]["spend"]["cache"]["source"], "live");
    assert!(json["data"]["spend"]["cache"]["age_seconds"].is_null());
    assert!(!stdout.contains(API_KEY), "API key leaked: {stdout}");
    assert!(!stdout.contains(&server.uri()), "base URL leaked: {stdout}");
    assert!(output.stderr.is_empty());
}

#[tokio::test]
async fn status_uses_cached_spend_but_keeps_the_gateway_probe_live() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "info": { "spend": 41.53, "max_budget": 500.0 }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    let first = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args(["--config", config.to_str().unwrap(), "status", "work"])
        .output()
        .unwrap();
    assert!(first.status.success());
    assert!(String::from_utf8_lossy(&first.stdout).contains("· live"));

    let second = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args([
            "--config",
            config.to_str().unwrap(),
            "status",
            "work",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(second.status.success());
    let json: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(json["data"]["spend"]["cache"]["source"], "cache");
    assert!(json["data"]["spend"]["cache"]["age_seconds"].is_u64());
    // wiremock checks that the connectivity probe ran twice and spend was fetched once.
}

#[tokio::test]
async fn status_refresh_bypasses_the_spend_cache() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "info": { "spend": 41.53, "max_budget": 500.0 }
        })))
        .expect(2)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());
    let args = ["--config", config.to_str().unwrap(), "status", "work"];

    let first = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args(args)
        .output()
        .unwrap();
    assert!(first.status.success());

    let refreshed = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args([
            "--config",
            config.to_str().unwrap(),
            "status",
            "work",
            "--refresh",
        ])
        .output()
        .unwrap();
    assert!(refreshed.status.success());
    assert!(String::from_utf8_lossy(&refreshed.stdout).contains("· live"));
    // wiremock expects both status calls to fetch spend rather than reusing the first result.
}

#[tokio::test]
async fn status_reports_spend_unsupported_for_a_non_litellm_gateway() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "info": {} })))
        .expect(0)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = dir.child("aix.toml");
    config
        .write_str(&format!(
            r#"
[endpoint]
base_url = "{}"
gateway = "openai-compatible"
provider = "openai"

[profiles.work]
api_key = "{API_KEY}"
"#,
            server.uri()
        ))
        .unwrap();

    let output = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args([
            "--config",
            config.path().to_str().unwrap(),
            "status",
            "work",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["data"]["gateway"]["gateway"], "openai-compatible");
    assert_eq!(json["data"]["gateway"]["provider"], "openai");
    assert_eq!(json["data"]["gateway"]["status"], "reachable");
    assert_eq!(json["data"]["spend"]["status"], "unsupported");
    assert!(json["data"]["spend"]["spend"].is_null());
    assert!(json["data"]["spend"]["max_budget"].is_null());
    assert_eq!(json["data"]["spend"]["cache"]["source"], "none");
}

#[tokio::test]
async fn status_succeeds_when_litellm_management_endpoint_is_unsupported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args(["--config", config.to_str().unwrap(), "status", "work"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Spend       unsupported"));
    assert!(output.stderr.is_empty());
}

#[tokio::test]
async fn status_reports_spend_unavailable_when_management_response_has_no_spend_fields() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "info": {} })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args(["--config", config.to_str().unwrap(), "status", "work"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Spend       unavailable"));
    assert!(output.stderr.is_empty());
}

#[tokio::test]
async fn status_authentication_failure_uses_auth_exit_code_without_leaking_secrets() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "message": format!("credential {API_KEY} rejected at {}", server.uri()),
            "access_token": "private-upstream-token"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args([
            "--json",
            "--config",
            config.to_str().unwrap(),
            "status",
            "work",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("403"), "unexpected diagnostic: {stderr}");
    assert!(!stderr.contains(API_KEY), "API key leaked: {stderr}");
    assert!(
        !stderr.contains("private-upstream-token"),
        "upstream token leaked: {stderr}"
    );
    assert!(!stderr.contains(&server.uri()), "base URL leaked: {stderr}");
}

#[tokio::test]
async fn status_network_failure_uses_network_exit_code_without_leaking_secrets() {
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let api_key = "sk-status-network-secret-5678";
    let url_marker = "private-url-marker";
    let config = dir.child("aix.toml");
    config
        .write_str(&format!(
            r#"
[endpoint]
base_url = "http://127.0.0.1:1/{url_marker}"

[profiles.work]
api_key = "{api_key}"
"#
        ))
        .unwrap();

    let output = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args([
            "--json",
            "--config",
            config.path().to_str().unwrap(),
            "status",
            "work",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains(api_key), "API key leaked: {stderr}");
    assert!(!stderr.contains(url_marker), "URL leaked: {stderr}");
}

#[tokio::test]
async fn status_secondary_timeout_uses_the_network_exit_code() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": "example-model" }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(2))
                .set_body_json(json!({ "info": { "spend": 1.0 } })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args([
            "--timeout",
            "100ms",
            "--config",
            config.to_str().unwrap(),
            "status",
            "work",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    assert!(
        stderr.contains("timeout") || stderr.contains("timed out"),
        "{stderr}"
    );
}

#[tokio::test]
async fn status_never_prints_a_short_key_as_its_own_suffix() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("Authorization", "Bearer abcd"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "info": { "spend": 1.23 }
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = dir.child("aix.toml");
    config
        .write_str(&format!(
            r#"
[endpoint]
base_url = "{}"

[profiles.work]
api_key = "abcd"
"#,
            server.uri()
        ))
        .unwrap();
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args([
            "--config",
            config.path().to_str().unwrap(),
            "status",
            "work",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Key         redacted"),
        "unexpected output: {stdout}"
    );
    assert!(!stdout.contains("abcd"), "short API key leaked: {stdout}");
}
