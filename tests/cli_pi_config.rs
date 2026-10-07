use assert_cmd::Command;
use assert_fs::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::thread;

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

fn script(root: &Path, name: &str, body: &str) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn config_file(contents: &str) -> assert_fs::NamedTempFile {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(contents).unwrap();
    file
}

fn quote(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}

fn pi_config(agent_dir: &Path, command: &Path, extra: &str) -> String {
    format!(
        r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "profile-key"

[profiles.work.tool_configs.pi]
agent_dir = {}

{}

[tools.pi]
command = {}
api_format = "openai"
"#,
        quote(&agent_dir.to_string_lossy()),
        extra,
        quote(&command.to_string_lossy()),
    )
}

#[test]
#[cfg(unix)]
fn selected_native_directory_wins_and_preserves_shared_pi_data() {
    let root = assert_fs::TempDir::new().unwrap();
    let home = root.path().join("home");
    let normal_agent = home.join(".pi/agent");
    let source = root.path().join("profiles/work/agent");
    fs::create_dir_all(source.join("../shared")).unwrap();
    fs::create_dir_all(&normal_agent).unwrap();
    fs::write(source.join("AGENTS.md"), "WORK_AGENT_CONTEXT\n").unwrap();
    fs::write(
        source.join("../shared/ext.js"),
        "export default () => {};\n",
    )
    .unwrap();
    let source_settings =
        r#"{"theme":"light","extensions":["../shared/ext.js"],"skills":["../shared/skills"]}"#;
    let source_models = r#"{"providers":{"openai":{"baseUrl":"https://native.invalid/v1","models":[{"id":"profile-model","baseUrl":"https://model.invalid/v1"}]},"anthropic":{"baseUrl":"https://saved.invalid"}}}"#;
    fs::write(source.join("settings.json"), source_settings).unwrap();
    fs::write(source.join("models.json"), source_models).unwrap();
    fs::write(
        source.join("keybindings.json"),
        r#"{"tui.editor.deleteWordBackward":"ctrl+alt+w"}"#,
    )
    .unwrap();
    fs::write(normal_agent.join("settings.json"), "NORMAL_GLOBAL_ONLY\n").unwrap();
    fs::write(normal_agent.join("auth.json"), "SAVED_AUTH_MARKER\n").unwrap();
    fs::write(normal_agent.join("trust.json"), "SAVED_TRUST_MARKER\n").unwrap();

    let probe = script(
        root.path(),
        "pi-wrapper",
        r#"
printf 'agent=%s\n' "$PI_CODING_AGENT_DIR"
printf 'session=%s\n' "${PI_CODING_AGENT_SESSION_DIR-}"
printf 'key=%s\n' "$OPENAI_API_KEY"
printf 'args=%s\n' "$*"
printf 'auth=%s\n' "$(cat "$PI_CODING_AGENT_DIR/auth.json")"
printf 'trust=%s\n' "$(cat "$PI_CODING_AGENT_DIR/trust.json")"
printf 'settings=%s\n' "$(cat "$PI_CODING_AGENT_DIR/settings.json")"
printf 'models=%s\n' "$(cat "$PI_CODING_AGENT_DIR/models.json")"
printf 'marker=%s\n' "$(cat "$PI_CODING_AGENT_DIR/AGENTS.md")"
"#,
    );
    let env_agent = root.path().join("profile-env-agent");
    let tool_agent = root.path().join("tool-env-agent");
    let tool_session = root.path().join("configured-session-store");
    // The profile/tool selectors deliberately conflict with the typed source.
    let extra = format!(
        "[profiles.work.env]\nPI_CODING_AGENT_DIR = {}\n\n[tools.pi.env]\nPI_CODING_AGENT_DIR = {}\nPI_CODING_AGENT_SESSION_DIR = {}\n",
        quote(&env_agent.to_string_lossy()),
        quote(&tool_agent.to_string_lossy()),
        quote(&tool_session.to_string_lossy())
    );
    let config = config_file(&pi_config(&source, &probe, &extra));
    let mut invocation = cmd();
    invocation
        .env("HOME", &home)
        .env("PI_CODING_AGENT_DIR", &normal_agent)
        .env_remove("PI_CODING_AGENT_SESSION_DIR")
        .env("AIX_CONFIG", config.path())
        .args([
            "--profile",
            "work",
            "--non-interactive",
            "pi",
            "--",
            "--provider",
            "anthropic",
            "--model",
            "anthropic/saved-model",
        ]);
    let output = invocation.assert().success().get_output().clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stage_dir = stdout
        .lines()
        .find_map(|line| line.strip_prefix("agent="))
        .unwrap();
    assert!(stage_dir.contains("aix-pi-agent-"));
    assert_ne!(Path::new(stage_dir), source);
    assert!(
        !Path::new(stage_dir).exists(),
        "temporary stage was not cleaned up"
    );
    assert!(stdout.contains(&format!("session={}\n", tool_session.display())));
    assert!(stdout.contains("key=profile-key\n"));
    assert!(stdout.contains(
        "args=--api-key profile-key --provider anthropic --model anthropic/saved-model\n"
    ));
    assert!(stdout.contains("auth=SAVED_AUTH_MARKER\n"));
    assert!(stdout.contains("trust=SAVED_TRUST_MARKER\n"));
    assert!(stdout.contains("WORK_AGENT_CONTEXT\n"));
    assert!(!stdout.contains("NORMAL_GLOBAL_ONLY"));
    assert!(stdout.contains(
        &source
            .join("../shared/ext.js")
            .to_string_lossy()
            .to_string()
    ));
    assert!(stdout.contains("https://gateway.invalid/v1"));
    assert!(stdout.contains("https://saved.invalid"));
    assert_eq!(
        fs::read_to_string(source.join("settings.json")).unwrap(),
        source_settings
    );
    assert_eq!(
        fs::read_to_string(source.join("models.json")).unwrap(),
        source_models
    );
}

