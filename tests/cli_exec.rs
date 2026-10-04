use assert_cmd::Command;
use assert_fs::prelude::*;

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

const CONFIG: &str = r#"
[endpoint]
base_url = "https://ai.example.com"

[profiles.swtb]
api_key = "sk-swtb-key"
"#;

const CONFIG_WITH_DEFAULT: &str = r#"
default_profile = "swtb"

[endpoint]
base_url = "https://ai.example.com"

[profiles.swtb]
api_key = "sk-swtb-key"
"#;

// --- exec: environment variable injection ---

#[test]
#[cfg(unix)]
fn exec_sets_aix_env_vars_in_child() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["exec", "swtb", "--", "printenv", "AIX_PROFILE"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap().trim();
    assert_eq!(s, "swtb", "got: {s}");
}

#[test]
#[cfg(unix)]
fn exec_sets_anthropic_env_vars_by_default() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["exec", "swtb", "--", "printenv", "ANTHROPIC_API_KEY"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap().trim();
    assert_eq!(s, "sk-swtb-key", "got: {s}");
}

#[test]
#[cfg(unix)]
fn exec_uses_profile_base_url_override() {
    let config = r#"
[endpoint]
base_url = "https://shared.example.com"

[profiles.local]
api_key = "sk-local-key"
base_url = "https://local.example.com"
"#;
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(config).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["exec", "local", "--", "printenv", "ANTHROPIC_BASE_URL"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    assert_eq!(
        std::str::from_utf8(&out).unwrap().trim(),
        "https://local.example.com"
    );
}

#[test]
#[cfg(unix)]
fn exec_sets_profile_custom_env_vars_in_child() {
    let config = r#"
[endpoint]
base_url = "https://ai.example.com"

[profiles.qwen]
api_key = "sk-qwen-key"

[profiles.qwen.env]
CLAUDE_CODE_SUBAGENT_MODEL = "qwen3.7-max"
"#;
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(config).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args([
            "exec",
            "qwen",
            "--",
            "printenv",
            "CLAUDE_CODE_SUBAGENT_MODEL",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let Ok(value) = std::str::from_utf8(&out) else {
        panic!("child output must be UTF-8");
    };
    assert_eq!(value.trim(), "qwen3.7-max");
}

// --- exec: exit code propagation ---

#[test]
#[cfg(unix)]
fn exec_propagates_zero_exit_code() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["exec", "swtb", "--", "true"])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn exec_propagates_nonzero_exit_code() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["exec", "swtb", "--", "sh", "-c", "exit 42"])
        .assert()
        .code(42);
}

// --- exec: missing executable ---

#[test]
fn exec_missing_executable_gives_clear_error() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let err = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["exec", "swtb", "--", "aihub-nonexistent-command-xyz"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&err).unwrap();
    assert!(
        s.contains("aihub-nonexistent-command-xyz"),
        "error must name the missing executable: {s}"
    );
}

// --- exec: no command after -- ---

#[test]
fn exec_no_command_after_separator_errors() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["exec", "swtb", "--"])
        .assert()
        .failure();
}

// --- exec: --dry-run ---

#[test]
fn exec_dry_run_shows_variable_names_not_values() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let output = cmd()
        .env("AIX_CONFIG", file.path())
        .args([
            "exec",
            "swtb",
            "--dry-run",
            "--",
            "printenv",
            "ANTHROPIC_API_KEY",
        ])
        .assert()
        .success()
        .get_output()
        .clone();

    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    assert!(
        stderr.contains("ANTHROPIC_API_KEY"),
        "must mention var name: {stderr}"
    );
    assert!(
        !stderr.contains("sk-swtb-key"),
        "must not leak value: {stderr}"
    );
    assert!(
        stderr.contains("printenv"),
        "must mention program: {stderr}"
    );
    // Child was not executed, so stdout must be empty
    assert!(output.stdout.is_empty(), "dry-run must not run the child");
}

#[test]
fn exec_dry_run_does_not_execute_nonexistent_binary() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    // With --dry-run, even a nonexistent program succeeds (we never exec it)
    cmd()
        .env("AIX_CONFIG", file.path())
        .args([
            "exec",
            "swtb",
            "--dry-run",
            "--",
            "aihub-nonexistent-command-xyz",
        ])
        .assert()
        .success();
}

