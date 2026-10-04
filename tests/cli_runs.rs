use assert_cmd::Command;
use assert_fs::prelude::*;
use serde_json::Value;
use std::time::{Duration, Instant};

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

#[test]
#[cfg(unix)]
fn run_passes_metadata_to_child_and_persists_a_success_record() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://ai.example.com"

[profiles.work]
api_key = "sk-test-key"
"#,
        )
        .unwrap();
    let state = assert_fs::TempDir::new().unwrap();

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .args([
            "run",
            "--profile",
            "work",
            "--name",
            "nightly check",
            "--workflow",
            "verification",
            "--task-id",
            "ticket-14",
            "--tag",
            "nightly",
            "--tag",
            "ci",
            "--",
            "sh",
            "-c",
            "printf '%s\\n' \"$AIX_RUN_ID\" \"$AIX_RUN_NAME\" \"$AIX_WORKFLOW\" \"$AIX_TASK_ID\" \"$AIX_RUN_TAGS\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let child_output = String::from_utf8(output).unwrap();
    let lines: Vec<_> = child_output.lines().collect();
    assert_eq!(lines.len(), 5);
    let run_id = lines[0];
    assert_eq!(run_id.len(), 36);
    assert!(uuid::Uuid::parse_str(run_id).is_ok());
    assert_eq!(lines[1], "nightly check");
    assert_eq!(lines[2], "verification");
    assert_eq!(lines[3], "ticket-14");
    assert_eq!(lines[4], r#"["nightly","ci"]"#);

    let record_output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", run_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let envelope: Value = serde_json::from_slice(&record_output).unwrap();
    let record = &envelope["data"];
    assert_eq!(envelope["schema_version"], 1);
    assert_eq!(envelope["command"], "runs show");
    assert_eq!(record["schema_version"], 1);
    assert_eq!(record["run_id"], run_id);
    assert_eq!(record["name"], "nightly check");
    assert_eq!(record["workflow"], "verification");
    assert_eq!(record["task_id"], "ticket-14");
    assert_eq!(record["tags"], serde_json::json!(["nightly", "ci"]));
    assert_eq!(record["profile"], "work");
    assert_eq!(record["executable_name"], "sh");
    assert_eq!(record["status"], "succeeded");
    assert!(record["duration_ms"].is_number());
    assert!(record["finished_at_unix_ms"].is_number());
    assert_eq!(record["process_exit_code"], 0);
}

#[test]
#[cfg(unix)]
fn arbitrary_command_gets_both_credential_formats_and_keeps_exec_independent() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "sk-private-key"
"#,
        )
        .unwrap();
    let state = assert_fs::TempDir::new().unwrap();

    cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_RUN_NAME", "stale-name")
        .env("AIX_WORKFLOW", "stale-workflow")
        .env("AIX_TASK_ID", "stale-task")
        .env("AIX_RUN_TAGS", "[\"stale\"]")
        .args([
            "run",
            "--profile",
            "work",
            "--",
            "sh",
            "-c",
            "test \"$ANTHROPIC_API_KEY\" = sk-private-key && test \"$OPENAI_API_KEY\" = sk-private-key && test \"$LITELLM_API_KEY\" = sk-private-key && test \"$AIX_PROFILE\" = work && test -z \"${AIX_RUN_NAME+x}\" && test -z \"${AIX_WORKFLOW+x}\" && test -z \"${AIX_TASK_ID+x}\" && test -z \"${AIX_RUN_TAGS+x}\"",
        ])
        .assert()
        .success();

    cmd()
        .env("AIX_CONFIG", config.path())
        .args([
            "exec",
            "work",
            "--",
            "sh",
            "-c",
            "test -n \"$OPENAI_API_KEY\"",
        ])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn configured_tool_wiring_applies_with_tool_environment_precedence() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "sk-profile-key"

[profiles.work.env]
RUN_LAYER = "profile"

[tools.review]
command = "sh"
api_format = "openai"

[tools.review.env]
OPENAI_API_KEY = "tool-secret"
RUN_LAYER = "tool"
"#,
        )
        .unwrap();
    let state = assert_fs::TempDir::new().unwrap();

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("ANTHROPIC_API_KEY", "ambient-secret")
        .env("ANTHROPIC_BASE_URL", "https://ambient.invalid")
        .args([
            "run",
            "--profile",
            "work",
            "--",
            "review",
            "-c",
            "test \"$OPENAI_API_KEY\" = tool-secret && test \"$RUN_LAYER\" = tool && test -z \"${ANTHROPIC_API_KEY+x}\" && test -z \"${ANTHROPIC_BASE_URL+x}\"; printf '%s\\n' \"$AIX_RUN_ID\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let run_id = String::from_utf8(output).unwrap().trim().to_string();

    let record_output = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", &run_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let envelope: Value = serde_json::from_slice(&record_output).unwrap();
    assert_eq!(envelope["data"]["logical_tool_name"], "review");
    assert_eq!(envelope["data"]["executable_name"], "sh");
    let serialized = envelope.to_string();
    assert!(!serialized.contains("tool-secret"));
    assert!(!serialized.contains("sk-profile-key"));
}

#[test]
#[cfg(unix)]
fn run_routes_configured_tool_credentials_through_the_local_gateway() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "sk-parent-profile-secret"

[tools.review]
command = "sh"
api_format = "both"
local_gateway = true
"#,
        )
        .unwrap();
    let state = assert_fs::TempDir::new().unwrap();

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .args([
            "run",
            "--profile",
            "work",
            "--",
            "review",
            "-c",
            "test \"$OPENAI_API_KEY\" = \"$ANTHROPIC_API_KEY\" && test \"$OPENAI_API_KEY\" = \"$LITELLM_API_KEY\" && test \"$OPENAI_API_KEY\" != sk-parent-profile-secret && case \"$OPENAI_BASE_URL\" in http://127.0.0.1:*/v1) ;; *) exit 1 ;; esac && test \"$ANTHROPIC_BASE_URL/v1\" = \"$OPENAI_BASE_URL\" && printf '%s\\n' \"$AIX_RUN_ID\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let run_id = String::from_utf8(output).unwrap().trim().to_string();
    assert!(uuid::Uuid::parse_str(&run_id).is_ok());

    let record_output = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", &run_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let envelope: Value = serde_json::from_slice(&record_output).unwrap();
    assert_eq!(envelope["data"]["logical_tool_name"], "review");
}

#[test]
#[cfg(unix)]
fn nonzero_child_exit_is_recorded_and_command_content_is_not_persisted() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://private-base.example"

[profiles.work]
api_key = "sk-private-key"
"#,
        )
        .unwrap();
    let state = assert_fs::TempDir::new().unwrap();

    let assertion = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .args([
            "run",
            "--profile",
            "work",
            "--",
            "sh",
            "-c",
            "read input; printf '%s\\n' \"$AIX_RUN_ID\"; printf '%s\\n' STDOUT_PRIVATE; printf '%s\\n' STDERR_PRIVATE >&2; printf '%s' \"$input\" >/dev/null; test \"$1\" = ARGUMENT_PRIVATE; exit 42",
            "_",
            "ARGUMENT_PRIVATE",
        ])
        .write_stdin("STDIN_PRIVATE\n")
        .assert()
        .code(42);
    let run_id = std::str::from_utf8(&assertion.get_output().stdout)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();
    assert!(String::from_utf8_lossy(&assertion.get_output().stderr).contains("STDERR_PRIVATE"));

    let output = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", &run_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let envelope: Value = serde_json::from_slice(&output).unwrap();
    let serialized = envelope.to_string();
    for private_value in [
        "ARGUMENT_PRIVATE",
        "STDIN_PRIVATE",
        "STDOUT_PRIVATE",
        "STDERR_PRIVATE",
        "sk-private-key",
        "https://private-base.example",
    ] {
        assert!(
            !serialized.contains(private_value),
            "run record leaked {private_value}: {serialized}"
        );
    }
    assert_eq!(envelope["data"]["status"], "failed");
    assert_eq!(envelope["data"]["process_exit_code"], 42);
}

