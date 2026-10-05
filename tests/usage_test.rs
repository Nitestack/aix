use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use chrono::{DateTime, Duration, Local, NaiveDate, Utc};
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

fn write_chatgpt_config(dir: &TempDir) -> std::path::PathBuf {
    let file = dir.child("chatgpt.toml");
    file.write_str("[profiles.personal]\nlabel = 'Personal'\nauth = { type = 'chatgpt' }\n")
        .unwrap();
    file.path().to_path_buf()
}

struct LocalEventFixture<'a> {
    event_id: &'a str,
    profile: &'a str,
    tool: &'a str,
    run_id: Option<&'a str>,
    model: Option<&'a str>,
    protocol: &'a str,
    outcome: &'a str,
    input_total: Option<u64>,
    input_uncached: Option<u64>,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
    output: Option<u64>,
    total: Option<u64>,
}

fn write_local_event(state_dir: &std::path::Path, date: NaiveDate, event: LocalEventFixture<'_>) {
    let started_at = date
        .and_hms_opt(12, 0, 0)
        .unwrap()
        .and_local_timezone(Local)
        .single()
        .unwrap()
        .timestamp_millis();
    let utc_date = DateTime::<Utc>::from_timestamp_millis(started_at)
        .unwrap()
        .format("%Y-%m-%d")
        .to_string();
    let directory = state_dir.join("usage/events").join(utc_date);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join(format!("{}.json", event.event_id)),
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "event_id": event.event_id,
            "started_at_unix_ms": started_at as u64,
            "finished_at_unix_ms": started_at as u64 + 10,
            "duration_ms": 10,
            "profile": event.profile,
            "logical_tool_name": event.tool,
            "run_id": event.run_id,
            "run_policy": null,
            "protocol": event.protocol,
            "model": event.model,
            "outcome": event.outcome,
            "http_status": if event.outcome == "succeeded" { 200 } else { 500 },
            "error_category": null,
            "input_tokens_total": event.input_total,
            "input_tokens_uncached": event.input_uncached,
            "cache_read_input_tokens": event.cache_read,
            "cache_write_input_tokens": event.cache_write,
            "output_tokens": event.output,
            "total_tokens": event.total,
            "request_count": 1,
            "usage_completeness": if event.input_total.is_some() && event.output.is_some() { "complete" } else { "unavailable" },
            "actual_cost_usd": null,
            "cost_source": null,
            "prompt": "private prompt content must never appear in reports"
        }))
        .unwrap(),
    )
    .unwrap();
}

fn run_local_usage(
    config: &std::path::Path,
    state_dir: &std::path::Path,
    start: &str,
    end: &str,
    filters: &[&str],
) -> std::process::Output {
    Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "usage",
            "personal",
            "--start",
            start,
            "--end",
            end,
            "--json",
        ])
        .args(filters)
        .env("AIX_STATE_DIR", state_dir)
        .output()
        .unwrap()
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
    assert_eq!(data["source"], "litellm");
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
        .args([
            "--config",
            config.to_str().unwrap(),
            "usage",
            "test",
            "--source",
            "litellm",
        ])
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

    let all_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/daily/activity"))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .expect(1)
        .mount(&all_server)
        .await;
    let all_config = write_config(&dir, &all_server.uri(), None);
    let all_output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            all_config.to_str().unwrap(),
            "usage",
            "test",
            "--source",
            "all",
            "--json",
        ])
        .env("AIX_STATE_DIR", dir.path().join("all-state"))
        .output()
        .unwrap();
    assert!(
        all_output.status.success(),
        "{}",
        String::from_utf8_lossy(&all_output.stderr)
    );
    let all: Value = serde_json::from_slice(&all_output.stdout).unwrap();
    assert_eq!(all["data"]["sources"]["local_gateway"]["status"], "empty");
    assert_eq!(all["data"]["sources"]["litellm"]["status"], "unsupported");

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