// --- exec: profile selection ---

#[test]
#[cfg(unix)]
fn exec_uses_global_profile_flag() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["--profile", "swtb", "exec", "--", "true"])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn exec_uses_default_profile_when_none_specified() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_WITH_DEFAULT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("AIX_PROFILE")
        .args(["exec", "--", "printenv", "AIX_PROFILE"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap().trim();
    assert_eq!(s, "swtb");
}

// --- claude: dry-run ---

#[test]
fn claude_dry_run_shows_claude_command() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let stderr = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["claude", "swtb", "--dry-run", "--", "--version"])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&stderr).unwrap();
    assert!(s.contains("claude"), "must mention 'claude': {s}");
    assert!(
        s.contains("ANTHROPIC_API_KEY"),
        "must list env var names: {s}"
    );
    assert!(!s.contains("sk-swtb-key"), "must not leak value: {s}");
}

// --- pi: dry-run ---

#[test]
fn pi_dry_run_shows_pi_command() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let stderr = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["pi", "swtb", "--dry-run", "--", "--version"])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&stderr).unwrap();
    assert!(s.contains(" pi"), "must mention 'pi': {s}");
    assert!(!s.contains("sk-swtb-key"), "must not leak value: {s}");
}

#[test]
#[cfg(unix)]
fn configured_named_tool_uses_command_format_and_tool_env_precedence() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://ai.example.com"

[profiles.work]
api_key = "sk-profile-key"

[profiles.work.env]
LAUNCH_LAYER = "profile"

[tools.review]
command = "sh"
api_format = "openai"

[tools.review.env]
OPENAI_API_KEY = "tool-key"
LAUNCH_LAYER = "tool"
TOOL_TOKEN = { env = "AIX_REVIEW_TOKEN" }
"#,
        )
        .unwrap();

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_REVIEW_TOKEN", "resolved-review-token")
        .env("ANTHROPIC_API_KEY", "ambient-key")
        .env("ANTHROPIC_BASE_URL", "https://ambient.example.com")
        .args([
            "review",
            "work",
            "--",
            "-c",
            "test \"$OPENAI_API_KEY\" = tool-key && test \"$LITELLM_API_KEY\" = sk-profile-key && test \"$LAUNCH_LAYER\" = tool && test \"$TOOL_TOKEN\" = resolved-review-token && test -z \"${ANTHROPIC_API_KEY+x}\" && test -z \"${ANTHROPIC_BASE_URL+x}\" && printf '%s' \"$1\"",
            "_",
            "forwarded-unchanged",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    assert_eq!(String::from_utf8(output).unwrap(), "forwarded-unchanged");
}

#[test]
#[cfg(unix)]
fn configured_named_tools_support_each_api_format() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://ai.example.com"

[profiles.work]
api_key = "sk-profile-key"

[tools.anthropic]
command = "sh"
api_format = "anthropic"

[tools.openai]
command = "sh"
api_format = "openai"

[tools.both]
command = "sh"
api_format = "both"
"#,
        )
        .unwrap();

    for (tool, check) in [
        (
            "anthropic",
            "test -n \"$ANTHROPIC_API_KEY\" && test -z \"${OPENAI_API_KEY+x}\"",
        ),
        (
            "openai",
            "test -z \"${ANTHROPIC_API_KEY+x}\" && test -n \"$OPENAI_API_KEY\"",
        ),
        (
            "both",
            "test -n \"$ANTHROPIC_API_KEY\" && test -n \"$OPENAI_API_KEY\"",
        ),
    ] {
        cmd()
            .env("AIX_CONFIG", config.path())
            .args([tool, "work", "--", "-c", check])
            .assert()
            .success();
    }
}

#[test]
#[cfg(unix)]
fn local_gateway_overrides_standard_credentials_for_each_api_format() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://upstream.example.com"

[profiles.work]
api_key = "sk-parent-profile-secret"

[profiles.work.env]
OPENAI_API_KEY = "profile-openai-secret"
ANTHROPIC_API_KEY = "profile-anthropic-secret"
LITELLM_API_KEY = "profile-litellm-secret"

[tools.openai]
command = "sh"
api_format = "openai"
local_gateway = true

