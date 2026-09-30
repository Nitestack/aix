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
fn global_json_on_an_unsupported_command_is_a_validation_error() {
    let output = cmd().args(["--json", "config", "path"]).output().unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("only supported"));
}
