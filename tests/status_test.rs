use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use serde_json::{json, Value};
use std::time::Duration;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const API_KEY: &str = "sk-status-native-secret-1234";

fn write_config(
    dir: &TempDir,
    base_url: &str,
    gateway: Option<&str>,
    api_key: &str,
) -> std::path::PathBuf {
    let gateway = gateway.map_or(String::new(), |gateway| {
        format!("gateway = \"{gateway}\"\n")
    });
    let file = dir.child("aix.toml");
    file.write_str(&format!(
        r#"
[endpoint]
base_url = "{base_url}"
{gateway}
provider = "openai"

[profiles.work]
label = "Work"
api_key = "{api_key}"
"#
    ))
    .unwrap();
    file.path().to_path_buf()
}

fn native_budget() -> Value {
    json!({
        "key_spent_usd": "41.53",
        "remaining_usd": "12.00",
        "limited_by": "project",
        "layers": [
            {
                "scope": "key",
                "budget_usd": "100.00",
                "spent_usd": "41.53",
                "remaining_usd": "58.47",
                "period_start": "2026-11-01T00:00:00Z"
            },
            {
                "scope": "project",
                "budget_usd": "20.00",
                "spent_usd": "8.00",
                "remaining_usd": "12.00",
                "period_start": "2026-11-01T00:00:00Z"
            }
        ],
        "expires_at": "2026-12-01T00:00:00Z",
        "key_id": "private-key-id",
        "metadata": { "token": "private-token" }
    })
}

async fn mount_models(server: &MockServer, api_key: &str, expected: u64) {
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("Authorization", format!("Bearer {api_key}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": "example-model" }]
        })))
        .expect(expected)
        .mount(server)
        .await;
}

async fn mount_budget(server: &MockServer, api_key: &str, body: Value, expected: u64) {
    Mock::given(method("GET"))
        .and(path("/v1/key/info"))
        .and(header("Authorization", format!("Bearer {api_key}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .expect(expected)
        .mount(server)
        .await;
}

fn invoke(
    config: &std::path::Path,
    cache_dir: &std::path::Path,
    args: &[&str],
) -> std::process::Output {
    Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir)
        .args(["--config", config.to_str().unwrap()])
        .args(args)
        .output()
        .unwrap()
}

#[tokio::test]
async fn status_human_output_shows_key_spend_and_native_effective_remaining() {
    let server = MockServer::start().await;
    mount_models(&server, API_KEY, 1).await;
    mount_budget(&server, API_KEY, native_budget(), 1).await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), Some("litellm"), API_KEY);

    let output = invoke(&config, cache_dir.path(), &["status", "work"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("work · Work"), "{stdout}");
    assert!(stdout.contains("litellm"), "{stdout}");
    assert!(stdout.contains("sk-...1234"), "{stdout}");
    assert!(stdout.contains("Key spend   $41.53"), "{stdout}");
    assert!(
        stdout.contains("Available   $12.00 · limited by project"),
        "{stdout}"
    );
    assert!(stdout.contains("$41.53 / $100.00"), "{stdout}");
    assert!(stdout.contains("Project *"), "{stdout}");
    assert!(stdout.contains("42% used"), "{stdout}");
    assert!(stdout.contains("40% used"), "{stdout}");
    assert_eq!(
        stdout.matches('░').count() + stdout.matches('█').count(),
        80
    );
    assert!(!stdout.contains("$53.53"), "fabricated total in {stdout}");
    assert!(!stdout.contains(API_KEY), "API key leaked: {stdout}");
    assert!(!stdout.contains(&server.uri()), "base URL leaked: {stdout}");
    assert!(output.stderr.is_empty());
}

#[tokio::test]
async fn status_json_keeps_the_envelope_and_reports_native_budget_fields() {
    let server = MockServer::start().await;
    mount_models(&server, API_KEY, 1).await;
    mount_budget(&server, API_KEY, native_budget(), 1).await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), Some("litellm"), API_KEY);

    let output = invoke(&config, cache_dir.path(), &["status", "work", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["command"], "status");
    assert_eq!(report["data"]["profile"]["name"], "work");
    assert_eq!(report["data"]["profile"]["label"], "Work");
    assert_eq!(report["data"]["gateway"]["status"], "reachable");
    assert_eq!(report["data"]["key_suffix"], "1234");
    assert_eq!(report["data"]["spend"]["status"], "available");
    assert_eq!(report["data"]["spend"]["key_spent_usd"], 41.53);
    assert_eq!(report["data"]["spend"]["remaining_usd"], 12.0);
    assert_eq!(report["data"]["spend"]["limited_by"], "project");
    assert_eq!(report["data"]["spend"]["layers"][1]["scope"], "project");
    assert_eq!(report["data"]["spend"]["layers"][1]["budget_usd"], 20.0);
    assert_eq!(report["data"]["spend"]["cache"]["source"], "live");
    assert!(report["data"]["spend"].get("max_budget").is_none());
    assert!(report["data"]["spend"].get("percent_used").is_none());
    for secret in [API_KEY, "private-key-id", "private-token", "metadata"] {
        assert!(!stdout.contains(secret), "leaked {secret}: {stdout}");
    }
    assert!(output.stderr.is_empty());
}