#[test]
fn chatgpt_usage_defaults_to_local_events_and_filters_normalized_usage() {
    let dir = TempDir::new().unwrap();
    let config = write_chatgpt_config(&dir);
    let state = dir.path().join("state");
    let today = Local::now().date_naive();
    let yesterday = today - Duration::days(1);
    let before_range = yesterday - Duration::days(1);

    write_local_event(
        &state,
        today,
        LocalEventFixture {
            event_id: "00000000-0000-4000-8000-000000000001",
            profile: "personal",
            tool: "opencode",
            run_id: Some("run-openai"),
            model: Some("gpt-test"),
            protocol: "openai_responses",
            outcome: "succeeded",
            input_total: Some(100),
            input_uncached: Some(70),
            cache_read: Some(30),
            cache_write: Some(0),
            output: Some(10),
            total: Some(110),
        },
    );
    write_local_event(
        &state,
        yesterday,
        LocalEventFixture {
            event_id: "00000000-0000-4000-8000-000000000002",
            profile: "personal",
            tool: "codex",
            run_id: Some("run-anthropic"),
            model: Some("claude-test"),
            protocol: "anthropic_messages",
            outcome: "failed",
            input_total: Some(50),
            input_uncached: Some(15),
            cache_read: Some(30),
            cache_write: Some(5),
            output: Some(20),
            total: Some(70),
        },
    );
    write_local_event(
        &state,
        before_range,
        LocalEventFixture {
            event_id: "00000000-0000-4000-8000-000000000003",
            profile: "personal",
            tool: "opencode",
            run_id: Some("old-run"),
            model: Some("gpt-test"),
            protocol: "openai_responses",
            outcome: "succeeded",
            input_total: Some(999),
            input_uncached: Some(999),
            cache_read: Some(0),
            cache_write: Some(0),
            output: Some(999),
            total: Some(1998),
        },
    );
    write_local_event(
        &state,
        today,
        LocalEventFixture {
            event_id: "00000000-0000-4000-8000-000000000004",
            profile: "other",
            tool: "opencode",
            run_id: Some("other-run"),
            model: Some("gpt-test"),
            protocol: "openai_responses",
            outcome: "succeeded",
            input_total: Some(999),
            input_uncached: Some(999),
            cache_read: Some(0),
            cache_write: Some(0),
            output: Some(999),
            total: Some(1998),
        },
    );

    let start = yesterday.format("%Y-%m-%d").to_string();
    let end = today.format("%Y-%m-%d").to_string();
    let output = run_local_usage(&config, &state, &start, &end, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: Value = serde_json::from_str(&stdout).unwrap();
    let data = &parsed["data"];
    assert_eq!(data["source"], "local_gateway");
    assert_eq!(data["status"], "available");
    assert_eq!(data["profile"], "personal");
    assert_eq!(data["metrics"]["request_count"], 2);
    assert_eq!(data["metrics"]["successful_requests"], 1);
    assert_eq!(data["metrics"]["failed_requests"], 1);
    assert_eq!(data["metrics"]["input_tokens_total"], 150);
    assert_eq!(data["metrics"]["input_tokens_uncached"], 85);
    assert_eq!(data["metrics"]["cache_read_input_tokens"], 60);
    assert_eq!(data["metrics"]["cache_write_input_tokens"], 5);
    assert_eq!(data["metrics"]["output_tokens"], 30);
    assert_eq!(data["metrics"]["total_tokens"], 180);
    assert_eq!(data["daily"].as_array().unwrap().len(), 2);
    assert_eq!(data["models"].as_array().unwrap().len(), 2);
    assert_eq!(data["tools"].as_array().unwrap().len(), 2);
    assert_eq!(data["protocols"].as_array().unwrap().len(), 2);
    assert_eq!(data["runs"].as_array().unwrap().len(), 2);
    assert_eq!(data["billing"]["status"], "not_applicable");
    assert!(data["billing"]["actual_spend"].is_null());
    assert!(!stdout.contains("private prompt content"));
    assert!(!stdout.contains(state.to_str().unwrap()));

    for filter in [
        vec!["--model", "gpt-test"],
        vec!["--tool", "codex"],
        vec!["--run", "run-anthropic"],
        vec!["--protocol", "anthropic_messages"],
    ] {
        let filtered = run_local_usage(&config, &state, &start, &end, &filter);
        assert!(
            filtered.status.success(),
            "{}",
            String::from_utf8_lossy(&filtered.stderr)
        );
        let data: Value = serde_json::from_slice(&filtered.stdout).unwrap();
        assert_eq!(data["data"]["metrics"]["request_count"], 1);
        let (breakdown, dimension) = match filter[0] {
            "--model" => ("models", "model"),
            "--tool" => ("tools", "tool"),
            "--run" => ("runs", "run_id"),
            _ => ("protocols", "protocol"),
        };
        assert_eq!(data["data"][breakdown][0][dimension], filter[1]);
    }
}

#[test]
fn local_usage_without_events_returns_a_safe_empty_report() {
    let dir = TempDir::new().unwrap();
    let config = write_chatgpt_config(&dir);
    let state = dir.path().join("empty-state");
    let today = Local::now().date_naive();
    let date = today.format("%Y-%m-%d").to_string();
    let output = run_local_usage(&config, &state, &date, &date, &["--source", "local"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["data"]["status"], "empty");
    assert_eq!(parsed["data"]["metrics"]["request_count"], 0);
    assert_eq!(parsed["data"]["billing"]["status"], "not_applicable");
    assert!(stdout.contains("No local usage events"));
    assert!(!stdout.contains(state.to_str().unwrap()));
}

#[test]
fn local_usage_marks_malformed_event_neighbors_as_partial_without_leaking_paths() {
    let dir = TempDir::new().unwrap();
    let config = write_chatgpt_config(&dir);
    let state = dir.path().join("state");
    let today = Local::now().date_naive();
    let date = today.format("%Y-%m-%d").to_string();
    write_local_event(
        &state,
        today,
        LocalEventFixture {
            event_id: "00000000-0000-4000-8000-000000000008",
            profile: "personal",
            tool: "opencode",
            run_id: Some("partial-run"),
            model: Some("gpt-test"),
            protocol: "openai_responses",
            outcome: "succeeded",
            input_total: Some(10),
            input_uncached: Some(10),
            cache_read: Some(0),
            cache_write: Some(0),
            output: Some(4),
            total: Some(14),
        },
    );
    let event_day = std::fs::read_dir(state.join("usage/events"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(event_day.join("malformed.json"), b"not-json").unwrap();

    let output = run_local_usage(&config, &state, &date, &date, &["--source", "local"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["data"]["status"], "partial");
    assert_eq!(parsed["data"]["metrics"]["request_count"], 1);
    assert!(parsed["data"]["message"]
        .as_str()
        .unwrap()
        .contains("totals may be incomplete"));
    assert!(!stdout.contains(state.to_str().unwrap()));
}

#[tokio::test]
async fn usage_all_keeps_local_and_litellm_metrics_in_separate_sections() {
    let server = MockServer::start().await;
    let today = Local::now().date_naive();
    let date = today.format("%Y-%m-%d").to_string();
    Mock::given(method("GET"))
        .and(path("/user/daily/activity"))
        .and(query_param("start_date", date.clone()))
        .and(query_param("end_date", date.clone()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "metrics": { "spend": 4.25, "prompt_tokens": 900, "completion_tokens": 100, "total_tokens": 1000, "request_count": 9 }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), None);
    let state = dir.path().join("state");
    write_local_event(
        &state,
        today,
        LocalEventFixture {
            event_id: "00000000-0000-4000-8000-000000000005",
            profile: "test",
            tool: "opencode",
            run_id: Some("run-all"),
            model: Some("gpt-test"),
            protocol: "openai_responses",
            outcome: "succeeded",
            input_total: Some(20),
            input_uncached: Some(20),
            cache_read: Some(0),
            cache_write: Some(0),
            output: Some(5),
            total: Some(25),
        },
    );
    let output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "usage",
            "test",
            "--source",
            "all",
            "--start",
            &date,
            "--end",
            &date,
            "--json",
        ])
        .env("AIX_STATE_DIR", &state)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["data"]["source"], "all");
    assert_eq!(
        parsed["data"]["sources"]["local_gateway"]["metrics"]["total_tokens"],
        25
    );
    assert_eq!(parsed["data"]["sources"]["litellm"]["total_tokens"], 1000);
    assert_eq!(parsed["data"]["sources"]["litellm"]["spend"], 4.25);
    assert!(parsed["data"].get("total_tokens").is_none());
}

#[tokio::test]
async fn source_all_keeps_local_report_when_litellm_credentials_are_unavailable() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let config = dir.child("missing-key.toml");
    config
        .write_str(&format!(
            "[endpoint]\nbase_url = '{}'\n\n[profiles.test]\napi_key = {{ env = 'AIX_USAGE_TEST_MISSING_KEY' }}\n",
            server.uri()
        ))
        .unwrap();
    let state = dir.path().join("state");
    let today = Local::now().date_naive();
    let date = today.format("%Y-%m-%d").to_string();
    write_local_event(
        &state,
        today,
        LocalEventFixture {
            event_id: "00000000-0000-4000-8000-000000000009",
            profile: "test",
            tool: "opencode",
            run_id: Some("available-local-run"),
            model: Some("gpt-test"),
            protocol: "openai_responses",
            outcome: "succeeded",
            input_total: Some(11),
            input_uncached: Some(11),
            cache_read: Some(0),
            cache_write: Some(0),
            output: Some(2),
            total: Some(13),
        },
    );

    let output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.path().to_str().unwrap(),
            "usage",
            "test",
            "--source",
            "all",
            "--start",
            &date,
            "--end",
            &date,
            "--json",
        ])
        .env_remove("AIX_USAGE_TEST_MISSING_KEY")
        .env("AIX_STATE_DIR", &state)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        parsed["data"]["sources"]["local_gateway"]["metrics"]["request_count"],
        1
    );
    assert_eq!(
        parsed["data"]["sources"]["litellm"]["status"],
        "unavailable"
    );
    assert!(!stdout.contains("AIX_USAGE_TEST_MISSING_KEY"));
    assert!(!stdout.contains(state.to_str().unwrap()));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn local_only_filter_selects_local_events_without_calling_litellm() {
    let server = MockServer::start().await;
    let today = Local::now().date_naive();
    let date = today.format("%Y-%m-%d").to_string();
    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), None);
    let state = dir.path().join("state");
    write_local_event(
        &state,
        today,
        LocalEventFixture {
            event_id: "00000000-0000-4000-8000-000000000006",
            profile: "test",
            tool: "opencode",
            run_id: Some("local-only-run"),
            model: Some("gpt-local"),
            protocol: "openai_responses",
            outcome: "succeeded",
            input_total: Some(12),
            input_uncached: Some(12),
            cache_read: Some(0),
            cache_write: Some(0),
            output: Some(3),
            total: Some(15),
        },
    );
    let output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "usage",
            "test",
            "--tool",
            "opencode",
            "--start",
            &date,
            "--end",
            &date,
            "--json",
        ])
        .env("AIX_STATE_DIR", &state)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["data"]["source"], "local_gateway");
    assert_eq!(parsed["data"]["metrics"]["request_count"], 1);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test]
