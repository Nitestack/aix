use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use chrono::{Duration, Local};
use predicates::prelude::*;
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn write_config(dir: &TempDir, base_url: &str, gateway: Option<&str>) -> std::path::PathBuf {
    let file = dir.child("aix.toml");
    let gateway = gateway.map_or(String::new(), |gateway| format!("gateway = \"{gateway}\""));
    file.write_str(&format!(
        r#"
[endpoint]
base_url = "{base_url}"
{gateway}

[profiles.test]
api_key = "usage-test-key"
"#
    ))
    .unwrap();
    file.path().to_path_buf()
}

fn usage_response() -> Value {
    json!({
        "results": [
            {
                "date": "2026-09-01",
                "metrics": {
                    "spend": 5.0,
                    "prompt_tokens": 100,
                    "completion_tokens": 50,
                    "total_tokens": 150,
                    "api_requests": 5
                },
                "breakdown": {
                    "models": {
                        "model-low": {
                            "spend": 1.25,
                            "prompt_tokens": 25,
                            "completion_tokens": 15,
                            "total_tokens": 40,
                            "api_requests": 2
                        },
                        "model-high": {
                            "spend": 3.75,
                            "prompt_tokens": 75,
                            "completion_tokens": 35,
                            "total_tokens": 110,
                            "api_requests": 3
                        }
                    },
                    "api_keys": { "raw-key-never-output-1234": { "spend": 5.0 } }
                }
            },
            {
                "date": "2026-09-02",
                "metrics": {
                    "spend": 3.5,
                    "prompt_tokens": 200,
                    "completion_tokens": 100,
                    "total_tokens": 300,
                    "api_requests": 4
                },
                "breakdown": {
                    "models": {
                        "model-low": {
                            "spend": 0.25,
                            "prompt_tokens": 10,
                            "completion_tokens": 5,
                            "total_tokens": 15,
                            "api_requests": 1
                        },
                        "model-high": {
                            "spend": 3.25,
                            "prompt_tokens": 190,
                            "completion_tokens": 95,
                            "total_tokens": 285,
                            "api_requests": 3
                        }
                    },
                    "api_keys": { "another-private-key": { "spend": 3.5 } }
                }
            }
        ],
        "metrics": {
            "spend": 8.5,
            "prompt_tokens": 300,
            "completion_tokens": 150,
            "total_tokens": 450,
            "request_count": 9
        },
        "api_keys": { "top-level-private-key": { "spend": 8.5 } }
    })
}

#[tokio::test]
async fn usage_default_range_is_last_30_local_calendar_days_including_today() {
    let server = MockServer::start().await;
    let today = Local::now().date_naive();
    let start = today - Duration::days(29);
    Mock::given(method("GET"))
        .and(path("/user/daily/activity"))
        .and(header("Authorization", "Bearer usage-test-key"))
        .and(query_param(
            "start_date",
            start.format("%Y-%m-%d").to_string(),
        ))
        .and(query_param(
            "end_date",
            today.format("%Y-%m-%d").to_string(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{
                "date": today.format("%Y-%m-%d").to_string(),
                "metrics": { "spend": 1.25 }
            }],
            "metrics": { "spend": 9.5 }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), None);
    Command::cargo_bin("aix")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "usage", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Spend today"))
        .stdout(predicate::str::contains("Spend last 7 days"))
        .stdout(predicate::str::contains("$9.50"));
}

#[tokio::test]
async fn usage_since_seven_days_includes_today_and_previous_six_days() {
    let server = MockServer::start().await;
    let today = Local::now().date_naive();
    let start = today - Duration::days(6);
    Mock::given(method("GET"))
        .and(path("/user/daily/activity"))
        .and(query_param(
            "start_date",
            start.format("%Y-%m-%d").to_string(),
        ))
        .and(query_param(
            "end_date",
            today.format("%Y-%m-%d").to_string(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [],
            "metadata": { "total_spend": 2.0 }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), None);
    let output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "usage",
            "test",
            "--since",
            "7d",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        parsed["data"]["start_date"],
        start.format("%Y-%m-%d").to_string()
    );
    assert_eq!(
        parsed["data"]["end_date"],
        today.format("%Y-%m-%d").to_string()
    );
}

#[tokio::test]
async fn usage_json_aggregates_daily_and_model_metrics_and_redacts_key_breakdowns() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/daily/activity"))
        .and(query_param("start_date", "2026-09-01"))
        .and(query_param("end_date", "2026-09-02"))
        .respond_with(ResponseTemplate::new(200).set_body_json(usage_response()))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), None);
    let output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--json",
            "--config",
            config.to_str().unwrap(),
            "usage",
            "test",
            "--start",
            "2026-09-01",
            "--end",
            "2026-09-02",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(parsed["command"], "usage");
    let data = &parsed["data"];
    assert_eq!(data["start_date"], "2026-09-01");
    assert_eq!(data["end_date"], "2026-09-02");
    assert_eq!(data["spend"], 8.5);
    assert_eq!(data["prompt_tokens"], 300);
    assert_eq!(data["completion_tokens"], 150);
    assert_eq!(data["total_tokens"], 450);
    assert_eq!(data["request_count"], 9);
    assert_eq!(data["daily"].as_array().unwrap().len(), 2);
    assert_eq!(data["models"][0]["model"], "model-high");
    assert_eq!(data["models"][0]["spend"], 7.0);
    assert_eq!(data["models"][1]["model"], "model-low");
    for secret in [
        "raw-key-never-output-1234",
        "another-private-key",
        "top-level-private-key",
        "api_keys",
    ] {
        assert!(
            !stdout.contains(secret),
            "unexpected upstream data {secret}: {stdout}"
        );
    }
}

