use assert_cmd::Command;
use assert_fs::prelude::*;
use serde_json::Value;

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

#[test]
#[cfg(unix)]
fn usage_summary_failure_does_not_hide_the_child_exit_status() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "sk-test-key"

[tools.sh]
command = "sh"
api_format = "openai"
local_gateway = true
"#,
        )
        .unwrap();
    let state = assert_fs::TempDir::new().unwrap();
    let usage_dir = state.path().join("usage");
    std::fs::create_dir_all(&usage_dir).unwrap();
    std::fs::write(usage_dir.join("events"), b"not-a-directory").unwrap();

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .args([
            "run",
            "--profile",
            "work",
            "--",
            "sh",
            "-c",
            "printf '%s\\n' \"$AIX_RUN_ID\"; exit 23",
        ])
        .assert()
        .code(23)
        .get_output()
        .clone();
    let run_id = String::from_utf8(output.stdout).unwrap().trim().to_string();
    assert!(String::from_utf8_lossy(&output.stderr).contains("run_usage_unavailable"));

    let record = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", &run_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let record: Value = serde_json::from_slice(&record).unwrap();
    assert_eq!(record["data"]["status"], "failed");
    assert_eq!(record["data"]["process_exit_code"], 23);
    assert!(record["data"].get("usage").is_none());
}

#[test]
fn human_run_listing_handles_mixed_legacy_and_usage_records() {
    let state = assert_fs::TempDir::new().unwrap();
    let legacy_id = "4b9a85df-51d9-49a4-9a17-69d7f0dc91f1";
    let usage_id = "4b9a85df-51d9-49a4-9a17-69d7f0dc91f2";
    let runs = state.path().join("runs");
    let legacy_dir = runs.join(legacy_id);
    let usage_dir = runs.join(usage_id);
    std::fs::create_dir_all(&legacy_dir).unwrap();
    std::fs::create_dir_all(&usage_dir).unwrap();
    std::fs::write(
        legacy_dir.join("00000000000000000001.json"),
        format!(
            r#"{{
  "schema_version": 1,
  "run_id": "{legacy_id}",
  "name": null,
  "workflow": null,
  "task_id": null,
  "tags": [],
  "profile": "work",
  "logical_tool_name": null,
  "executable_name": "sh",
  "started_at_unix_ms": 100,
  "finished_at_unix_ms": 101,
  "duration_ms": 1,
  "process_exit_code": 0,
  "status": "succeeded",
  "lease": null
}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        usage_dir.join("00000000000000000001.json"),
        format!(
            r#"{{
  "schema_version": 3,
  "run_id": "{usage_id}",
  "name": null,
  "workflow": null,
  "task_id": null,
  "tags": [],
  "profile": "work",
  "logical_tool_name": "review",
  "executable_name": "agent",
  "started_at_unix_ms": 200,
  "finished_at_unix_ms": 210,
  "duration_ms": 10,
  "process_exit_code": 0,
  "status": "succeeded",
  "lease": null,
  "usage": {{
    "request_count": 47,
    "successful_requests": 46,
    "failed_requests": 1,
    "input_tokens_total": 800000,
    "input_tokens_uncached": 700000,
    "cache_read_input_tokens": 100000,
    "cache_write_input_tokens": null,
    "output_tokens": 53000,
    "total_tokens": 853000,
    "models": ["model-a"],
    "protocols": ["openai_responses"]
  }}
}}"#
        ),
    )
    .unwrap();

    let output = cmd()
        .env("AIX_STATE_DIR", state.path())
        .arg("runs")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();
    let lines: Vec<_> = output.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("REQS  TOKENS"));
    assert!(lines[1].contains("47  853k"));
    assert!(lines[2].ends_with("-  -"));
}
