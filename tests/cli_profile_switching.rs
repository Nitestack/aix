use assert_cmd::Command;
use assert_fs::prelude::*;
use std::io::Write;
use std::process::{Command as ProcessCommand, Stdio};

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

const CONFIG_WITH_UNRESOLVED_SECRETS: &str = r#"
[endpoint]
base_url = { env = "AIX_TEST_SWITCH_BASE_URL" }

[profiles.work]
label = { env = "AIX_TEST_SWITCH_LABEL" }
api_key = { env = "AIX_TEST_SWITCH_API_KEY" }

[profiles.work.env]
CUSTOM_SECRET = { env = "AIX_TEST_SWITCH_CUSTOM_SECRET" }
"#;

const CONFIG_WITH_DEFAULT: &str = r#"
default_profile = "personal"

[endpoint]
base_url = { env = "AIX_TEST_SWITCH_BASE_URL" }

[profiles.work]
label = "Work account"
api_key = { env = "AIX_TEST_SWITCH_API_KEY" }

[profiles.personal]
label = "Personal account"
api_key = { env = "AIX_TEST_SWITCH_PERSONAL_KEY" }
"#;

const CONFIG_WITHOUT_DEFAULT: &str = r#"
[endpoint]
base_url = { env = "AIX_TEST_SWITCH_BASE_URL" }

[profiles.work]
api_key = { env = "AIX_TEST_SWITCH_API_KEY" }
"#;

#[test]
fn use_selects_a_profile_without_resolving_secrets() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_WITH_UNRESOLVED_SECRETS).unwrap();

    let output = cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("AIX_TEST_SWITCH_LABEL")
        .env_remove("AIX_TEST_SWITCH_BASE_URL")
        .env_remove("AIX_TEST_SWITCH_API_KEY")
        .env_remove("AIX_TEST_SWITCH_CUSTOM_SECRET")
        .args(["use", "work"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    assert_eq!(output, b"export AIX_PROFILE='work'\n");
}

#[test]
fn use_rejects_an_unknown_profile() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_WITH_UNRESOLVED_SECRETS).unwrap();

    let output = cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("AIX_TEST_SWITCH_LABEL")
        .args(["use", "missing"])
        .assert()
        .failure()
        .get_output()
        .clone();

    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("missing"), "got: {stderr}");
    assert!(stderr.contains("work"), "got: {stderr}");
}

#[test]
fn use_emits_an_assignment_for_each_supported_shell() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_WITH_UNRESOLVED_SECRETS).unwrap();

    let formats = [
        ("sh", "export AIX_PROFILE='work'\n"),
        ("bash", "export AIX_PROFILE='work'\n"),
        ("zsh", "export AIX_PROFILE='work'\n"),
        ("fish", "set -gx AIX_PROFILE 'work'\n"),
        ("nu", "$env.AIX_PROFILE = \"work\"\n"),
        ("powershell", "$env:AIX_PROFILE = 'work'\n"),
    ];

    for (shell, expected) in formats {
        let output = cmd()
            .env("AIX_CONFIG", file.path())
            .env_remove("AIX_TEST_SWITCH_LABEL")
            .args(["use", "work", "--shell", shell])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_eq!(output, expected.as_bytes(), "shell: {shell}");
        check_use_syntax(shell, expected);
    }
}

#[test]
fn use_quotes_profile_names_for_shell_syntax() {
    let config = r#"
[endpoint]
base_url = "https://example.invalid"

[profiles."work'$profile \"x\""]
api_key = "unused"
"#;
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(config).unwrap();

    let formats = [
        ("sh", "export AIX_PROFILE='work'\\''$profile \"x\"'\n"),
        ("fish", "set -gx AIX_PROFILE 'work\\'$profile \"x\"'\n"),
        ("nu", "$env.AIX_PROFILE = \"work'\\$profile \\\"x\\\"\"\n"),
        ("powershell", "$env:AIX_PROFILE = 'work''$profile \"x\"'\n"),
    ];

    for (shell, expected) in formats {
        let output = cmd()
            .env("AIX_CONFIG", file.path())
            .args(["use", "work'$profile \"x\"", "--shell", shell])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_eq!(output, expected.as_bytes(), "shell: {shell}");
        check_use_syntax(shell, expected);
    }
}

#[test]
fn use_clear_emits_an_unset_command_for_each_supported_shell() {
    let formats = [
        ("sh", "unset AIX_PROFILE\n"),
        ("bash", "unset AIX_PROFILE\n"),
        ("zsh", "unset AIX_PROFILE\n"),
        ("fish", "set --erase --global AIX_PROFILE\n"),
        ("nu", "hide-env --ignore-errors AIX_PROFILE\n"),
        (
            "powershell",
            "if (Test-Path Env:AIX_PROFILE) { Remove-Item Env:AIX_PROFILE }\n",
        ),
    ];

    for (shell, expected) in formats {
        let output = cmd()
            .args(["use", "--clear", "--shell", shell])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_eq!(output, expected.as_bytes(), "shell: {shell}");
    }
}

