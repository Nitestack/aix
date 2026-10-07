use assert_cmd::Command;
use assert_fs::prelude::*;
use std::path::{Path, PathBuf};

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

fn write_config(dir: &assert_fs::TempDir, text: &str) -> PathBuf {
    let path = dir.child("aix.toml");
    path.write_str(text).unwrap();
    path.path().to_path_buf()
}

fn toml_string(value: &Path) -> String {
    toml::Value::String(value.to_string_lossy().into_owned()).to_string()
}

fn child_prints_codex_home(config: &Path, tool: &str, profile: &str, state: &Path) -> String {
    let output = cmd()
        .env("AIX_CONFIG", config)
        .env("AIX_STATE_DIR", state)
        .env("CODEX_HOME", "/ambient/codex-home")
        .env_remove("AIX_MISSING_PROFILE_CODEX_HOME")
        .env_remove("AIX_MISSING_TOOL_CODEX_HOME")
        .args([
            "run",
            "--profile",
            profile,
            "--",
            tool,
            "-c",
            "printf '%s\\n' \"$CODEX_HOME\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(output).unwrap().trim().to_string()
}

#[test]
#[cfg(unix)]
fn codex_config_dir_is_selected_by_profile_for_named_and_run_launches() {
    let dir = assert_fs::TempDir::new().unwrap();
    let work_home = dir.path().join("work-codex");
    let personal_home = dir.path().join("personal-codex");
    std::fs::create_dir_all(&work_home).unwrap();
    std::fs::create_dir_all(&personal_home).unwrap();
    let config = write_config(
        &dir,
        &format!(
            r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "work-key"

[profiles.work.tool_configs.codex]
config_dir = {}

[profiles.personal]
api_key = "personal-key"

[profiles.personal.tool_configs.codex]
config_dir = "personal-codex"

[profiles.personal.env]
CODEX_HOME = {{ env = "AIX_MISSING_PROFILE_CODEX_HOME" }}

[tools.codex]
command = "sh"
api_format = "openai"

[tools.codex.env]
CODEX_HOME = {{ env = "AIX_MISSING_TOOL_CODEX_HOME" }}
"#,
            toml_string(&work_home)
        ),
    );
    let state = dir.path().join("state");

    let named = cmd()
        .env("AIX_CONFIG", &config)
        .env("CODEX_HOME", "/ambient/codex-home")
        .env_remove("AIX_MISSING_PROFILE_CODEX_HOME")
        .env_remove("AIX_MISSING_TOOL_CODEX_HOME")
        .args([
            "codex",
            "work",
            "--",
            "-c",
            "printf '%s\\n' \"$CODEX_HOME\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        String::from_utf8(named).unwrap().trim(),
        work_home.display().to_string()
    );

    assert_eq!(
        child_prints_codex_home(&config, "codex", "personal", &state),
        personal_home.display().to_string(),
        "aix run must use the named logical tool and profile config, despite its configured wrapper"
    );
}

#[test]
#[cfg(unix)]
fn profile_codex_config_also_applies_to_the_explicit_codex_tool_without_global_tool_defaults() {
    use std::os::unix::fs::PermissionsExt;

    let dir = assert_fs::TempDir::new().unwrap();
    let selected = dir.path().join("native-codex");
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir_all(&selected).unwrap();
    std::fs::create_dir_all(&bin_dir).unwrap();
    let fake_codex = bin_dir.join("codex");
    std::fs::write(&fake_codex, "#!/bin/sh\nprintf '%s\\n' \"$CODEX_HOME\"\n").unwrap();
    std::fs::set_permissions(&fake_codex, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = write_config(
        &dir,
        &format!(
            r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "work-key"

[profiles.work.tool_configs.codex]
config_dir = {}
"#,
            toml_string(&selected)
        ),
    );

    let named = cmd()
        .env("AIX_CONFIG", &config)
        .env("PATH", &bin_dir)
        .env("CODEX_HOME", "/ambient/codex-home")
        .args(["codex", "work"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        String::from_utf8(named).unwrap().trim(),
        selected.display().to_string()
    );

    let run = cmd()
        .env("AIX_CONFIG", &config)
        .env("AIX_STATE_DIR", dir.path().join("state"))
        .env("PATH", &bin_dir)
        .env("CODEX_HOME", "/ambient/codex-home")
        .args(["run", "--profile", "work", "--", "codex"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        String::from_utf8(run).unwrap().trim(),
        selected.display().to_string()
    );
}

#[test]
#[cfg(unix)]
fn concurrent_runs_keep_codex_home_selection_bound_to_each_profile() {
    let dir = assert_fs::TempDir::new().unwrap();
    let work_home = dir.path().join("work-codex");
    let personal_home = dir.path().join("personal-codex");
    std::fs::create_dir_all(&work_home).unwrap();
    std::fs::create_dir_all(&personal_home).unwrap();
    let config = write_config(
        &dir,
        &format!(
            r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "work-key"

[profiles.work.tool_configs.codex]
config_dir = {}

[profiles.personal]
api_key = "personal-key"

[profiles.personal.tool_configs.codex]
config_dir = {}

[tools.codex]
command = "sh"
api_format = "openai"
"#,
            toml_string(&work_home),
            toml_string(&personal_home)
        ),
    );
    let launch = |profile: &str| {
        let state = dir.path().join(format!("state-{profile}"));
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_aix"));
        command
            .env("AIX_CONFIG", &config)
            .env("AIX_STATE_DIR", state)
            .env("CODEX_HOME", "/ambient/codex-home")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .args([
                "run",
                "--profile",
                profile,
                "--",
                "codex",
                "-c",
                "sleep 0.1; printf '%s|%s\\n' \"$AIX_PROFILE\" \"$CODEX_HOME\"",
            ]);
        command.spawn().unwrap()
    };

    let work = launch("work");
    let personal = launch("personal");
    let work_output = work.wait_with_output().unwrap();
    let personal_output = personal.wait_with_output().unwrap();
    assert!(work_output.status.success());
    assert!(personal_output.status.success());
    assert_eq!(
        String::from_utf8(work_output.stdout).unwrap().trim(),
        format!("work|{}", work_home.display())
    );
    assert_eq!(
        String::from_utf8(personal_output.stdout).unwrap().trim(),
        format!("personal|{}", personal_home.display())
    );
}

#[test]
#[cfg(unix)]
fn codex_config_dir_supports_home_shorthand() {
    let dir = assert_fs::TempDir::new().unwrap();
    let fake_home = dir.path().join("user-home");
    let selected = fake_home.join("native-codex");
    std::fs::create_dir_all(&selected).unwrap();
    let config = write_config(
        &dir,
        r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "work-key"

[profiles.work.tool_configs.codex]
config_dir = "~/native-codex"

[tools.codex]
command = "sh"
api_format = "openai"
"#,
    );

    let output = cmd()
        .env("AIX_CONFIG", &config)
        .env("HOME", &fake_home)
        .args([
            "codex",
            "work",
            "--",
            "-c",
            "printf '%s\\n' \"$CODEX_HOME\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    assert_eq!(
        String::from_utf8(output).unwrap().trim(),
        selected.display().to_string()
    );
}

#[test]
#[cfg(unix)]
fn unavailable_codex_config_dir_warns_and_preserves_existing_selector_precedence() {
    let dir = assert_fs::TempDir::new().unwrap();
    let not_a_directory = dir.path().join("plain-file");
    std::fs::write(&not_a_directory, "not a directory").unwrap();

    for unavailable in [dir.path().join("missing-home"), not_a_directory] {
        let config = write_config(
            &dir,
            &format!(
                r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "work-key"

[profiles.work.tool_configs.codex]
config_dir = {}

[profiles.work.env]
CODEX_HOME = "profile-selector"

[tools.codex]
command = "sh"
api_format = "openai"

[tools.codex.env]
CODEX_HOME = "tool-selector"
"#,
                toml_string(&unavailable)
            ),
        );

        let output = cmd()
            .env("AIX_CONFIG", &config)
            .env("CODEX_HOME", "/ambient/codex-home")
            .args([
                "codex",
                "work",
                "--",
                "-c",
                "printf '%s\\n' \"$CODEX_HOME\"",
            ])
            .assert()
            .success()
            .get_output()
            .clone();
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            "tool-selector",
            "fallback must retain normal tool-over-profile-over-inherited env precedence"
        );
        let stderr = String::from_utf8(output.stderr).unwrap();
        for marker in [
            "profile 'work'",
            "tool 'codex'",
            &unavailable.display().to_string(),
            &config.display().to_string(),
            "standard Codex configuration",
        ] {
            assert!(stderr.contains(marker), "missing {marker:?}: {stderr}");
        }
        assert!(!stderr.contains("work-key"));
    }
}

#[test]
#[cfg(unix)]
fn profile_codex_config_does_not_apply_to_other_logical_tools_or_shell_commands() {
    let dir = assert_fs::TempDir::new().unwrap();
    let selected = dir.path().join("native-codex");
    std::fs::create_dir_all(&selected).unwrap();
    let config = write_config(
        &dir,
        &format!(
            r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "work-key"

[profiles.work.tool_configs.codex]
config_dir = {}

[tools.shell]
command = "sh"
api_format = "openai"
"#,
            toml_string(&selected)
        ),
    );

    let output = cmd()
        .env("AIX_CONFIG", &config)
        .env("CODEX_HOME", "/ambient/codex-home")
        .args([
            "run",
            "--profile",
            "work",
            "--",
            "shell",
            "-c",
            "printf '%s\\n' \"$CODEX_HOME\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    assert_eq!(
        String::from_utf8(output).unwrap().trim(),
        "/ambient/codex-home"
    );
}

#[test]
#[cfg(unix)]
fn a_codex_executable_filename_does_not_select_the_codex_profile_config() {
    use std::os::unix::fs::PermissionsExt;

    let dir = assert_fs::TempDir::new().unwrap();
    let selected = dir.path().join("native-codex");
    std::fs::create_dir_all(&selected).unwrap();
    let fake_codex = dir.child("codex");
    fake_codex
        .write_str("#!/bin/sh\nprintf '%s\\n' \"$CODEX_HOME\"\n")
        .unwrap();
    std::fs::set_permissions(fake_codex.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = write_config(
        &dir,
        &format!(
            r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "work-key"

[profiles.work.tool_configs.codex]
config_dir = {}
"#,
            toml_string(&selected)
        ),
    );

    let output = cmd()
        .env("AIX_CONFIG", &config)
        .env("CODEX_HOME", "/ambient/codex-home")
        .env("AIX_STATE_DIR", dir.path().join("state"))
        .args([
            "run",
            "--profile",
            "work",
            "--",
            fake_codex.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    assert_eq!(
        String::from_utf8(output).unwrap().trim(),
        "/ambient/codex-home"
    );
}

#[test]
fn codex_config_dry_run_reports_sources_without_resolving_credentials() {
    let dir = assert_fs::TempDir::new().unwrap();
    let selected = dir.path().join("native-codex");
    std::fs::create_dir_all(&selected).unwrap();
    let native_config = selected.join("config.toml");
    std::fs::write(&native_config, "model = 'native-model'\n").unwrap();
    let native_config_before = std::fs::read(&native_config).unwrap();
    let state = dir.path().join("dry-run-state");
    let config = write_config(
        &dir,
        &format!(
            r#"
[endpoint]
base_url = {{ env = "AIX_MISSING_URL_SENTINEL" }}

[profiles.work]
api_key = {{ env = "AIX_MISSING_KEY_SENTINEL" }}

[profiles.work.tool_configs.codex]
config_dir = {}

[tools.codex]
command = "missing-codex-wrapper"
api_format = "openai"
"#,
            toml_string(&selected)
        ),
    );

    let output = cmd()
        .env("AIX_CONFIG", &config)
        .env("AIX_STATE_DIR", &state)
        .env_remove("AIX_MISSING_URL_SENTINEL")
        .env_remove("AIX_MISSING_KEY_SENTINEL")
        .args(["codex", "work", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .clone();
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        rendered.contains(&selected.display().to_string()),
        "{rendered}"
    );
    assert!(
        rendered.contains(&config.display().to_string()),
        "{rendered}"
    );
    assert!(rendered.contains("CODEX_HOME"), "{rendered}");
    assert!(!rendered.contains("AIX_MISSING_URL_SENTINEL"));
    assert!(!rendered.contains("AIX_MISSING_KEY_SENTINEL"));
    assert!(!state.exists(), "dry-run must not create run state");
    assert_eq!(std::fs::read(native_config).unwrap(), native_config_before);
}

#[test]
#[cfg(unix)]
fn codex_user_config_bypass_argument_is_rejected_when_custom_config_is_active() {
    use std::os::unix::fs::PermissionsExt;

    let dir = assert_fs::TempDir::new().unwrap();
    let selected = dir.path().join("native-codex");
    std::fs::create_dir_all(&selected).unwrap();
    let marker = dir.path().join("launched");
    let fake_tool = dir.child("fake-codex");
    fake_tool
        .write_str(&format!("#!/bin/sh\ntouch {}\n", marker.display()))
        .unwrap();
    std::fs::set_permissions(fake_tool.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = write_config(
        &dir,
        &format!(
            r#"
[endpoint]
base_url = {{ env = "AIX_MISSING_CONFLICT_URL" }}

[profiles.work]
api_key = {{ env = "AIX_MISSING_CONFLICT_KEY" }}

[profiles.work.tool_configs.codex]
config_dir = {}

[tools.codex]
command = {}
api_format = "openai"
"#,
            toml_string(&selected),
            toml_string(fake_tool.path())
        ),
    );

    for argument in [
        "--ignore-user-config",
        "--oss",
        "--local-provider=ollama",
        "--remote=wss://remote.invalid",
    ] {
        let output = cmd()
            .env("AIX_CONFIG", &config)
            .env_remove("AIX_MISSING_CONFLICT_URL")
            .env_remove("AIX_MISSING_CONFLICT_KEY")
            .args(["codex", "work", "--", argument])
            .assert()
            .failure()
            .get_output()
            .stderr
            .clone();

        let stderr = String::from_utf8(output).unwrap();
        assert!(
            stderr.contains("conflicts with the selected Codex configuration directory"),
            "{argument}: {stderr}"
        );
        assert!(!stderr.contains("AIX_MISSING_CONFLICT"), "{stderr}");
        assert!(
            !marker.exists(),
            "the rejected launch must not run the tool"
        );
    }
}

#[test]
#[cfg(unix)]
fn selected_codex_failure_is_not_retried_with_standard_configuration() {
    use std::os::unix::fs::PermissionsExt;

    let dir = assert_fs::TempDir::new().unwrap();
    let selected = dir.path().join("native-codex");
    std::fs::create_dir_all(&selected).unwrap();
    std::fs::write(selected.join("config.toml"), "invalid_native_config = [\n").unwrap();
    let invocation_log = dir.path().join("invocations");
    let fake_codex = dir.child("fake-codex");
    fake_codex
        .write_str(&format!(
            "#!/bin/sh\nprintf '%s\\n' \"$CODEX_HOME\" >> {}\nif grep -q invalid_native_config \"$CODEX_HOME/config.toml\"; then exit 23; fi\nexit 0\n",
            invocation_log.display()
        ))
        .unwrap();
    std::fs::set_permissions(fake_codex.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = write_config(
        &dir,
        &format!(
            r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "work-key"

[profiles.work.tool_configs.codex]
config_dir = {}

[tools.codex]
command = {}
api_format = "openai"
"#,
            toml_string(&selected),
            toml_string(fake_codex.path())
        ),
    );

    cmd()
        .env("AIX_CONFIG", &config)
        .env("CODEX_HOME", "/ambient/codex-home")
        .args(["codex", "work"])
        .assert()
        .code(23);

    let invocations = std::fs::read_to_string(invocation_log).unwrap();
    assert_eq!(invocations.lines().count(), 1, "{invocations}");
    assert_eq!(invocations.trim(), selected.display().to_string());
}

#[test]
#[cfg(unix)]
fn aix_codex_gateway_connection_overrides_conflicting_native_and_caller_settings() {
    use std::os::unix::fs::PermissionsExt;

    let dir = assert_fs::TempDir::new().unwrap();
    let selected = dir.path().join("native-codex");
    std::fs::create_dir_all(&selected).unwrap();
    let native_config = selected.join("config.toml");
    let native_config_contents = "model_provider = 'aix_api_key_gateway'\n[model_providers.aix_api_key_gateway]\nname = 'conflicting provider'\nbase_url = 'https://native.invalid/v1'\nenv_key = 'NATIVE_TOKEN'\nwire_api = 'chat'\n";
    std::fs::write(&native_config, native_config_contents).unwrap();
    let fake_codex = dir.child("fake-codex");
    fake_codex
        .write_str(
            "#!/bin/sh\nprintf 'CODEX_HOME=%s\\n' \"$CODEX_HOME\"\nfor arg in \"$@\"; do printf 'ARG=%s\\n' \"$arg\"; done\n",
        )
        .unwrap();
    std::fs::set_permissions(fake_codex.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = write_config(
        &dir,
        &format!(
            r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "work-key"

[profiles.work.tool_configs.codex]
config_dir = {}

[tools.codex]
command = {}
api_format = "openai"
local_gateway = true
"#,
            toml_string(&selected),
            toml_string(fake_codex.path())
        ),
    );

    let output = cmd()
        .env("AIX_CONFIG", &config)
        .env("AIX_STATE_DIR", dir.path().join("state"))
        .env("CODEX_HOME", "/ambient/codex-home")
        .args([
            "run",
            "--profile",
            "work",
            "--",
            "codex",
            "-c",
            "model_providers.aix_api_key_gateway.base_url=\"https://caller.invalid/v1\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();
    assert!(
        output.contains(&format!("CODEX_HOME={}", selected.display())),
        "selected profile configuration must be active: {output}"
    );
    let args: Vec<_> = output
        .lines()
        .filter_map(|line| line.strip_prefix("ARG="))
        .collect();
    let caller_override = args
        .iter()
        .position(|arg| arg.contains("https://caller.invalid/v1"))
        .expect("ordinary caller overrides remain supported");
    let aix_override = args
        .iter()
        .position(|arg| {
            arg.starts_with("model_providers.aix_api_key_gateway.base_url=")
                && arg.contains("http://127.0.0.1:")
        })
        .expect("aix's local gateway remains the selected Codex connection");
    assert!(aix_override > caller_override);
    assert!(args.contains(&"model_provider=\"aix_api_key_gateway\""));
    assert_eq!(
        std::fs::read_to_string(native_config).unwrap(),
        native_config_contents
    );
}

#[test]
#[cfg(unix)]
fn direct_aix_codex_connection_overrides_native_config_after_caller_args() {
    use std::os::unix::fs::PermissionsExt;

    let dir = assert_fs::TempDir::new().unwrap();
    let selected = dir.path().join("native-codex");
    std::fs::create_dir_all(&selected).unwrap();
    let native_config = selected.join("config.toml");
    let native_config_contents = "model_provider = 'aix_api_key_gateway'\n[model_providers.aix_api_key_gateway]\nname = 'conflicting provider'\nbase_url = 'https://native.invalid/v1'\nenv_key = 'NATIVE_TOKEN'\nwire_api = 'chat'\n";
    std::fs::write(&native_config, native_config_contents).unwrap();
    let fake_codex = dir.child("fake-codex");
    fake_codex
        .write_str("#!/bin/sh\nfor arg in \"$@\"; do printf 'ARG=%s\\n' \"$arg\"; done\n")
        .unwrap();
    std::fs::set_permissions(fake_codex.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = write_config(
        &dir,
        &format!(
            r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "work-key"

[profiles.work.tool_configs.codex]
config_dir = {}

[tools.codex]
command = {}
api_format = "openai"
"#,
            toml_string(&selected),
            toml_string(fake_codex.path())
        ),
    );

    let output = cmd()
        .env("AIX_CONFIG", &config)
        .args([
            "codex",
            "work",
            "--",
            "--profile",
            "native",
            "-c",
            "model_providers.aix_api_key_gateway.base_url=\"https://caller.invalid/v1\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();
    let args: Vec<_> = output
        .lines()
        .filter_map(|line| line.strip_prefix("ARG="))
        .collect();
    let caller_override = args
        .iter()
        .position(|arg| arg.contains("https://caller.invalid/v1"))
        .expect("ordinary caller overrides remain supported");
    let aix_override = args
        .iter()
        .position(|arg| {
            arg.starts_with("model_providers.aix_api_key_gateway.base_url=")
                && arg.contains("https://gateway.invalid/v1")
        })
        .expect("aix's selected profile connection overrides native config");
    assert!(aix_override > caller_override);
    assert!(args.contains(&"model_provider=\"aix_api_key_gateway\""));
    assert_eq!(
        std::fs::read_to_string(native_config).unwrap(),
        native_config_contents
    );
}

#[test]
fn profile_tool_config_schema_is_optional_and_rejects_unknown_tool_names() {
    let dir = assert_fs::TempDir::new().unwrap();
    let valid = write_config(
        &dir,
        r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "work-key"

[profiles.work.tool_configs.codex]
config_dir = "/existing/codex-home"
"#,
    );
    cmd()
        .env("AIX_CONFIG", valid)
        .args(["config", "validate"])
        .assert()
        .success();

    for invalid_config in [
        r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "work-key"

[profiles.work.tool_configs.claude]
config_dir = "/existing/claude-home"
"#,
        r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.work]
api_key = "work-key"

[profiles.work.tool_configs.codex]
directory = "/existing/codex-home"
"#,
    ] {
        let invalid = write_config(&dir, invalid_config);
        cmd()
            .env("AIX_CONFIG", invalid)
            .args(["config", "validate"])
            .assert()
            .failure();
    }
}
