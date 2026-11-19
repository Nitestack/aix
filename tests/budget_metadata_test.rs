//! Budget metadata consumed by external callers: project ids, key
//! fingerprints, reset times, legacy LiteLLM `/key/info`, `--max-age`, and the
//! gateway kind reported by `status`, `spend`, and `env`.

use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const API_KEY: &str = "sk-budget-metadata-secret-7788";

fn write_config(dir: &TempDir, base_url: &str) -> std::path::PathBuf {
    let file = dir.child("aix.toml");
    file.write_str(&format!(
        r#"
[endpoint]
base_url = "{base_url}"

[profiles.test]
api_key = "{API_KEY}"
"#
    ))
    .unwrap();
    file.path().to_path_buf()
}

fn native_budget() -> Value {
    json!({
        "key_id": "private-key-id",
        "kind": "project",
        "project_id": "proj-123",
        "source": "aihub",
        "key_spent_usd": "4.25",
        "remaining_usd": "15.75",
        "limited_by": "project",
        "layers": [
            {
                "scope": "project",
                "budget_usd": "20.00",
                "spent_usd": "4.25",
                "remaining_usd": "15.75",
                "period_start": "2026-11-01T00:00:00Z"
            },
            {
                "scope": "key",
                "budget_usd": "50.00",
                "spent_usd": "4.25",
                "remaining_usd": "45.75",
                "period_start": "2026-11-10T00:00:00Z",
                "budget_duration": "7d"
            }
        ],
        "expires_at": null
    })
}

async fn mount(server: &MockServer, route: &str, status: u16, body: Value, expected: u64) {
    Mock::given(method("GET"))
        .and(path(route))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .expect(expected)
        .mount(server)
        .await;
}

fn invoke(config: &std::path::Path, cache_dir: &std::path::Path, args: &[&str]) -> Value {
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir)
        .env_remove("AIX_PROFILE")
        .args(["--config", config.to_str().unwrap()])
        .args(args)
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{args:?}: {stderr}");
    // `env` exists to export the key; every other command must not print it.
    if args[0] != "env" {
        assert!(!stdout.contains(API_KEY), "leaked key: {stdout}");
    }
    assert!(
        !stdout.contains("private-key-id"),
        "leaked key id: {stdout}"
    );
    serde_json::from_str(&stdout).unwrap_or_else(|_| json!({ "raw": stdout }))
}

#[tokio::test]
async fn spend_json_reports_the_full_budget_without_a_gateway_probe() {
    let server = MockServer::start().await;
    mount(&server, "/v1/key/info", 200, native_budget(), 1).await;
    // `spend` never probes the gateway.
    mount(&server, "/v1/models", 200, json!({ "data": [] }), 0).await;
    let dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    let report = invoke(&config, cache.path(), &["spend", "test", "--json"]);
    let data = &report["data"];

    assert_eq!(data["spend"], 4.25);
    assert_eq!(data["max_budget"], 20.0);
    assert_eq!(data["status"], "available");
    assert_eq!(data["key_spent_usd"], 4.25);
    assert_eq!(data["remaining_usd"], 15.75);
    assert_eq!(data["limited_by"], "project");
    assert_eq!(data["project_id"], "proj-123");
    assert_eq!(data["cache"]["source"], "live");
    assert!(data["cache"]["age_seconds"].is_null());
    assert!(data["cache"]["fetched_at"].is_string());
    assert_eq!(data["profile"], json!({ "name": "test", "label": "test" }));
    assert_eq!(data["gateway"]["gateway"], "aihub");
    assert_eq!(data["gateway"]["detected_from"], "response");

    let fingerprint = data["key_fingerprint"].as_str().unwrap();
    assert_eq!(fingerprint.len(), 64);
    assert!(fingerprint.chars().all(|c| c.is_ascii_hexdigit()));

    let project = &data["layers"][0];
    assert_eq!(project["resets_at"], "2026-12-01T00:00:00Z");
    assert_eq!(project["budget_duration"], "1mo");
    assert_eq!(project["reset_source"], "inferred_monthly");
    let key = &data["layers"][1];
    assert_eq!(key["resets_at"], "2026-11-17T00:00:00Z");
    assert_eq!(key["budget_duration"], "7d");
    assert_eq!(key["reset_source"], "gateway");
}