#[test]
fn use_json_outputs_only_the_profile_assignment() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_WITH_UNRESOLVED_SECRETS).unwrap();

    let output = cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("AIX_TEST_SWITCH_LABEL")
        .args(["use", "work", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value, serde_json::json!({ "AIX_PROFILE": "work" }));
    assert!(!String::from_utf8(output)
        .unwrap()
        .contains("AIX_TEST_SWITCH_"));
}

#[test]
fn current_prefers_a_valid_aix_profile_over_the_config_default() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_WITH_DEFAULT).unwrap();

    cmd()
        .env("AIX_CONFIG", file.path())
        .env("AIX_PROFILE", "work")
        .env_remove("AIX_TEST_SWITCH_API_KEY")
        .arg("current")
        .assert()
        .success()
        .stdout("work\n");
}

#[test]
fn current_falls_back_to_a_valid_default_when_aix_profile_is_invalid() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_WITH_DEFAULT).unwrap();

    cmd()
        .env("AIX_CONFIG", file.path())
        .env("AIX_PROFILE", "missing")
        .env_remove("AIX_TEST_SWITCH_PERSONAL_KEY")
        .args(["current", "--format", "short"])
        .assert()
        .success()
        .stdout("personal\n");
}

#[test]
fn current_json_marks_a_valid_default_profile_as_default_source() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_WITH_DEFAULT).unwrap();

    let output = cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("AIX_PROFILE")
        .env_remove("AIX_TEST_SWITCH_PERSONAL_KEY")
        .args(["current", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["data"]["name"], "personal");
    assert_eq!(value["data"]["label"], "Personal account");
    assert_eq!(value["data"]["source"], "default");
}

#[test]
fn current_without_a_valid_environment_or_default_profile_prints_none() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_WITHOUT_DEFAULT).unwrap();

    cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("AIX_PROFILE")
        .env_remove("AIX_TEST_SWITCH_API_KEY")
        .arg("current")
        .assert()
        .success()
        .stdout("none\n");
}

#[test]
fn current_without_a_config_file_reports_none_without_prompting() {
    let home = assert_fs::TempDir::new().unwrap();

    cmd()
        .env("HOME", home.path())
        .env_remove("AIX_CONFIG")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("AIX_PROFILE")
        .arg("current")
        .assert()
        .success()
        .stdout("none\n");
}

#[test]
fn current_json_returns_profile_name_label_and_source_in_the_cli_envelope() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_WITH_DEFAULT).unwrap();

    let output = cmd()
        .env("AIX_CONFIG", file.path())
        .env("AIX_PROFILE", "work")
        .env_remove("AIX_TEST_SWITCH_API_KEY")
        .args(["--json", "current"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["command"], "current");
    assert_eq!(value["data"]["name"], "work");
    assert_eq!(value["data"]["label"], "Work account");
    assert_eq!(value["data"]["source"], "env");
    assert!(!String::from_utf8(output)
        .unwrap()
        .contains("AIX_TEST_SWITCH_"));
}

#[test]
fn current_json_reports_a_missing_selection_with_null_profile_fields() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_WITHOUT_DEFAULT).unwrap();

    let output = cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("AIX_PROFILE")
        .args(["current", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["data"]["name"], serde_json::Value::Null);
    assert_eq!(value["data"]["label"], serde_json::Value::Null);
    assert_eq!(value["data"]["source"], "none");
}

#[test]
fn current_human_output_does_not_resolve_profile_labels() {
    let config = r#"
[endpoint]
base_url = { env = "AIX_TEST_SWITCH_BASE_URL" }

[profiles.work]
label = { env = "AIX_TEST_SWITCH_LABEL" }
api_key = { env = "AIX_TEST_SWITCH_API_KEY" }
"#;
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(config).unwrap();

    cmd()
        .env("AIX_CONFIG", file.path())
        .env("AIX_PROFILE", "work")
        .env_remove("AIX_TEST_SWITCH_LABEL")
        .env_remove("AIX_TEST_SWITCH_API_KEY")
        .arg("current")
        .assert()
        .success()
        .stdout("work\n");
}

