use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use chrono::{DateTime, Duration, Local, NaiveDate, Utc};
use serde_json::{json, Value};
use wiremock::MockServer;

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

#[test]
fn usage_rejects_invalid_or_conflicting_date_arguments() {
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

#[test]
fn chatgpt_usage_reports_local_events_and_filters_normalized_usage() {
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
    let output = run_local_usage(&config, &state, &date, &date, &[]);

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

    let output = run_local_usage(&config, &state, &date, &date, &[]);
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
async fn local_usage_filters_use_observed_events_without_http() {
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

#[tokio::test]
async fn api_key_usage_needs_no_gateway_or_key_secret_and_reports_unknown_cost_safely() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let today = Local::now().date_naive();
    let date = today.format("%Y-%m-%d").to_string();
    let config = dir.child("api-key.toml");
    config
        .write_str(&format!(
            "[endpoint]\nbase_url = '{}'\n\n[profiles.test]\napi_key = {{ env = 'AIX_USAGE_MISSING_KEY' }}\nbase_url = {{ env = 'AIX_USAGE_MISSING_PROFILE_URL' }}\n",
            server.uri()
        ))
        .unwrap();
    let state = dir.path().join("state");
    write_local_event(
        &state,
        today,
        LocalEventFixture {
            event_id: "00000000-0000-4000-8000-000000000007",
            profile: "test",
            tool: "codex",
            run_id: Some("local-api-key-run"),
            model: Some("model-api-key"),
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

    let output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "usage",
            "test",
            "--model",
            "model-api-key",
            "--tool",
            "codex",
            "--protocol",
            "anthropic_messages",
            "--run",
            "local-api-key-run",
            "--start",
            &date,
            "--end",
            &date,
            "--json",
        ])
        .env("AIX_STATE_DIR", &state)
        .env_remove("AIX_USAGE_MISSING_KEY")
        .env_remove("AIX_USAGE_MISSING_PROFILE_URL")
        .output()
        .unwrap();
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
    assert_eq!(data["profile"], "test");
    assert_eq!(data["start_date"], date);
    assert_eq!(data["end_date"], date);
    assert_eq!(data["metrics"]["request_count"], 1);
    assert_eq!(data["metrics"]["total_tokens"], 38);
    assert_eq!(data["model_filter"], "model-api-key");
    assert_eq!(data["models"][0]["model"], "model-api-key");
    assert_eq!(data["tools"][0]["tool"], "codex");
    assert_eq!(data["protocols"][0]["protocol"], "anthropic_messages");
    assert_eq!(data["runs"][0]["run_id"], "local-api-key-run");
    assert_eq!(data["billing"]["status"], "unavailable");
    assert!(data["billing"]["actual_spend"].is_null());
    assert!(!stdout.contains("private prompt content"));
    assert!(!stdout.contains("AIX_USAGE_MISSING_KEY"));
    assert!(!stdout.contains("AIX_USAGE_MISSING_PROFILE_URL"));
    assert!(server.received_requests().await.unwrap().is_empty());
}
