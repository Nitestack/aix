use assert_cmd::Command;
use assert_fs::prelude::*;
use predicates::prelude::*;
use predicates::str::contains;

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

#[test]
fn help_exits_zero() {
    cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Usage: aix"));
}

#[test]
fn version_exits_zero() {
    cmd()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains("aix"));
}

#[test]
fn no_args_exits_nonzero() {
    cmd().assert().failure();
}

#[test]
fn profiles_help() {
    cmd().args(["profiles", "--help"]).assert().success();
}

#[test]
fn env_help() {
    cmd().args(["env", "--help"]).assert().success();
}

#[test]
fn shell_help() {
    cmd().args(["shell", "--help"]).assert().success();
}

#[test]
fn exec_help() {
    cmd().args(["exec", "--help"]).assert().success();
}

#[test]
fn config_help() {
    cmd().args(["config", "--help"]).assert().success();
}

#[test]
fn profiles_run_exits_nonzero() {
    cmd().arg("profiles").assert().failure();
}

#[test]
fn env_default_exits_nonzero() {
    cmd().arg("env").assert().failure();
}

const VALID_TOML: &str = r#"
[endpoint]
base_url = { env = "AIX_BASE_URL" }

[profiles.work]
api_key = { env = "AIX_API_KEY" }
"#;

#[test]
fn shell_with_args_after_separator_gives_helpful_message() {
    // `aix shell` does not accept commands after --; the error must point to `aix exec`.
    // No config needed: the check fires before config is loaded.
    let err = cmd()
        .args(["shell", "--", "echo", "hello"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&err).unwrap();
    assert!(s.contains("aix exec"), "error must mention 'aix exec': {s}");
}

#[test]
fn config_path_prints_explicit_path() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(VALID_TOML).unwrap();
    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["config", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            file.path().to_string_lossy().as_ref(),
        ));
}

#[test]
fn config_path_no_config_found_exits_nonzero() {
    let home = assert_fs::TempDir::new().unwrap();
    cmd()
        .env("HOME", home.path())
        .env_remove("AIX_CONFIG")
        .env_remove("XDG_CONFIG_HOME")
        .args(["config", "path"])
        .assert()
        .failure();
}

#[test]
fn config_validate_valid_config() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(VALID_TOML).unwrap();
    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["config", "validate"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Config is valid."));
}

#[test]
fn config_validate_invalid_config() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    // Missing required `endpoint` section → serde parse error
    file.write_str("[profiles.work]\napi_key = \"sk-test\"\n")
        .unwrap();
    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["config", "validate"])
        .assert()
        .failure();
}