#[test]
fn init_generates_a_wrapper_for_each_supported_shell_without_secrets() {
    let wrappers = [
        (
            "sh",
            "command aix use --shell sh \"$@\"",
            "command aix \"$@\"",
        ),
        (
            "bash",
            "command aix use --shell bash \"$@\"",
            "command aix \"$@\"",
        ),
        (
            "zsh",
            "command aix use --shell zsh \"$@\"",
            "command aix \"$@\"",
        ),
        (
            "fish",
            "command aix use --shell fish $use_args",
            "command aix $argv",
        ),
        ("nu", "def --env --wrapped aix [...args]", "^aix ...$args"),
        (
            "powershell",
            "Get-Command aix -CommandType Application",
            "& $aixExecutable @args",
        ),
    ];

    for (shell, use_command, forward_command) in wrappers {
        let output = cmd()
            .args(["init", shell])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let snippet = String::from_utf8(output).unwrap();
        assert!(snippet.contains(use_command), "shell: {shell}: {snippet}");
        assert!(
            snippet.contains(forward_command),
            "shell: {shell}: {snippet}"
        );
        assert!(!snippet.contains("API_KEY"), "shell: {shell}: {snippet}");
        assert!(!snippet.contains("BASE_URL"), "shell: {shell}: {snippet}");
    }
}

#[test]
fn init_does_not_offer_cmd_exe_support() {
    cmd().args(["init", "cmd"]).assert().failure();
}