[tools.openai.env]
OPENAI_API_KEY = "tool-openai-secret"
TOOL_SETTING = "preserved"

[tools.anthropic]
command = "sh"
api_format = "anthropic"
local_gateway = true

[tools.both]
command = "sh"
api_format = "both"
local_gateway = true
"#,
        )
        .unwrap();
    let state = assert_fs::TempDir::new().unwrap();

    for (tool, check) in [
        (
            "openai",
            "test -n \"$OPENAI_API_KEY\" && test \"$OPENAI_API_KEY\" != sk-parent-profile-secret && test \"$OPENAI_API_KEY\" != tool-openai-secret && case \"$OPENAI_BASE_URL\" in http://127.0.0.1:*/v1) ;; *) exit 1 ;; esac && test \"$LITELLM_API_KEY\" = \"$OPENAI_API_KEY\" && test \"$LITELLM_BASE_URL\" = \"$OPENAI_BASE_URL\" && test -z \"${ANTHROPIC_API_KEY+x}\" && test -z \"${ANTHROPIC_BASE_URL+x}\" && test \"$TOOL_SETTING\" = preserved",
        ),
        (
            "anthropic",
            "test -n \"$ANTHROPIC_API_KEY\" && test \"$ANTHROPIC_API_KEY\" != sk-parent-profile-secret && case \"$ANTHROPIC_BASE_URL\" in http://127.0.0.1:*) ;; *) exit 1 ;; esac && test \"$LITELLM_API_KEY\" = \"$ANTHROPIC_API_KEY\" && test \"$LITELLM_BASE_URL\" = \"$ANTHROPIC_BASE_URL/v1\" && test -z \"${OPENAI_API_KEY+x}\" && test -z \"${OPENAI_BASE_URL+x}\"",
        ),
        (
            "both",
            "test -n \"$OPENAI_API_KEY\" && test \"$OPENAI_API_KEY\" = \"$ANTHROPIC_API_KEY\" && test \"$OPENAI_BASE_URL\" = \"$ANTHROPIC_BASE_URL/v1\" && test \"$LITELLM_API_KEY\" = \"$OPENAI_API_KEY\" && test \"$LITELLM_BASE_URL\" = \"$OPENAI_BASE_URL\" && test \"$OPENAI_API_KEY\" != sk-parent-profile-secret",
        ),
    ] {
        cmd()
            .env("AIX_CONFIG", config.path())
            .env("AIX_STATE_DIR", state.path())
            .env("ANTHROPIC_API_KEY", "ambient-anthropic-secret")
            .env("OPENAI_API_KEY", "ambient-openai-secret")
            .env("LITELLM_API_KEY", "ambient-litellm-secret")
            .args([tool, "work", "--", "-c", check])
            .assert()
            .success();
    }
}

#[test]
#[cfg(unix)]
fn local_gateway_dry_run_does_not_resolve_parent_or_custom_secrets() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = { env = "AIX_DRY_RUN_UPSTREAM_URL_MISSING" }

[profiles.work]
api_key = { env = "AIX_DRY_RUN_PARENT_KEY_MISSING" }

[profiles.work.env]
PROFILE_SETTING = { env = "AIX_DRY_RUN_PROFILE_ENV_MISSING" }

[tools.review]
command = "sh"
api_format = "openai"
local_gateway = true

[tools.review.env]
TOOL_SETTING = { env = "AIX_DRY_RUN_TOOL_ENV_MISSING" }
"#,
        )
        .unwrap();

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env_remove("AIX_DRY_RUN_UPSTREAM_URL_MISSING")
        .env_remove("AIX_DRY_RUN_PARENT_KEY_MISSING")
        .env_remove("AIX_DRY_RUN_PROFILE_ENV_MISSING")
        .env_remove("AIX_DRY_RUN_TOOL_ENV_MISSING")
        .args(["review", "work", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .clone();
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.contains("local gateway: openai"), "{output}");
    for name in [
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "LITELLM_API_KEY",
        "LITELLM_BASE_URL",
        "PROFILE_SETTING",
        "TOOL_SETTING",
    ] {
        assert!(output.contains(name), "missing {name}: {output}");
    }
    for marker in [
        "AIX_DRY_RUN_UPSTREAM_URL_MISSING",
        "AIX_DRY_RUN_PARENT_KEY_MISSING",
        "AIX_DRY_RUN_PROFILE_ENV_MISSING",
        "AIX_DRY_RUN_TOOL_ENV_MISSING",
        "sk-parent-profile-secret",
    ] {
        assert!(!output.contains(marker), "revealed {marker}: {output}");
    }
}