#[tokio::test]
async fn status_preserves_unlimited_native_remaining_and_layer_fields() {
    let server = MockServer::start().await;
    mount_models(&server, API_KEY, 1).await;
    let mut budget = native_budget();
    budget["remaining_usd"] = Value::Null;
    budget["limited_by"] = Value::Null;
    budget["layers"] = json!([{
        "scope": "tenant",
        "budget_usd": null,
        "spent_usd": "41.53",
        "remaining_usd": null,
        "period_start": "2026-11-01T00:00:00Z"
    }]);
    mount_budget(&server, API_KEY, budget, 1).await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), None, API_KEY);

    let output = invoke(&config, cache_dir.path(), &["status", "work", "--json"]);
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["data"]["spend"]["key_spent_usd"], 41.53);
    assert_eq!(report["data"]["spend"]["remaining_usd"], Value::Null);
    assert_eq!(report["data"]["spend"]["limited_by"], Value::Null);
    assert_eq!(
        report["data"]["spend"]["layers"][0]["budget_usd"],
        Value::Null
    );
    assert_eq!(
        report["data"]["spend"]["layers"][0]["remaining_usd"],
        Value::Null
    );
}

#[tokio::test]
async fn budget_capability_is_probed_even_for_non_litellm_gateway_metadata() {
    let server = MockServer::start().await;
    mount_models(&server, API_KEY, 1).await;
    mount_budget(&server, API_KEY, native_budget(), 1).await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), Some("openai-compatible"), API_KEY);

    let output = invoke(&config, cache_dir.path(), &["status", "work", "--json"]);
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["data"]["gateway"]["gateway"], "openai-compatible");
    assert_eq!(report["data"]["spend"]["status"], "available");
}

#[tokio::test]
async fn unsupported_native_budget_endpoint_is_reported_without_failing_status() {
    let server = MockServer::start().await;
    mount_models(&server, API_KEY, 1).await;
    Mock::given(method("GET"))
        .and(path("/v1/key/info"))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .expect(1)
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), Some("openai-compatible"), API_KEY);

    let output = invoke(&config, cache_dir.path(), &["status", "work", "--json"]);
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["data"]["spend"]["status"], "unsupported");
    assert_eq!(report["data"]["spend"]["key_spent_usd"], Value::Null);
    assert_eq!(report["data"]["spend"]["layers"], Value::Null);
    assert_eq!(report["data"]["spend"]["cache"]["source"], "none");
}

#[tokio::test]
async fn malformed_budget_payload_fails_protocol_instead_of_reporting_zero() {
    let server = MockServer::start().await;
    mount_models(&server, API_KEY, 1).await;
    let mut malformed = native_budget();
    malformed["layers"] = Value::Null;
    mount_budget(&server, API_KEY, malformed, 1).await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), None, API_KEY);

    let output = invoke(&config, cache_dir.path(), &["status", "work", "--json"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("malformed native budget information"),
        "{stderr}"
    );
    assert!(!stderr.contains(API_KEY));
}

#[tokio::test]
async fn status_uses_budget_cache_but_keeps_gateway_probe_live() {
    let server = MockServer::start().await;
    mount_models(&server, API_KEY, 2).await;
    mount_budget(&server, API_KEY, native_budget(), 1).await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), None, API_KEY);

    let first = invoke(&config, cache_dir.path(), &["status", "work"]);
    let second = invoke(&config, cache_dir.path(), &["status", "work", "--json"]);
    assert!(first.status.success());
    assert!(second.status.success());
    let report: Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(report["data"]["spend"]["cache"]["source"], "cache");
    assert!(report["data"]["spend"]["cache"]["age_seconds"].is_u64());
}

#[tokio::test]
async fn status_refresh_bypasses_the_budget_cache() {
    let server = MockServer::start().await;
    mount_models(&server, API_KEY, 2).await;
    mount_budget(&server, API_KEY, native_budget(), 2).await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), None, API_KEY);

    for args in [vec!["status", "work"], vec!["status", "work", "--refresh"]] {
        let output = invoke(&config, cache_dir.path(), &args);
        assert!(output.status.success());
    }
}

#[tokio::test]
async fn secondary_budget_timeout_uses_network_exit_code() {
    let server = MockServer::start().await;
    mount_models(&server, API_KEY, 1).await;
    Mock::given(method("GET"))
        .and(path("/v1/key/info"))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(2))
                .set_body_json(native_budget()),
        )
        .expect(1)
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), None, API_KEY);

    let output = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args(["--timeout", "100ms", "--config", config.to_str().unwrap()])
        .args(["status", "work"])
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
    mount_models(&server, "abcd", 1).await;
    mount_budget(
        &server,
        "abcd",
        json!({
            "key_spent_usd": "1.23",
            "remaining_usd": null,
            "limited_by": null,
            "layers": []
        }),
        1,
    )
    .await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), None, "abcd");

    let output = invoke(&config, cache_dir.path(), &["status", "work"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Key         redacted"), "{stdout}");
    assert!(!stdout.contains("abcd"), "short API key leaked: {stdout}");
}

#[tokio::test]
async fn status_auth_failure_does_not_echo_upstream_secrets() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "message": format!("credential {API_KEY} rejected at {}", server.uri()),
            "access_token": "private-upstream-token"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), None, API_KEY);

    let output = invoke(&config, cache_dir.path(), &["status", "work", "--json"]);
    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains(API_KEY));
    assert!(!stderr.contains("private-upstream-token"));
    assert!(!stderr.contains(&server.uri()));
}
