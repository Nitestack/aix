use assert_cmd::Command;
use assert_fs::prelude::*;

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

const CONFIG: &str = r#"
[endpoint]
base_url = "https://ai.example.com"

[profiles.work]
api_key = "sk-work-key"

[profiles.sub]
label = "Claude subscription"
auth = { type = "native" }

[profiles.sub.env]
CLAUDE_CONFIG_DIR = "/tmp/claude-sub"
"#;

fn config_file(contents: &str) -> assert_fs::NamedTempFile {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(contents).unwrap();
    file
}

fn inherited_credentials(command: &mut Command) -> &mut Command {
    command
        .env("ANTHROPIC_API_KEY", "sk-inherited-anthropic")
        .env("ANTHROPIC_AUTH_TOKEN", "sk-inherited-token")
        .env("ANTHROPIC_BASE_URL", "https://inherited.example.com")
        .env("OPENAI_API_KEY", "sk-inherited-openai")
        .env("OPENAI_BASE_URL", "https://inherited.example.com/v1")
        .env("LITELLM_API_KEY", "sk-inherited-litellm")
        .env("LITELLM_BASE_URL", "https://inherited.example.com")
}

fn stdout_of(command: &mut Command) -> String {
    let out = command.assert().success().get_output().stdout.clone();
    String::from_utf8(out).unwrap()
}

#[test]
#[cfg(unix)]
fn exec_with_native_profile_removes_inherited_gateway_credentials() {
    let file = config_file(CONFIG);
    let mut command = cmd();
    inherited_credentials(&mut command)
        .env("AIX_CONFIG", file.path())
        .args(["exec", "sub", "--", "env"]);
    let env = stdout_of(&mut command);

    assert!(env.contains("AIX_PROFILE=sub\n"), "got: {env}");
    assert!(
        env.contains("CLAUDE_CONFIG_DIR=/tmp/claude-sub\n"),
        "got: {env}"
    );
    for name in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "LITELLM_API_KEY",
        "LITELLM_BASE_URL",
    ] {
        assert!(
            !env.lines()
                .any(|line| line.starts_with(&format!("{name}="))),
            "{name} must be removed: {env}"
        );
    }
}

#[test]
#[cfg(unix)]
fn named_tool_with_native_profile_runs_without_gateway_credentials() {
    let config =
        format!("{CONFIG}\n[tools.claude]\ncommand = \"env\"\napi_format = \"anthropic\"\n");
    let file = config_file(&config);
    let mut command = cmd();
    inherited_credentials(&mut command)
        .env("AIX_CONFIG", file.path())
        .args(["claude", "sub"]);
    let env = stdout_of(&mut command);

    assert!(env.contains("AIX_PROFILE=sub\n"), "got: {env}");
    assert!(
        env.contains("CLAUDE_CONFIG_DIR=/tmp/claude-sub\n"),
        "got: {env}"
    );
    assert!(!env.contains("sk-inherited"), "got: {env}");
    assert!(!env.contains("sk-work-key"), "got: {env}");
}

#[test]
#[cfg(unix)]
fn api_key_profile_still_injects_gateway_credentials() {
    let file = config_file(CONFIG);
    let mut command = cmd();
    inherited_credentials(&mut command)
        .env("AIX_CONFIG", file.path())
        .args(["exec", "work", "--", "printenv", "ANTHROPIC_API_KEY"]);

    assert_eq!(stdout_of(&mut command).trim(), "sk-work-key");
}

#[test]
fn native_profile_dry_run_lists_no_gateway_credentials() {
    let file = config_file(CONFIG);
    let stderr = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["claude", "sub", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&stderr).unwrap();

    assert!(s.contains("Would run: claude"), "got: {s}");
    assert!(s.contains("AIX_PROFILE"), "got: {s}");
    assert!(s.contains("CLAUDE_CONFIG_DIR"), "got: {s}");
    assert!(!s.contains("ANTHROPIC_API_KEY"), "got: {s}");
}

#[test]
fn env_command_rejects_native_profile() {
    let file = config_file(CONFIG);
    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "sub"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains(
            "native profiles leave authentication to the launched tool",
        ));
}

#[test]
fn native_profile_rejects_base_url() {
    let config = r#"
[endpoint]
base_url = "https://ai.example.com"

[profiles.sub]
auth = { type = "native" }
base_url = "https://other.example.com"
"#;
    let file = config_file(config);
    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["exec", "sub", "--", "true"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains("must not configure base_url"));
}

#[test]
fn native_profile_rejects_budget_policy() {
    let config = format!("{CONFIG}\n[run_policies.capped]\nprofile = \"sub\"\nmax_budget = 1.0\nmax_duration = \"1m\"\n");
    let file = config_file(&config);
    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["run", "--policy", "capped", "--", "true"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "monetary budget enforcement unavailable",
        ));
}

#[test]
fn profiles_listing_accepts_native_profile_without_api_key() {
    let file = config_file(CONFIG);
    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["profiles"])
        .assert()
        .success()
        .stdout(predicates::str::contains("sub"));
}