#[test]
fn generated_sh_wrapper_forwards_non_use_arguments_unchanged() {
    use std::os::unix::fs::PermissionsExt;

    let temp = assert_fs::TempDir::new().unwrap();
    let fake_aix = temp.child("aix");
    let args_file = temp.child("args.txt");
    fake_aix
        .write_str("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$AIX_ARGS_FILE\"\n")
        .unwrap();
    std::fs::set_permissions(fake_aix.path(), std::fs::Permissions::from_mode(0o755)).unwrap();

    let init = cmd()
        .args(["init", "sh"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let script = format!(
        "{}\naix --model 'two words' '' --flag\n",
        String::from_utf8(init).unwrap()
    );
    let path = std::env::join_paths(std::iter::once(temp.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();

    let output = ProcessCommand::new("sh")
        .args(["-c", &script])
        .env("PATH", path)
        .env("AIX_ARGS_FILE", args_file.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        std::fs::read_to_string(args_file.path()).unwrap(),
        "--model\ntwo words\n\n--flag\n"
    );
}

#[cfg(unix)]
#[test]
fn sh_wrapper_persists_and_clears_only_the_selected_profile() {
    let temp = assert_fs::TempDir::new().unwrap();
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config.write_str(CONFIG_WITHOUT_DEFAULT).unwrap();
    put_aix_on_path(&temp);

    let init = cmd()
        .args(["init", "sh"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let script = format!(
        "{}\naix --version\nif aix use missing; then printf 'unexpected\\n'; else printf 'failure:%s\\n' \"$?\"; fi\nprintf 'scratch:%s:%s\\n' \"${{__aix_use_code-unset}}\" \"${{__aix_use_status-unset}}\"\naix use work\naix current\naix use --clear\naix current\n",
        String::from_utf8(init).unwrap()
    );
    let path = path_with(&temp);

    let output = ProcessCommand::new("sh")
        .args(["-c", &script])
        .env("PATH", path)
        .env("AIX_CONFIG", config.path())
        .env_remove("AIX_PROFILE")
        .env_remove("AIX_TEST_SWITCH_BASE_URL")
        .env_remove("AIX_TEST_SWITCH_API_KEY")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("aix "),
        "stdout: {stdout}; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.ends_with("failure:2\nscratch:unset:unset\nwork\nnone\n"),
        "stdout: {stdout}; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn nu_wrapper_persists_the_profile_for_later_aix_invocations() {
    if ProcessCommand::new("nu").arg("--version").output().is_err() {
        return;
    }

    let temp = assert_fs::TempDir::new().unwrap();
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config.write_str(CONFIG_WITHOUT_DEFAULT).unwrap();
    put_aix_on_path(&temp);

    let init = cmd()
        .args(["init", "nu"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let script = format!(
        "{}\naix --version\naix use work\naix current\naix use --clear\naix current\n",
        String::from_utf8(init).unwrap()
    );
    let output = ProcessCommand::new("nu")
        .args(["--commands", &script])
        .env("PATH", path_with(&temp))
        .env("AIX_CONFIG", config.path())
        .env_remove("AIX_PROFILE")
        .env_remove("AIX_TEST_SWITCH_BASE_URL")
        .env_remove("AIX_TEST_SWITCH_API_KEY")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("aix "),
        "stdout: {stdout}; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.ends_with("work\nnone\n"),
        "stdout: {stdout}; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn fish_wrapper_persists_the_profile_for_later_aix_invocations() {
    if ProcessCommand::new("fish")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }

    let temp = assert_fs::TempDir::new().unwrap();
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config.write_str(CONFIG_WITHOUT_DEFAULT).unwrap();
    put_aix_on_path(&temp);

    let init = cmd()
        .args(["init", "fish"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let script = format!(
        "{}\naix --version\naix use work\naix current\naix use --clear\naix current\n",
        String::from_utf8(init).unwrap()
    );
    let output = ProcessCommand::new("fish")
        .args(["--no-config", "--command", &script])
        .env("PATH", path_with(&temp))
        .env("AIX_CONFIG", config.path())
        .env_remove("AIX_PROFILE")
        .env_remove("AIX_TEST_SWITCH_BASE_URL")
        .env_remove("AIX_TEST_SWITCH_API_KEY")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("aix "),
        "stdout: {stdout}; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.ends_with("work\nnone\n"),
        "stdout: {stdout}; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn powershell_wrapper_persists_the_profile_for_later_aix_invocations() {
    if ProcessCommand::new("pwsh")
        .arg("-Version")
        .output()
        .is_err()
    {
        return;
    }

    let temp = assert_fs::TempDir::new().unwrap();
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config.write_str(CONFIG_WITHOUT_DEFAULT).unwrap();
    put_aix_on_path(&temp);

    let init = cmd()
        .args(["init", "powershell"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let script = format!(
        "{}\naix --version\naix use work\naix current\naix use --clear\naix current\n",
        String::from_utf8(init).unwrap()
    );
    let output = ProcessCommand::new("pwsh")
        .args(["-NoProfile", "-Command", &script])
        .env("PATH", path_with(&temp))
        .env("AIX_CONFIG", config.path())
        .env_remove("AIX_PROFILE")
        .env_remove("AIX_TEST_SWITCH_BASE_URL")
        .env_remove("AIX_TEST_SWITCH_API_KEY")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("aix "),
        "stdout: {stdout}; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.ends_with("work\nnone\n"),
        "stdout: {stdout}; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
fn put_aix_on_path(temp: &assert_fs::TempDir) {
    let binary = std::path::PathBuf::from(cmd().get_program());
    std::os::unix::fs::symlink(binary, temp.child("aix").path()).unwrap();
}

#[cfg(unix)]
fn path_with(temp: &assert_fs::TempDir) -> std::ffi::OsString {
    std::env::join_paths(
        std::iter::once(temp.path().to_path_buf()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )),
    )
    .unwrap()
}

#[test]
fn generated_shell_snippets_parse_with_available_shells() {
    for (shell, args) in [
        ("sh", vec!["-n"]),
        ("bash", vec!["-n"]),
        ("zsh", vec!["-n"]),
        ("fish", vec!["--no-execute"]),
        ("nu", vec!["--commands"]),
    ] {
        let output = cmd()
            .args(["init", shell])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let snippet = String::from_utf8(output).unwrap();
        check_shell_syntax(shell, &args, &snippet);
    }

    let powershell = cmd()
        .args(["init", "powershell"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    check_powershell_syntax(&String::from_utf8(powershell).unwrap());
}

fn check_use_syntax(shell: &str, snippet: &str) {
    match shell {
        "sh" | "bash" | "zsh" => check_shell_syntax(shell, &["-n"], snippet),
        "fish" => check_shell_syntax(shell, &["--no-execute"], snippet),
        "nu" => check_shell_syntax(shell, &["--commands"], snippet),
        "powershell" => check_powershell_syntax(snippet),
        _ => unreachable!("tested shell is supported"),
    }
}

fn check_shell_syntax(shell: &str, args: &[&str], snippet: &str) {
    if shell == "nu" {
        let Ok(output) = ProcessCommand::new(shell).args(args).arg(snippet).output() else {
            return;
        };
        assert!(
            output.status.success(),
            "{shell} rejected generated snippet: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let Ok(mut child) = ProcessCommand::new(shell)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return;
    };

    child
        .stdin
        .take()
        .expect("shell stdin is piped")
        .write_all(snippet.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{shell} rejected generated snippet: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn check_powershell_syntax(snippet: &str) {
    let parser = "$tokens = $null; $parseErrors = $null; [System.Management.Automation.Language.Parser]::ParseInput([Console]::In.ReadToEnd(), [ref]$tokens, [ref]$parseErrors) | Out-Null; if ($parseErrors.Count -gt 0) { $parseErrors | ForEach-Object { [Console]::Error.WriteLine($_.Message) }; exit 1 }";
    let Ok(mut child) = ProcessCommand::new("pwsh")
        .args(["-NoProfile", "-Command", parser])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return;
    };

    child
        .stdin
        .take()
        .expect("PowerShell stdin is piped")
        .write_all(snippet.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "PowerShell rejected generated snippet: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
