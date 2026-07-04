/// Parity tests: verify the Rust CLI preserves the important behaviors of the
/// old Nix-generated `aix` shell wrapper.
///
/// Each test is tagged with the old-wrapper behavior it covers.
/// Intentional differences are noted in `docs/aix/migration.md`.
use assert_cmd::Command;
use assert_fs::prelude::*;

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

// Config where api_key is resolved from an env var (not a Direct literal).
// Used to test "missing API key" error paths.
const CONFIG_ENV_API_KEY: &str = r#"
[endpoint]
base_url = "https://ai.example.com"
api_format = "anthropic"

[profiles.swtb]
api_key = { env = "AIX_PARITY_TEST_API_KEY" }
"#;

// Config where base_url is resolved from an env var.
const CONFIG_ENV_BASE_URL: &str = r#"
[endpoint]
base_url = { env = "AIX_PARITY_TEST_BASE_URL" }
api_format = "anthropic"

[profiles.swtb]
api_key = "sk-direct-key"
"#;

// Minimal working config for exec / shell parity tests.
const CONFIG_DIRECT: &str = r#"
[endpoint]
base_url = "https://ai.example.com"
api_format = "anthropic"

[profiles.swtb]
api_key = "sk-swtb-key"
"#;

// ── missing API key ─────────────────────────────────────────────────────────
//
// Old wrapper: `aix: missing secret for profile '$profile'` + exit 1
// Rust CLI: must fail with a message that identifies the missing variable.

#[test]
fn env_missing_api_key_exits_nonzero() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_ENV_API_KEY).unwrap();

    cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("AIX_PARITY_TEST_API_KEY")
        .args(["env", "swtb", "--format", "sh"])
        .assert()
        .failure();
}

#[test]
fn env_missing_api_key_error_names_the_variable() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_ENV_API_KEY).unwrap();

    let err = cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("AIX_PARITY_TEST_API_KEY")
        .args(["env", "swtb", "--format", "sh"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&err).unwrap();
    assert!(
        s.contains("AIX_PARITY_TEST_API_KEY"),
        "error must name the missing variable: {s}"
    );
}

#[test]
fn exec_missing_api_key_exits_nonzero() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_ENV_API_KEY).unwrap();

    cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("AIX_PARITY_TEST_API_KEY")
        .args(["exec", "swtb", "--", "true"])
        .assert()
        .failure();
}

// ── missing base URL ─────────────────────────────────────────────────────────
//
// Old wrapper: `aix: missing base URL secret` + exit 1
// Rust CLI: must fail with a message that identifies the missing variable.

#[test]
fn env_missing_base_url_exits_nonzero() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_ENV_BASE_URL).unwrap();

    cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("AIX_PARITY_TEST_BASE_URL")
        .args(["env", "swtb", "--format", "sh"])
        .assert()
        .failure();
}

#[test]
fn env_missing_base_url_error_names_the_variable() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_ENV_BASE_URL).unwrap();

    let err = cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("AIX_PARITY_TEST_BASE_URL")
        .args(["env", "swtb", "--format", "sh"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&err).unwrap();
    assert!(
        s.contains("AIX_PARITY_TEST_BASE_URL"),
        "error must name the missing variable: {s}"
    );
}

#[test]
fn exec_missing_base_url_exits_nonzero() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_ENV_BASE_URL).unwrap();

    cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("AIX_PARITY_TEST_BASE_URL")
        .args(["exec", "swtb", "--", "true"])
        .assert()
        .failure();
}

// ── env variable name parity ─────────────────────────────────────────────────
//
// For api_format = "anthropic" the CLI emits exactly 3 variables:
//   AIX_PROFILE, ANTHROPIC_API_KEY, ANTHROPIC_BASE_URL

#[test]
fn env_sh_exact_variable_set_matches_old_wrapper() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "sh"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    for var in &["AIX_PROFILE", "ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL"] {
        assert!(s.contains(var), "missing variable {var}: {s}");
    }
    assert!(!s.contains("AIX_API_KEY"), "unexpected AIX_API_KEY: {s}");
    assert!(!s.contains("AIX_BASE_URL"), "unexpected AIX_BASE_URL: {s}");
    assert!(!s.contains("OPENAI_"), "unexpected OPENAI_ variable: {s}");
}

#[test]
fn env_json_exact_key_set_matches_old_wrapper() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value = serde_json::from_slice(&out).expect("must be valid JSON");

    for key in &["AIX_PROFILE", "ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL"] {
        assert!(
            parsed.get(*key).is_some(),
            "missing key {key} in JSON output"
        );
    }
    assert!(
        parsed.get("AIX_API_KEY").is_none(),
        "unexpected AIX_API_KEY"
    );
    assert!(
        parsed.get("AIX_BASE_URL").is_none(),
        "unexpected AIX_BASE_URL"
    );
    assert!(
        parsed.get("OPENAI_API_KEY").is_none(),
        "unexpected OPENAI_API_KEY in JSON output"
    );
}

// ── exec with command sets env vars (mirrors old `shell -- cmd`) ─────────────

#[test]
#[cfg(unix)]
fn exec_with_command_sets_all_three_anthropic_vars() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    for var in &["AIX_PROFILE", "ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL"] {
        let out = cmd()
            .env("AIX_CONFIG", file.path())
            .args(["exec", "swtb", "--", "printenv", var])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let val = std::str::from_utf8(&out).unwrap().trim();
        assert!(!val.is_empty(), "variable {var} must be non-empty in child");
    }
}

// ── non-interactive safety ───────────────────────────────────────────────────
//
// Old wrapper: exits 1 with "profile required when no interactive terminal"
// Rust CLI: assert_cmd pipes stdin → is_terminal() = false → must exit nonzero.
//
// This is already covered in cli_profiles.rs but included here as a parity
// checkpoint with an explicit message assertion.

#[test]
fn exec_non_interactive_no_profile_does_not_hang() {
    // assert_cmd always pipes stdin, so the test is inherently non-interactive.
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let err = cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("AIX_PROFILE")
        .args(["exec", "--", "true"]) // no profile
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&err).unwrap();
    // Message must mention profile so the user knows what to fix.
    assert!(s.contains("profile"), "error must mention 'profile': {s}");
}

// ── secrets never appear in stdout ──────────────────────────────────────────
//
// Old wrapper: API key is only in env var output.
// Rust CLI: secret values must not appear in any non-env-output command.

#[test]
fn exec_dry_run_never_prints_secret_value() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let output = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["exec", "swtb", "--dry-run", "--", "true"])
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    assert!(!stdout.contains("sk-swtb-key"), "secret leaked to stdout");
    assert!(!stderr.contains("sk-swtb-key"), "secret leaked to stderr");
}