#[test]
#[cfg(unix)]
fn configured_tool_run_uses_profile_pi_directory_and_custom_command() {
    let root = assert_fs::TempDir::new().unwrap();
    let home = root.path().join("home");
    let source = root.path().join("profile-agent");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("marker"), "run-profile-marker\n").unwrap();
    let probe = script(
        root.path(),
        "pi-run-wrapper",
        "printf 'agent=%s\\n' \"$PI_CODING_AGENT_DIR\"; printf 'session=%s\\n' \"${PI_CODING_AGENT_SESSION_DIR-}\"; printf 'marker=%s\\n' \"$(cat \"$PI_CODING_AGENT_DIR/marker\")\"; printf 'args=%s\\n' \"$*\"",
    );
    let config = config_file(&pi_config(&source, &probe, ""));
    let state = root.path().join("aix-state");
    let output = cmd()
        .env("HOME", &home)
        .env("PI_CODING_AGENT_DIR", home.join(".pi/agent"))
        .env_remove("PI_CODING_AGENT_SESSION_DIR")
        .env("AIX_STATE_DIR", &state)
        .env("AIX_CONFIG", config.path())
        .args([
            "--profile",
            "work",
            "--non-interactive",
            "run",
            "--",
            "pi",
            "--provider",
            "anthropic",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    assert!(stdout.contains("marker=run-profile-marker\n"));
    assert!(stdout.contains("session=\n"));
    assert!(stdout.contains("args=--api-key profile-key --provider anthropic\n"));
}

#[test]
#[cfg(unix)]
fn profile_key_launch_without_model_keeps_pi_default_model_available() {
    let root = assert_fs::TempDir::new().unwrap();
    let home = root.path().join("home");
    let source = root.path().join("profile-agent");
    fs::create_dir_all(&source).unwrap();
    let probe = script(
        root.path(),
        "pi-default-model-wrapper",
        "printf 'args=%s\\n' \"$*\"",
    );
    let config = config_file(&pi_config(&source, &probe, ""));
    let state = root.path().join("aix-state");

    for launch_args in [vec!["pi"], vec!["run", "--", "pi"]] {
        let output = cmd()
            .env("HOME", &home)
            .env("AIX_STATE_DIR", &state)
            .env("AIX_CONFIG", config.path())
            .args(["--profile", "work", "--non-interactive"])
            .args(&launch_args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "args=--api-key profile-key --models *\n"
        );
    }
}

#[test]
#[cfg(unix)]
fn unavailable_directory_warns_and_preserves_existing_environment_selectors() {
    let root = assert_fs::TempDir::new().unwrap();
    let home = root.path().join("home");
    let missing = root.path().join("missing-agent");
    let profile_agent = root.path().join("profile-env-agent");
    let tool_agent = root.path().join("tool-env-agent");
    let probe = script(
        root.path(),
        "pi-fallback-wrapper",
        "printf 'agent=%s\\n' \"$PI_CODING_AGENT_DIR\"; printf 'args=%s\\n' \"$*\"",
    );
    let extra = format!(
        "[profiles.work.env]\nPI_CODING_AGENT_DIR = {}\n\n[tools.pi.env]\nPI_CODING_AGENT_DIR = {}\n",
        quote(&profile_agent.to_string_lossy()),
        quote(&tool_agent.to_string_lossy())
    );
    let config = config_file(&pi_config(&missing, &probe, &extra));
    let output = cmd()
        .env("HOME", &home)
        .env("PI_CODING_AGENT_DIR", home.join(".pi/agent"))
        .env("AIX_CONFIG", config.path())
        .args([
            "--profile",
            "work",
            "--non-interactive",
            "pi",
            "--",
            "--provider",
            "anthropic",
        ])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stdout.contains(&format!("agent={}\n", tool_agent.display())));
    assert!(stdout.contains("args=--provider anthropic\n"));
    assert!(stderr.contains("profile \"work\""));
    assert!(stderr.contains("tool \"pi\""));
    assert!(stderr.contains("profiles.work.tool_configs.pi.agent_dir"));
    assert!(stderr.contains("using standard Pi configuration"));

    let not_directory = root.path().join("not-a-directory");
    fs::write(&not_directory, "file").unwrap();
    let config = config_file(&pi_config(&not_directory, &probe, &extra));
    let output = cmd()
        .env("HOME", &home)
        .env("PI_CODING_AGENT_DIR", home.join(".pi/agent"))
        .env("AIX_CONFIG", config.path())
        .args(["--profile", "work", "--non-interactive", "pi"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains(&format!("agent={}\n", tool_agent.display())));
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("path is not a directory"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let unreadable = root.path().join("unreadable-agent");
        fs::create_dir(&unreadable).unwrap();
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read_dir(&unreadable).is_err() {
            let config = config_file(&pi_config(&unreadable, &probe, &extra));
            let output = cmd()
                .env("HOME", &home)
                .env("PI_CODING_AGENT_DIR", home.join(".pi/agent"))
                .env("AIX_CONFIG", config.path())
                .args(["--profile", "work", "--non-interactive", "pi"])
                .assert()
                .success()
                .get_output()
                .clone();
            fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o700)).unwrap();
            assert!(String::from_utf8(output.stderr)
                .unwrap()
                .contains("directory is not readable"));
        } else {
            fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
}

#[test]
#[cfg(unix)]
fn dry_run_reports_source_without_resolving_secrets_or_creating_data() {
    let root = assert_fs::TempDir::new().unwrap();
    let home = root.path().join("home");
    let source = root.path().join("selected-agent");
    fs::create_dir_all(&source).unwrap();
    let marker = root.path().join("secret-command-ran");
    let probe_marker = root.path().join("probe-ran");
    let probe = script(
        root.path(),
        "pi-dry-run-wrapper",
        &format!("touch {}", quote(&probe_marker.to_string_lossy())),
    );
    let secret_command = format!("touch {}; printf hidden-key", marker.display());
    let mut config = pi_config(&source, &probe, "");
    config = config.replace(
        "api_key = \"profile-key\"",
        &format!("api_key = {{ command = {} }}", quote(&secret_command)),
    );
    let config = config_file(&config);
    let output = cmd()
        .env("HOME", &home)
        .env("AIX_CONFIG", config.path())
        .env_remove("PI_CODING_AGENT_DIR")
        .env_remove("PI_CODING_AGENT_SESSION_DIR")
        .args(["--profile", "work", "--non-interactive", "pi", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!marker.exists(), "dry-run resolved the profile API key");
    assert!(
        !probe_marker.exists(),
        "dry-run launched the configured Pi command"
    );
    assert!(
        !home.join(".pi/agent").exists(),
        "dry-run mutated the Pi data directory"
    );
    assert!(stderr.contains("profiles.work.tool_configs.pi.agent_dir"));
    assert!(stderr.contains(&source.to_string_lossy().to_string()));
    assert!(!stderr.contains("hidden-key"));
    assert!(!stdout.contains("hidden-key"));
}

#[test]
#[cfg(unix)]
fn explicit_agent_directory_and_api_key_are_rejected_but_native_provider_overrides_work() {
    let root = assert_fs::TempDir::new().unwrap();
    let home = root.path().join("home");
    let source = root.path().join("selected-agent");
    fs::create_dir_all(&source).unwrap();
    let probe = script(root.path(), "pi-arg-wrapper", "printf 'ran\\n'");
    let config = config_file(&pi_config(&source, &probe, ""));

    for args in [
        vec!["--agent-dir".to_string(), root.path().display().to_string()],
        vec!["--agent-dir=/tmp/other".to_string()],
        vec!["--api-key".to_string(), "contradictory".to_string()],
        vec!["--api-key=contradictory".to_string()],
    ] {
        let output = cmd()
            .env("HOME", &home)
            .env("AIX_CONFIG", config.path())
            .env_remove("PI_CODING_AGENT_DIR")
            .args(["--profile", "work", "--non-interactive", "pi", "--"])
            .args(&args)
            .assert()
            .failure()
            .get_output()
            .clone();
        assert!(String::from_utf8(output.stderr)
            .unwrap()
            .contains("conflicts with explicit"));
    }

    let output = cmd()
        .env("HOME", &home)
        .env("AIX_CONFIG", config.path())
        .env_remove("PI_CODING_AGENT_DIR")
        .args([
            "--profile",
            "work",
            "--non-interactive",
            "pi",
            "--",
            "--provider",
            "anthropic",
            "--model",
            "anthropic/native-model",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(String::from_utf8(output).unwrap().contains("ran\n"));
}

#[test]
#[cfg(unix)]
fn no_selection_and_env_commands_keep_existing_behavior() {
    let root = assert_fs::TempDir::new().unwrap();
    let inherited_agent = root.path().join("inherited-agent");
    let probe = script(
        root.path(),
        "pi-no-config-wrapper",
        "printf 'agent=%s\\n' \"${PI_CODING_AGENT_DIR-}\"; printf 'session=%s\\n' \"${PI_CODING_AGENT_SESSION_DIR-}\"; printf 'args=%s\\n' \"$*\"",
    );
    let config = config_file(&format!(
        "[endpoint]\nbase_url = \"https://gateway.invalid\"\n\n[profiles.work]\napi_key = \"profile-key\"\n\n[tools.pi]\ncommand = {}\napi_format = \"openai\"\n",
        quote(&probe.to_string_lossy())
    ));
    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("PI_CODING_AGENT_DIR", &inherited_agent)
        .env_remove("PI_CODING_AGENT_SESSION_DIR")
        .args([
            "--profile",
            "work",
            "--non-interactive",
            "pi",
            "--",
            "native-arg",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    assert!(stdout.contains(&format!("agent={}\n", inherited_agent.display())));
    assert!(stdout.contains("session=\n"));
    assert!(stdout.contains("args=native-arg\n"));

    let env_output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("PI_CODING_AGENT_DIR", &inherited_agent)
        .args([
            "--profile",
            "work",
            "--non-interactive",
            "env",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env_json: serde_json::Value = serde_json::from_slice(&env_output).unwrap();
    assert!(env_json.get("PI_CODING_AGENT_DIR").is_none());
}

#[test]
#[cfg(unix)]
fn typed_selection_is_limited_to_configured_pi_launches() {
    let root = assert_fs::TempDir::new().unwrap();
    let source = root.path().join("selected-agent");
    fs::create_dir_all(&source).unwrap();
    let inherited = root.path().join("inherited-agent");
    let probe = script(
        root.path(),
        "pi-scope-probe",
        "printf 'agent=%s\\n' \"${PI_CODING_AGENT_DIR-}\"",
    );
    let config = config_file(&pi_config(&source, &probe, ""));

    let exec_output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("PI_CODING_AGENT_DIR", &inherited)
        .args([
            "--profile",
            "work",
            "--non-interactive",
            "exec",
            "--",
            probe.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        String::from_utf8(exec_output).unwrap(),
        format!("agent={}\n", inherited.display())
    );

    let shell_output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("PI_CODING_AGENT_DIR", &inherited)
        .env("SHELL", &probe)
        .args(["--profile", "work", "--non-interactive", "shell"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        String::from_utf8(shell_output).unwrap(),
        format!("agent={}\n", inherited.display())
    );

    let env_output = cmd()
        .env("AIX_CONFIG", config.path())
        .args([
            "--profile",
            "work",
            "--non-interactive",
            "env",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env_json: serde_json::Value = serde_json::from_slice(&env_output).unwrap();
    assert!(env_json.get("PI_CODING_AGENT_DIR").is_none());
}

#[test]
#[cfg(unix)]
fn typed_selection_rejects_unsupported_chatgpt_auth_before_launch() {
    let root = assert_fs::TempDir::new().unwrap();
    let source = root.path().join("selected-agent");
    fs::create_dir_all(&source).unwrap();
    let marker = root.path().join("pi-launched");
    let probe = script(
        root.path(),
        "pi-chatgpt-probe",
        &format!("touch {}", quote(&marker.to_string_lossy())),
    );
    let config = config_file(&format!(
        r#"
[profiles.work]
auth = {{ type = "chatgpt" }}

[profiles.work.tool_configs.pi]
agent_dir = {}

[tools.pi]
command = {}
api_format = "openai"
"#,
        quote(&source.to_string_lossy()),
        quote(&probe.to_string_lossy()),
    ));

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .args([
            "--profile",
            "work",
            "--non-interactive",
            "pi",
            "--",
            "--version",
        ])
        .assert()
        .failure()
        .get_output()
        .clone();
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("requires API-key profile authentication"));
    assert!(
        !marker.exists(),
        "Pi launched with unsupported ChatGPT auth"
    );
}

#[test]
#[cfg(unix)]
fn concurrent_profiles_keep_pi_stages_and_credentials_isolated() {
    let root = assert_fs::TempDir::new().unwrap();
    let home = root.path().join("home");
    let normal_agent = home.join(".pi/agent");
    let agent_a = root.path().join("profile-a-agent");
    let agent_b = root.path().join("profile-b-agent");
    fs::create_dir_all(&agent_a).unwrap();
    fs::create_dir_all(&agent_b).unwrap();
    fs::write(agent_a.join("marker"), "profile-a\n").unwrap();
    fs::write(agent_b.join("marker"), "profile-b\n").unwrap();
    let probe = script(
        root.path(),
        "pi-concurrent-wrapper",
        "printf 'agent=%s\\n' \"$PI_CODING_AGENT_DIR\"; printf 'key=%s\\n' \"$OPENAI_API_KEY\"; printf 'marker=%s\\n' \"$(cat \"$PI_CODING_AGENT_DIR/marker\")\"",
    );
    let config = config_file(&format!(
        r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.profile_a]
api_key = "profile-a-key"

[profiles.profile_a.tool_configs.pi]
agent_dir = {}

[profiles.profile_b]
api_key = "profile-b-key"

[profiles.profile_b.tool_configs.pi]
agent_dir = {}

[tools.pi]
command = {}
api_format = "openai"
"#,
        quote(&agent_a.to_string_lossy()),
        quote(&agent_b.to_string_lossy()),
        quote(&probe.to_string_lossy()),
    ));
    let config_path = config.path().to_path_buf();
    let state = root.path().join("aix-state");

    let launches = ["profile_a", "profile_b"].map(|profile| {
        let config_path = config_path.clone();
        let home = home.clone();
        let normal_agent = normal_agent.clone();
        let state = state.clone();
        thread::spawn(move || {
            ProcessCommand::new(env!("CARGO_BIN_EXE_aix"))
                .env("AIX_CONFIG", config_path)
                .env("AIX_STATE_DIR", state)
                .env("HOME", home)
                .env("PI_CODING_AGENT_DIR", normal_agent)
                .env_remove("PI_CODING_AGENT_SESSION_DIR")
                .args(["--profile", profile, "--non-interactive", "run", "--", "pi"])
                .output()
                .unwrap()
        })
    });

    let outputs = launches.map(|launch| launch.join().unwrap());
    let outputs = outputs.map(|output| {
        assert!(
            output.status.success(),
            "launch failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    });
    for (output, profile, key) in [
        (&outputs[0], "profile-a", "profile-a-key"),
        (&outputs[1], "profile-b", "profile-b-key"),
    ] {
        assert!(output.contains(&format!("key={key}\n")));
        assert!(output.contains(&format!("marker={profile}\n")));
        let stage = output
            .lines()
            .find_map(|line| line.strip_prefix("agent="))
            .unwrap();
        assert!(stage.contains("aix-pi-agent-"));
        assert!(!Path::new(stage).exists(), "Pi stage was not cleaned up");
    }
}