#[test]
#[cfg(unix)]
fn nonzero_child_exit_is_preserved_if_final_record_write_fails() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "sk-private-key"
"#,
        )
        .unwrap();
    let state = assert_fs::TempDir::new().unwrap();

    let assertion = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .args([
            "run",
            "--profile",
            "work",
            "--",
            "sh",
            "-c",
            "printf '%s\\n' \"$AIX_RUN_ID\"; mkdir \"$AIX_STATE_DIR/runs/$AIX_RUN_ID/00000000000000000002.json\"; exit 42",
        ])
        .assert()
        .code(42);
    let output = assertion.get_output();
    let run_id = String::from_utf8(output.stdout.clone())
        .unwrap()
        .trim()
        .to_string();
    assert!(!output.stderr.is_empty());

    let record = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", &run_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let record: Value = serde_json::from_slice(&record).unwrap();
    assert_eq!(record["data"]["status"], "running");
    assert!(record["data"]["finished_at_unix_ms"].is_null());
}

#[test]
#[cfg(unix)]
fn runs_list_is_newest_first_limited_and_supports_human_output() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "sk-private-key"
"#,
        )
        .unwrap();
    let state = assert_fs::TempDir::new().unwrap();

    let first = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .args([
            "run",
            "--profile",
            "work",
            "--workflow",
            "older",
            "--",
            "sh",
            "-c",
            "printf '%s' \"$AIX_RUN_ID\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    std::thread::sleep(Duration::from_millis(20));
    let second = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .args([
            "run",
            "--profile",
            "work",
            "--workflow",
            "newer",
            "--",
            "sh",
            "-c",
            "printf '%s' \"$AIX_RUN_ID\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let first_id = String::from_utf8(first).unwrap();
    let second_id = String::from_utf8(second).unwrap();

    let output = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "--json", "--limit", "1"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let envelope: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(envelope["command"], "runs");
    let data = envelope["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["run_id"], second_id);
    assert_ne!(data[0]["run_id"], first_id);

    let human = cmd()
        .env("AIX_STATE_DIR", state.path())
        .arg("runs")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let human = String::from_utf8(human).unwrap();
    assert!(human.contains("newer"));
    assert!(human.contains("succeeded"));
    assert!(!human.contains("sk-private-key"));

    let human_record = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", &second_id])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let human_record = String::from_utf8(human_record).unwrap();
    assert!(human_record.contains("Executable name: \"sh\""));
    assert!(human_record.contains("Workflow: \"newer\""));
    assert!(human_record.contains("Status: succeeded"));
}

#[test]
#[cfg(unix)]
fn human_run_listing_escapes_control_characters_in_metadata() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "sk-private-key"
"#,
        )
        .unwrap();
    let state = assert_fs::TempDir::new().unwrap();
    let workflow = "nightly\nStatus: forged\u{1b}[31m";

    cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .args([
            "run",
            "--profile",
            "work",
            "--workflow",
            workflow,
            "--",
            "sh",
            "-c",
            "true",
        ])
        .assert()
        .success();

    let output = cmd()
        .env("AIX_STATE_DIR", state.path())
        .arg("runs")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();
    assert_eq!(output.lines().count(), 2);
    assert!(output.contains("nightly\\nStatus: forged\\u{1b}[31m"));
    assert!(!output.contains("nightly\nStatus: forged"));
}

#[test]
#[cfg(unix)]
fn abruptly_terminated_aix_leaves_a_readable_running_record() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "sk-private-key"
"#,
        )
        .unwrap();
    let state = assert_fs::TempDir::new().unwrap();

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

    let record = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", &run_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let record: Value = serde_json::from_slice(&record).unwrap();
    assert_eq!(record["data"]["status"], "running");
    assert!(record["data"]["finished_at_unix_ms"].is_null());
}