#[test]
#[cfg(unix)]
fn unconfigured_named_tool_keeps_openai_fallback() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let output = cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("ANTHROPIC_API_KEY")
        .args([
            "sh",
            "swtb",
            "--",
            "-c",
            "printf '%s|%s' \"$OPENAI_API_KEY\" \"${ANTHROPIC_API_KEY-unset}\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    assert_eq!(String::from_utf8(output).unwrap(), "sk-swtb-key|unset");
}

#[test]
fn configured_tool_secrets_are_not_printed_in_dry_run_or_launch_errors() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://ai.example.com"

[profiles.work]
api_key = "sk-profile-key"

[tools.review]
command = "aix-missing-review-executable"
api_format = "both"

[tools.review.env]
TOOL_TOKEN = "tool-secret-sentinel"
"#,
        )
        .unwrap();

    let dry_run = cmd()
        .env("AIX_CONFIG", config.path())
        .args(["review", "work", "--dry-run", "--", "--help"])
        .assert()
        .success()
        .get_output()
        .clone();
    let dry_run_output = format!(
        "{}{}",
        String::from_utf8_lossy(&dry_run.stdout),
        String::from_utf8_lossy(&dry_run.stderr)
    );
    assert!(dry_run_output.contains("TOOL_TOKEN"));
    assert!(!dry_run_output.contains("tool-secret-sentinel"));

    let error = cmd()
        .env("AIX_CONFIG", config.path())
        .args(["review", "work", "--", "--help"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    assert!(!String::from_utf8(error)
        .unwrap()
        .contains("tool-secret-sentinel"));
}

#[test]
fn configured_tool_secret_sources_are_not_resolved_during_dry_run() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://ai.example.com"

[profiles.work]
api_key = "sk-profile-key"

[tools.review]
api_format = "both"

[tools.review.env]
TOOL_TOKEN = { env = "AIX_TOOL_ENV_DRY_RUN_MISSING" }
"#,
        )
        .unwrap();

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env_remove("AIX_TOOL_ENV_DRY_RUN_MISSING")
        .args(["review", "work", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();

    let stderr = String::from_utf8(output).unwrap();
    assert!(stderr.contains("TOOL_TOKEN"), "got: {stderr}");
    assert!(!stderr.contains("AIX_TOOL_ENV_DRY_RUN_MISSING"));
}

#[test]
fn configured_tool_validation_rejects_invalid_entries() {
    for invalid_config in [
        r#"
[endpoint]
base_url = "https://ai.example.com"
[profiles.work]
api_key = "sk-test"
[tools.""]
api_format = "both"
"#,
        r#"
[endpoint]
base_url = "https://ai.example.com"
[profiles.work]
api_key = "sk-test"
[tools.review]
command = "  "
api_format = "both"
"#,
        r#"
[endpoint]
base_url = "https://ai.example.com"
[profiles.work]
api_key = "sk-test"
[tools.review]
api_format = "both"
[tools.review.env]
"INVALID-NAME" = "value"
"#,
        r#"
[endpoint]
base_url = "https://ai.example.com"
[profiles.work]
api_key = "sk-test"
[tools.review]
api_format = "unknown"
"#,
        r#"
[endpoint]
base_url = "https://ai.example.com"
[profiles.work]
api_key = "sk-test"
[tools.review]
command = "review-agent"
"#,
    ] {
        let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
        config.write_str(invalid_config).unwrap();
        cmd()
            .env("AIX_CONFIG", config.path())
            .args(["config", "validate"])
            .assert()
            .failure();
    }
}

// --- shell: dry-run ---

#[test]
fn shell_dry_run_shows_shell_and_var_names() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let stderr = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["shell", "swtb", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&stderr).unwrap();
    assert!(s.contains("Would run:"), "must show would-run: {s}");
    assert!(s.contains("ANTHROPIC_API_KEY"), "must list var names: {s}");
    assert!(!s.contains("sk-swtb-key"), "must not leak value: {s}");
}
