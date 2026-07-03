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
base_url = "https://ai.example.com/v1"
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
base_url = "https://ai.example.com/v1"
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
// Old wrapper always emits exactly these 5 variables (api_format = anthropic):
//   AIX_PROFILE, AIX_API_KEY, AIX_BASE_URL,
//   ANTHROPIC_API_KEY, ANTHROPIC_BASE_URL
//
// Rust CLI: same 5 when api_format = "anthropic".

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

    // Must contain exactly these 5 variable names.
    for var in &[
        "AIX_PROFILE",
        "AIX_API_KEY",
        "AIX_BASE_URL",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
    ] {
        assert!(s.contains(var), "missing variable {var}: {s}");
    }
    // Must not introduce new variables beyond the old wrapper's set.
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

    for key in &[
        "AIX_PROFILE",
        "AIX_API_KEY",
        "AIX_BASE_URL",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
    ] {
        assert!(
            parsed.get(*key).is_some(),
            "missing key {key} in JSON output"
        );
    }
    assert!(
        parsed.get("OPENAI_API_KEY").is_none(),
        "unexpected OPENAI_API_KEY in JSON output"
    );
}

// ── exec with command sets env vars (mirrors old `shell -- cmd`) ─────────────
//
// Old wrapper: `aix shell myprofile -- printenv AIX_API_KEY`
// Rust CLI: `aix exec myprofile -- printenv AIX_API_KEY`
//
// The `shell` subcommand no longer accepts `-- cmd`; `exec` is the replacement.

#[test]
#[cfg(unix)]
fn exec_with_command_sets_all_five_anthropic_vars() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    for var in &[
        "AIX_PROFILE",
        "AIX_API_KEY",
        "AIX_BASE_URL",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
    ] {
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

#[test]
#[cfg(unix)]
fn exec_anthropic_vars_mirror_aix_vars() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let get = |var: &str| {
        let out = cmd()
            .env("AIX_CONFIG", file.path())
            .args(["exec", "swtb", "--", "printenv", var])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        std::str::from_utf8(&out).unwrap().trim().to_string()
    };

    assert_eq!(
        get("AIX_API_KEY"),
        get("ANTHROPIC_API_KEY"),
        "ANTHROPIC_API_KEY must equal AIX_API_KEY"
    );
    assert_eq!(
        get("AIX_BASE_URL"),
        get("ANTHROPIC_BASE_URL"),
        "ANTHROPIC_BASE_URL must equal AIX_BASE_URL"
    );
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
