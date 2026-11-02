use assert_cmd::assert::OutputAssertExt;
use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const API_KEY: &str = "sk-native-budget-secret-0011";

fn write_config(dir: &TempDir, base_url: &str, api_key: &str) -> std::path::PathBuf {
    let file = dir.child("aix.toml");
    file.write_str(&format!(
        r#"
[endpoint]
base_url = "{base_url}"

[profiles.test]
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
        "key": "raw-server-key-secret",
        "key_id": "private-key-id-secret",
        "name": "private-name-secret",
        "metadata": { "api_key": "nested-key-secret" }
    })
}

async fn mount_budget(server: &MockServer, body: Value, expected: u64) {
    Mock::given(method("GET"))
        .and(path("/v1/key/info"))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
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
async fn spend_json_preserves_the_original_spend_and_max_budget_fields() {
    let server = MockServer::start().await;
    mount_budget(&server, native_budget(), 1).await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), API_KEY);

    let output = invoke(&config, cache_dir.path(), &["spend", "test", "--json"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["command"], "spend");
    assert_eq!(report["data"], json!({"spend": 8.0, "max_budget": 20.0}));
    for secret in [
        API_KEY,
        "raw-server-key-secret",
        "private-key-id-secret",
        "private-name-secret",
        "nested-key-secret",
        "metadata",
    ] {
        assert!(!stdout.contains(secret), "leaked {secret}: {stdout}");
    }
}

#[tokio::test]
async fn spend_maps_a_key_limit_without_fabricating_or_expanding_the_summary() {
    let server = MockServer::start().await;
    let mut body = native_budget();
    body["limited_by"] = json!("key");
    body["remaining_usd"] = json!("58.47");
    mount_budget(&server, body, 1).await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), API_KEY);

    let output = invoke(&config, cache_dir.path(), &["spend", "test", "--json"]);
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["data"], json!({"spend": 41.53, "max_budget": 100.0}));
}

#[tokio::test]
async fn missing_limiting_layer_fails_instead_of_inventing_a_budget() {
    let server = MockServer::start().await;
    let mut body = native_budget();
    body["layers"] = json!([]);
    mount_budget(&server, body, 1).await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), API_KEY);

    let output = invoke(&config, cache_dir.path(), &["spend", "test", "--json"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("limiting budget layer"));
}

#[tokio::test]
async fn spend_human_output_is_only_the_original_summary_and_single_bar() {
    let server = MockServer::start().await;
    mount_budget(&server, native_budget(), 1).await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), API_KEY);

    let expected = format!(
        "$8.00 of $20.00  ·  $12.00 available (60%)\n[{}{}]  40% used\n",
        "█".repeat(16),
        "░".repeat(24),
    );
    invoke(&config, cache_dir.path(), &["spend", "test"])
        .assert()
        .success()
        .stdout(expected);
}

#[tokio::test]
async fn spend_shows_only_one_progress_bar_for_the_limiting_budget() {
    let server = MockServer::start().await;
    mount_budget(&server, native_budget(), 1).await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), API_KEY);

    let output = invoke(&config, cache_dir.path(), &["spend", "test"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("42% used"), "{stdout}");
    assert!(stdout.contains("40% used"), "{stdout}");
    assert_eq!(
        stdout.matches('░').count() + stdout.matches('█').count(),
        40
    );
    assert!(
        !stdout.contains('\u{1b}'),
        "redirected output must not contain ANSI codes"
    );
}

#[tokio::test]
async fn spend_omits_an_unlimited_budget_and_displays_the_original_no_budget_summary() {
    let server = MockServer::start().await;
    let mut body = native_budget();
    body["remaining_usd"] = Value::Null;
    body["limited_by"] = Value::Null;
    body["layers"] = json!([{
        "scope": "tenant",
        "budget_usd": null,
        "spent_usd": "41.53",
        "remaining_usd": null,
        "period_start": "2026-11-01T00:00:00Z"
    }]);
    mount_budget(&server, body, 2).await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), API_KEY);

    let output = invoke(&config, cache_dir.path(), &["spend", "test", "--json"]);
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["data"], json!({"spend": 41.53}));

    let human = invoke(&config, cache_dir.path(), &["spend", "test", "--no-cache"]);
    assert!(human.status.success());
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert_eq!(stdout, "$41.53 spent  (no budget set)\n");
}

