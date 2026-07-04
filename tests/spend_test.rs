use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use predicates::prelude::*;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn write_config(dir: &TempDir, base_url: &str) -> std::path::PathBuf {
    let file = dir.child("aix.toml");
    file.write_str(&format!(
        r#"
[endpoint]
base_url = "{base_url}"
gateway = "litellm"

[profiles.test]
api_key = "sk-test-key"
"#
    ))
    .unwrap();
    file.path().to_path_buf()
}

#[tokio::test]
async fn spend_shows_model_and_cost() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/spend/logs"))
        .and(header("Authorization", "Bearer sk-test-key"))
        .and(query_param("limit", "50"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "request_id": "req-abc123-xyz",
                "model": "gpt-4o",
                "spend": 0.0042,
                "startTime": "2026-07-04T10:00:00Z"
            }
        ])))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    Command::cargo_bin("aix")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "spend", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("gpt-4o"))
        .stdout(predicate::str::contains("0.0042"));
}

#[tokio::test]
async fn spend_limit_flag_forwarded_as_query_param() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/spend/logs"))
        .and(query_param("limit", "10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "spend",
            "test",
            "--limit",
            "10",
        ])
        .assert()
        .success();
}

#[tokio::test]
async fn spend_json_flag_returns_raw_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/spend/logs"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([{"model": "claude-sonnet-4-6"}])),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    let output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "spend",
            "test",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed[0]["model"], "claude-sonnet-4-6");
}

#[tokio::test]
async fn spend_empty_logs_prints_no_spend_logs() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/spend/logs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    Command::cargo_bin("aix")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "spend", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no spend logs"));
}