fn non_litellm_auto_source_uses_local_events_and_explains_empty_results() {
    let dir = TempDir::new().unwrap();
    let today = Local::now().date_naive();
    let date = today.format("%Y-%m-%d").to_string();
    let config = write_config(&dir, "http://127.0.0.1:1", Some("other"));
    let state = dir.path().join("state");
    write_local_event(
        &state,
        today,
        LocalEventFixture {
            event_id: "00000000-0000-4000-8000-000000000007",
            profile: "test",
            tool: "codex",
            run_id: Some("custom-gateway-run"),
            model: Some("custom-model"),
            protocol: "anthropic_messages",
            outcome: "succeeded",
            input_total: Some(30),
            input_uncached: Some(25),
            cache_read: Some(5),
            cache_write: Some(0),
            output: Some(8),
            total: Some(38),
        },
    );

    let available = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "usage",
            "test",
            "--start",
            &date,
            "--end",
            &date,
            "--json",
        ])
        .env("AIX_STATE_DIR", &state)
        .output()
        .unwrap();
    assert!(available.status.success());
    let parsed: Value = serde_json::from_slice(&available.stdout).unwrap();
    assert_eq!(parsed["data"]["source"], "local_gateway");
    assert_eq!(parsed["data"]["metrics"]["request_count"], 1);

    let empty_state = dir.path().join("empty-state");
    let empty = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "usage",
            "test",
            "--start",
            &date,
            "--end",
            &date,
            "--json",
        ])
        .env("AIX_STATE_DIR", &empty_state)
        .output()
        .unwrap();
    assert!(empty.status.success());
    let parsed: Value = serde_json::from_slice(&empty.stdout).unwrap();
    assert_eq!(parsed["data"]["status"], "empty");
    assert_eq!(parsed["data"]["metrics"]["request_count"], 0);
    assert!(parsed["data"]["message"]
        .as_str()
        .unwrap()
        .contains("does not provide LiteLLM"));
}

#[test]
fn litellm_source_rejects_local_attribution_filters_with_guidance() {
    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, "http://127.0.0.1:1", None);
    let output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "usage",
            "test",
            "--source",
            "litellm",
            "--tool",
            "opencode",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--source local"));
}
