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