#[tokio::test]
async fn usage_model_filter_reaggregates_from_model_breakdown_only() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/daily/activity"))
        .and(query_param("start_date", "2026-09-01"))
        .and(query_param("end_date", "2026-09-02"))
        .respond_with(ResponseTemplate::new(200).set_body_json(usage_response()))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), None);
    let output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "usage",
            "test",
            "--start",
            "2026-09-01",
            "--end",
            "2026-09-02",
            "--model",
            "model-low",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).unwrap();
    let data = &parsed["data"];
    assert_eq!(data["model_filter"], "model-low");
    assert_eq!(data["spend"], 1.5);
    assert_eq!(data["prompt_tokens"], 35);
    assert_eq!(data["completion_tokens"], 20);
    assert_eq!(data["total_tokens"], 55);
    assert_eq!(data["request_count"], 3);
    assert_eq!(data["models"].as_array().unwrap().len(), 1);
    assert_eq!(data["models"][0]["model"], "model-low");
    assert_eq!(data["daily"][0]["spend"], 1.25);
    assert_eq!(data["daily"][1]["spend"], 0.25);
}

#[tokio::test]
async fn usage_model_filter_does_not_create_rows_for_absent_models() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/daily/activity"))
        .respond_with(ResponseTemplate::new(200).set_body_json(usage_response()))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), None);
    let output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "usage",
            "test",
            "--start",
            "2026-09-01",
            "--end",
            "2026-09-02",
            "--model",
            "not-returned",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).unwrap();
    let data = &parsed["data"];
    assert_eq!(data["spend"], 0.0);
    assert!(data["models"].as_array().unwrap().is_empty());
    assert!(data["daily"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn usage_missing_optional_breakdowns_are_handled_safely() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/daily/activity"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{ "date": "2026-09-01", "metrics": { "spend": 1.0 } }],
            "metadata": { "total_spend": 1.0 }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), None);
    let output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "usage",
            "test",
            "--start",
            "2026-09-01",
            "--end",
            "2026-09-01",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).unwrap();
    let data = &parsed["data"];
    assert_eq!(data["spend"], 1.0);
    assert_eq!(data["prompt_tokens"], 0);
    assert_eq!(data["completion_tokens"], 0);
    assert_eq!(data["request_count"], 0);
    assert!(data["models"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn usage_rejects_invalid_or_conflicting_date_arguments() {
    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, "http://127.0.0.1:1", None);

    let invalid = [
        vec!["--since", "0d"],
        vec!["--since", "seven days"],
        vec!["--start", "2026-09-02", "--end", "2026-09-01"],
        vec!["--start", "2026-9-01", "--end", "2026-09-02"],
        vec!["--start", "2026-09-01"],
        vec![
            "--since",
            "7d",
            "--start",
            "2026-09-01",
            "--end",
            "2026-09-02",
        ],
    ];

    for args in invalid {
        let mut command = Command::cargo_bin("aix").unwrap();
        let output = command
            .arg("--config")
            .arg(config.to_str().unwrap())
            .arg("usage")
            .arg("test")
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}

#[tokio::test]
async fn usage_rejects_configured_non_litellm_gateway() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), Some("other"));
    let output = Command::cargo_bin("aix")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "usage", "test"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("LiteLLM"));
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn usage_unsupported_endpoint_is_distinct_from_auth_failure() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/daily/activity"))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .expect(1)
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), None);
    let output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "usage",
            "test",
            "--json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("usage history is unavailable"), "{stderr}");

    let auth_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/daily/activity"))
        .and(header("Authorization", "Bearer usage-test-key"))
        .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
        .expect(1)
        .mount(&auth_server)
        .await;
    let auth_config = write_config(&dir, &auth_server.uri(), None);
    let auth_output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            auth_config.to_str().unwrap(),
            "usage",
            "test",
            "--json",
        ])
        .output()
        .unwrap();

    assert_eq!(auth_output.status.code(), Some(4));
    assert!(auth_output.stdout.is_empty());
    let auth_stderr = String::from_utf8_lossy(&auth_output.stderr);
    assert!(auth_stderr.contains("HTTP 403"), "{auth_stderr}");
    assert!(!auth_stderr.contains("usage history is unavailable"));
}