#[tokio::test]
async fn status_and_spend_share_the_fingerprint_and_status_names_the_gateway() {
    let server = MockServer::start().await;
    mount(&server, "/v1/key/info", 200, native_budget(), 1).await;
    mount(&server, "/v1/models", 200, json!({ "data": [] }), 1).await;
    let dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    let status = invoke(&config, cache.path(), &["status", "test", "--json"]);
    let spend = invoke(&config, cache.path(), &["spend", "test", "--json"]);

    assert_eq!(
        status["data"]["key_fingerprint"],
        spend["data"]["key_fingerprint"]
    );
    assert_eq!(status["data"]["gateway"]["gateway"], "aihub");
    assert_eq!(status["data"]["spend"]["project_id"], "proj-123");
    assert_eq!(spend["data"]["cache"]["source"], "cache");
}

#[tokio::test]
async fn max_age_uses_young_cache_and_refetches_old_cache() {
    let server = MockServer::start().await;
    mount(&server, "/v1/key/info", 200, native_budget(), 2).await;
    let dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    let first = invoke(&config, cache.path(), &["spend", "test", "--json"]);
    assert_eq!(first["data"]["cache"]["source"], "live");

    let young = invoke(
        &config,
        cache.path(),
        &["spend", "test", "--json", "--max-age", "1h"],
    );
    assert_eq!(young["data"]["cache"]["source"], "cache");

    let forced = invoke(
        &config,
        cache.path(),
        &["spend", "test", "--json", "--max-age", "0"],
    );
    assert_eq!(forced["data"]["cache"]["source"], "live");
}

#[tokio::test]
async fn refresh_is_accepted_on_spend_and_no_cache_on_status() {
    let server = MockServer::start().await;
    mount(&server, "/v1/key/info", 200, native_budget(), 2).await;
    mount(&server, "/v1/models", 200, json!({ "data": [] }), 1).await;
    let dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    let spend = invoke(
        &config,
        cache.path(),
        &["spend", "test", "--json", "--refresh"],
    );
    assert_eq!(spend["data"]["cache"]["source"], "live");
    let status = invoke(
        &config,
        cache.path(),
        &["status", "test", "--json", "--no-cache"],
    );
    assert_eq!(status["data"]["spend"]["cache"]["source"], "live");
}

#[tokio::test]
async fn legacy_litellm_key_info_is_normalized_into_a_key_layer() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/v1/key/info",
        404,
        json!({ "detail": "Not Found" }),
        1,
    )
    .await;
    mount(
        &server,
        "/key/info",
        200,
        json!({
            "key": "raw-key-secret",
            "info": {
                "spend": 3.5,
                "max_budget": 10,
                "budget_duration": "30d",
                "budget_reset_at": "2026-12-01T00:00:00",
                "team_id": "team-1",
                "token": "hashed-token-secret"
            }
        }),
        1,
    )
    .await;
    let dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    let report = invoke(&config, cache.path(), &["spend", "test", "--json"]);
    let data = &report["data"];
    let text = report.to_string();
    assert!(!text.contains("raw-key-secret"), "{text}");
    assert!(!text.contains("hashed-token-secret"), "{text}");

    assert_eq!(data["spend"], 3.5);
    assert_eq!(data["max_budget"], 10.0);
    assert_eq!(data["limited_by"], "key");
    assert_eq!(data["remaining_usd"], 6.5);
    assert_eq!(data["gateway"]["gateway"], "litellm");
    let layer = &data["layers"][0];
    assert_eq!(layer["scope"], "key");
    assert_eq!(layer["resets_at"], "2026-12-01T00:00:00Z");
    assert_eq!(layer["budget_duration"], "30d");
    assert_eq!(layer["reset_source"], "gateway");
    assert_eq!(layer["period_start"], "2026-11-01T00:00:00Z");
}

#[tokio::test]
async fn missing_legacy_endpoint_still_reports_unsupported() {
    let server = MockServer::start().await;
    mount(&server, "/v1/key/info", 404, json!({}), 1).await;
    mount(&server, "/key/info", 404, json!({}), 1).await;
    let dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    let report = invoke(&config, cache.path(), &["spend", "test", "--json"]);
    assert_eq!(report["data"]["status"], "unsupported");
}

#[tokio::test]
async fn env_reports_the_cached_gateway_kind_without_contacting_the_gateway() {
    let server = MockServer::start().await;
    mount(&server, "/v1/key/info", 200, native_budget(), 1).await;
    let dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    let before = invoke(&config, cache.path(), &["env", "test", "--format", "json"]);
    assert!(before.get("AIX_GATEWAY_KIND").is_none(), "{before}");

    invoke(&config, cache.path(), &["spend", "test", "--json"]);
    let after = invoke(&config, cache.path(), &["env", "test", "--format", "json"]);
    assert_eq!(after["AIX_GATEWAY_KIND"], "aihub");
}