#[tokio::test]
async fn spend_reports_unsupported_endpoints_as_gateway_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/key/info"))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .expect(1)
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), API_KEY);

    let output = invoke(&config, cache_dir.path(), &["spend", "test", "--json"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("HTTP 404"));
}

#[tokio::test]
async fn malformed_native_fields_fail_as_protocol_errors_instead_of_zeroes() {
    let server = MockServer::start().await;
    let mut body = native_budget();
    body["key_spent_usd"] = json!("NaN");
    mount_budget(&server, body, 1).await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), API_KEY);

    let output = invoke(&config, cache_dir.path(), &["spend", "test", "--json"]);
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
async fn cache_is_key_scoped_secret_safe_and_legacy_records_are_refetched() {
    let server = MockServer::start().await;
    mount_budget(&server, native_budget(), 2).await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), API_KEY);

    let first = invoke(&config, cache_dir.path(), &["spend", "test", "--json"]);
    assert!(first.status.success());

    let cache_file = std::fs::read_dir(cache_dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .flat_map(|entry| {
            std::fs::read_dir(entry.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name() != "index.json")
                .map(|entry| entry.path())
                .collect::<Vec<_>>()
        })
        .next()
        .expect("budget cache data file");
    let mut cached: Value =
        serde_json::from_str(&std::fs::read_to_string(&cache_file).unwrap()).unwrap();
    cached["data"] = json!({ "spend": 41.53, "max_budget": 100.0 });
    std::fs::write(&cache_file, serde_json::to_vec(&cached).unwrap()).unwrap();

    let second = invoke(&config, cache_dir.path(), &["spend", "test", "--json"]);
    assert!(second.status.success());
    let report: Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(report["data"], json!({"spend": 8.0, "max_budget": 20.0}));

    for entry in std::fs::read_dir(cache_dir.path()).unwrap() {
        let endpoint = entry.unwrap().path();
        if endpoint.is_dir() {
            for file in std::fs::read_dir(endpoint).unwrap() {
                let contents = std::fs::read_to_string(file.unwrap().path()).unwrap();
                for private in [
                    API_KEY,
                    "raw-server-key-secret",
                    "private-key-id-secret",
                    "private-name-secret",
                    "nested-key-secret",
                    "metadata",
                ] {
                    assert!(!contents.contains(private), "cached {private}: {contents}");
                }
            }
        }
    }
}

#[tokio::test]
async fn cache_hit_uses_no_second_budget_request() {
    let server = MockServer::start().await;
    mount_budget(&server, native_budget(), 1).await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), API_KEY);

    let first = invoke(&config, cache_dir.path(), &["spend", "test"]);
    let second = invoke(&config, cache_dir.path(), &["spend", "test"]);
    assert!(first.status.success());
    assert!(second.status.success());
    assert!(String::from_utf8_lossy(&second.stdout).contains("cached"));
}

#[tokio::test]
async fn spend_auth_failure_does_not_leak_credentials_or_upstream_metadata() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/key/info"))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "message": format!("credential {API_KEY} rejected"),
            "access_token": "upstream-token-secret"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), API_KEY);

    let output = invoke(&config, cache_dir.path(), &["spend", "test", "--json"]);
    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("403"), "{stderr}");
    assert!(!stderr.contains(API_KEY), "leaked API key: {stderr}");
    assert!(
        !stderr.contains("upstream-token-secret"),
        "leaked token: {stderr}"
    );
}

#[tokio::test]
async fn no_cache_forces_each_request() {
    let server = MockServer::start().await;
    mount_budget(&server, native_budget(), 2).await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), API_KEY);

    for _ in 0..2 {
        let output = invoke(&config, cache_dir.path(), &["spend", "test", "--no-cache"]);
        assert!(output.status.success());
    }
}