#[test]
#[cfg(unix)]
fn interrupt_marks_the_run_interrupted_before_exiting() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "sk-private-key"
"#,
        )
        .unwrap();
    let state = assert_fs::TempDir::new().unwrap();

    let started = Instant::now();
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
            "printf '%s\\n' \"$AIX_RUN_ID\"; kill -INT \"$PPID\"; exec sleep 5",
        ])
        .assert()
        .code(130)
        .get_output()
        .stdout
        .clone();
    assert!(started.elapsed() < Duration::from_secs(2));
    let run_id = String::from_utf8(output)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();

    let record = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", &run_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let record: Value = serde_json::from_slice(&record).unwrap();
    assert_eq!(record["data"]["status"], "interrupted");
}

#[test]
#[cfg(unix)]
fn termination_signal_marks_the_run_interrupted_before_exiting() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "sk-private-key"
"#,
        )
        .unwrap();
    let state = assert_fs::TempDir::new().unwrap();

    let started = Instant::now();
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
            "printf '%s\\n' \"$AIX_RUN_ID\"; kill -TERM \"$PPID\"; exec sleep 5",
        ])
        .assert()
        .code(143)
        .get_output()
        .stdout
        .clone();
    assert!(started.elapsed() < Duration::from_secs(2));
    let run_id = String::from_utf8(output)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();

    let record = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", &run_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let record: Value = serde_json::from_slice(&record).unwrap();
    assert_eq!(record["data"]["status"], "interrupted");
}
