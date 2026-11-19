use assert_cmd::Command;
use assert_fs::prelude::*;
use serde_json::Value;

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

#[test]
fn config_failure_in_json_mode_uses_validation_exit_code_and_empty_stdout() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str("[profiles.test\napi_key = \"never-print-this-secret\"\n")
        .unwrap();

    let output = cmd()
        .args([
            "--json",
            "--config",
            config.path().to_str().unwrap(),
            "profiles",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
    assert!(serde_json::from_slice::<Value>(&output.stderr).is_err());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("never-print-this-secret"));
}

#[test]
fn secret_resolution_failure_uses_its_exit_code_without_json_stdout() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "http://127.0.0.1:1"

[profiles.test]
api_key = { env = "AIX_CONTRACT_MISSING_SECRET" }
"#,
        )
        .unwrap();

    let output = cmd()
        .env_remove("AIX_CONTRACT_MISSING_SECRET")
        .args([
            "--json",
            "--config",
            config.path().to_str().unwrap(),
            "spend",
            "test",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("AIX_CONTRACT_MISSING_SECRET"));
    assert!(serde_json::from_slice::<Value>(&output.stderr).is_err());
}

#[test]
fn non_interactive_profile_failure_is_actionable_and_stays_off_stdout() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://gateway.example"

[profiles.work]
api_key = "test-key"
"#,
        )
        .unwrap();

    let output = cmd()
        .args([
            "--non-interactive",
            "--config",
            config.path().to_str().unwrap(),
            "env",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("profile"), "unexpected error: {stderr}");
    assert!(
        stderr.contains("default_profile"),
        "unexpected error: {stderr}"
    );
}

#[test]
fn global_json_on_an_unsupported_command_is_a_validation_error() {
    let output = cmd().args(["--json", "config", "path"]).output().unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("only supported"));
}

fn json_error(stderr: &[u8]) -> Value {
    let text = String::from_utf8_lossy(stderr);
    assert_eq!(text.lines().count(), 1, "one JSON line expected: {text}");
    serde_json::from_slice(stderr).unwrap_or_else(|_| panic!("not JSON: {text}"))
}

#[test]
fn error_format_json_reports_a_stable_code_for_a_missing_profile() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://gateway.example"

[profiles.work]
api_key = "never-print-this-secret"
"#,
        )
        .unwrap();

    let output = cmd()
        .args([
            "--error-format",
            "json",
            "--config",
            config.path().to_str().unwrap(),
            "env",
            "missing",
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let error = json_error(&output.stderr);
    assert_eq!(error["schema_version"], 1);
    assert_eq!(error["command"], "env");
    assert_eq!(error["error"]["code"], "profile_not_found");
    assert_eq!(error["error"]["category"], "config");
    assert_eq!(error["error"]["exit_code"], 2);
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("\"missing\""));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("never-print-this-secret"));
}

#[test]
fn error_format_json_reports_secret_failures_with_json_output() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "http://127.0.0.1:1"

[profiles.test]
api_key = { env = "AIX_CONTRACT_MISSING_SECRET" }
"#,
        )
        .unwrap();

    let output = cmd()
        .env_remove("AIX_CONTRACT_MISSING_SECRET")
        .args([
            "--json",
            "--error-format=json",
            "--config",
            config.path().to_str().unwrap(),
            "status",
            "test",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    let error = json_error(&output.stderr);
    assert_eq!(error["command"], "status");
    assert_eq!(error["error"]["code"], "secret_unavailable");
    assert_eq!(error["error"]["category"], "secret");
}

#[test]
fn error_format_json_covers_argument_errors() {
    let output = cmd()
        .args(["--error-format", "json", "status", "--no-such-flag"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let error = json_error(&output.stderr);
    assert_eq!(error["command"], "aix");
    assert_eq!(error["error"]["code"], "invalid_arguments");
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("--no-such-flag"));
}
