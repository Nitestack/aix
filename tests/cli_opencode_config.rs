use assert_cmd::Command;
use std::path::{Path, PathBuf};

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

fn toml_string(path: &Path) -> String {
    serde_json::to_string(&path.to_string_lossy()).unwrap()
}

fn write_file(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

#[test]
fn profile_opencode_sources_parse_and_dry_run_resolves_paths_without_credentials() {
    let root = assert_fs::TempDir::new().unwrap();
    let root_path = root.path();
    let config_dir = root_path.join("aix-config");
    let config_path = config_dir.join("aix.toml");
    let cwd = root_path.join("launch-cwd");
    let home = root_path.join("home");
    let app_alpha = root_path.join("profiles/alpha/opencode.json");
    let cli_alpha = home.join(".config/opencode/cli.json");
    let app_beta = root_path.join("profiles/beta/opencode.json");
    let cli_beta = root_path.join("profiles/beta/cli.json");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    write_file(&app_alpha, "{\"username\":\"alpha\"}\n");
    write_file(&cli_alpha, "{\"animations\":false}\n");
    write_file(&app_beta, "{\"username\":\"beta\"}\n");
    write_file(&cli_beta, "{\"animations\":true}\n");

    let config = format!(
        r#"
[endpoint]
base_url = {{ command = "exit 42" }}

[profiles.alpha]
api_key = {{ command = "exit 41" }}
[profiles.alpha.tool_configs.opencode]
config_file = "../profiles/alpha/opencode.json"
cli_config_file = "~/.config/opencode/cli.json"

[profiles.beta]
api_key = {{ command = "exit 41" }}
[profiles.beta.tool_configs.opencode]
config_file = {}
cli_config_file = "../profiles/beta/cli.json"

[tools.opencode]
command = "custom-opencode-wrapper"
api_format = "openai"
"#,
        toml_string(&app_beta)
    );
    write_file(&config_path, &config);

    cmd()
        .env("AIX_CONFIG", &config_path)
        .env("HOME", &home)
        .current_dir(&cwd)
        .args(["config", "validate"])
        .assert()
        .success();

    for (profile, app_path, cli_path) in [
        (
            "alpha",
            config_dir.join("../profiles/alpha/opencode.json"),
            cli_alpha,
        ),
        (
            "beta",
            app_beta,
            config_dir.join("../profiles/beta/cli.json"),
        ),
    ] {
        let output = cmd()
            .env("AIX_CONFIG", &config_path)
            .env("HOME", &home)
            .current_dir(&cwd)
            .args(["opencode", profile, "--dry-run", "--non-interactive"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(profile), "missing profile in {stderr}");
        assert!(
            stderr.contains(&app_path.to_string_lossy().to_string()),
            "missing app path in {stderr}"
        );
        assert!(
            stderr.contains(&cli_path.to_string_lossy().to_string()),
            "missing CLI path in {stderr}"
        );
        assert!(
            stderr.contains("--standalone"),
            "private server not reported: {stderr}"
        );
        assert!(
            !stderr.contains("SHOULD_NOT_RUN"),
            "secret source output leaked: {stderr}"
        );
    }
}

#[test]
fn profile_tool_config_rejects_unknown_tool_names_and_unknown_fields() {
    let root = assert_fs::TempDir::new().unwrap();
    let config = root.path().join("aix.toml");
    for invalid in [
        r#"
[endpoint]
base_url = "https://gateway.invalid"
[profiles.work]
api_key = "key"
[profiles.work.tool_configs.unknown]
config_file = "/tmp/config.json"
"#,
        r#"
[endpoint]
base_url = "https://gateway.invalid"
[profiles.work]
api_key = "key"
[profiles.work.tool_configs.opencode]
inline_config = "{}"
"#,
        r#"
[endpoint]
base_url = "https://gateway.invalid"
[profiles.work]
api_key = "key"
[tools.opencode]
api_format = "openai"
config_file = "/tmp/config.json"
"#,
    ] {
        write_file(&config, invalid);
        cmd()
            .env("AIX_CONFIG", &config)
            .args(["config", "validate"])
            .assert()
            .failure();
    }
}

#[test]
fn unavailable_opencode_source_warns_and_falls_back_independently() {
    let root = assert_fs::TempDir::new().unwrap();
    let root_path = root.path();
    let config = root_path.join("aix.toml");
    let wrong_kind = root_path.join("not-a-file");
    let selected_cli = root_path.join("profile-cli.json");
    std::fs::create_dir_all(&wrong_kind).unwrap();
    write_file(&selected_cli, "{\"animations\":true}\n");
    write_file(
        &config,
        &format!(
            r#"
[endpoint]
base_url = "https://gateway.invalid"
[profiles.work]
api_key = "key"
[profiles.work.tool_configs.opencode]
config_file = {}
cli_config_file = {}
[tools.opencode]
api_format = "openai"
"#,
            toml_string(&wrong_kind),
            toml_string(&selected_cli),
        ),
    );

    let output = cmd()
        .env("AIX_CONFIG", &config)
        .args(["opencode", "work", "--dry-run", "--non-interactive"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning"), "missing warning: {stderr}");
    assert!(stderr.contains("work"), "missing profile: {stderr}");
    assert!(stderr.contains("opencode"), "missing tool: {stderr}");
    assert!(stderr.contains("config_file"), "missing source: {stderr}");
    assert!(
        stderr.contains("fallback"),
        "missing fallback decision: {stderr}"
    );
    assert!(
        stderr.contains("cli_config_file"),
        "valid counterpart was not reported: {stderr}"
    );
    assert!(
        stderr.contains("--standalone"),
        "valid CLI source did not remain active: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn when_all_selected_sources_are_unavailable_the_existing_environment_is_unchanged() {
    use std::os::unix::fs::PermissionsExt;

    let root = assert_fs::TempDir::new().unwrap();
    let root_path = root.path();
    let config = root_path.join("aix.toml");
    let wrapper = root_path.join("opencode-wrapper");
    let global_dir = root_path.join("normal-global-config");
    write_file(
        &wrapper,
        r#"#!/bin/sh
test "$#" -eq 0 || exit 91
test "$OPENCODE_CONFIG_DIR" = "$AIX_EXPECTED_CONFIG_DIR" || exit 92
test "$OPENCODE_CONFIG" = "normal-explicit-source" || exit 93
test "$OPENCODE_CONFIG_CONTENT" = "normal-inline-source" || exit 94
test "$OPENCODE_SERVER_URL" = "http://normal-server.invalid" || exit 95
printf 'standard behavior retained\n'
"#,
    );
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    write_file(
        &config,
        &format!(
            r#"
[endpoint]
base_url = "https://gateway.invalid"
[profiles.work]
api_key = "key"
[profiles.work.tool_configs.opencode]
config_file = {}
cli_config_file = {}
[tools.opencode]
command = {}
api_format = "openai"
"#,
            toml_string(&root_path.join("missing-app.json")),
            toml_string(&root_path.join("missing-cli.json")),
            toml_string(&wrapper),
        ),
    );

    let output = cmd()
        .env("AIX_CONFIG", &config)
        .env("OPENCODE_CONFIG_DIR", &global_dir)
        .env("AIX_EXPECTED_CONFIG_DIR", &global_dir)
        .env("OPENCODE_CONFIG", "normal-explicit-source")
        .env("OPENCODE_CONFIG_CONTENT", "normal-inline-source")
        .env("OPENCODE_SERVER_URL", "http://normal-server.invalid")
        .args(["opencode", "work", "--non-interactive"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("config_file"),
        "missing app fallback warning: {stderr}"
    );
    assert!(
        stderr.contains("cli_config_file"),
        "missing CLI fallback warning: {stderr}"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("standard behavior retained"));
    assert!(
        !stderr.contains("--standalone"),
        "fallback unexpectedly selected a private server: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn active_app_source_stages_native_resources_without_leaking_other_global_config() {
    use std::os::unix::fs::PermissionsExt;

    let root = assert_fs::TempDir::new().unwrap();
    let root_path = root.path();
    let config_path = root_path.join("config/aix.toml");
    let cwd = root_path.join("cwd");
    let app_source = root_path.join("profile-app");
    let inherited_root = root_path.join("inherited-global");
    let profile_root = root_path.join("profile-global");
    let tool_root = root_path.join("tool-global");
    let wrapper = root_path.join("opencode-wrapper");
    let data_home = root_path.join("data");
    std::fs::create_dir_all(&cwd).unwrap();
    write_file(
        &app_source.join("opencode.json"),
        "{\"username\":\"SELECTED_APP_MARKER\"}\n",
    );
    write_file(
        &app_source.join("resources/prompt.txt"),
        "SELECTED_RESOURCE_MARKER\n",
    );
    for directory in [&inherited_root, &profile_root, &tool_root] {
        write_file(
            &directory.join("opencode.json"),
            "{\"username\":\"GLOBAL_APP_MUST_NOT_LEAK\"}\n",
        );
        write_file(
            &directory.join("cli.json"),
            "{\"plugins\":[\"STANDARD_CLI_MARKER\"]}\n",
        );
    }
    write_file(
        &wrapper,
        r#"#!/bin/sh
test "$1" = "--standalone" || exit 71
shift
test -f "$OPENCODE_CONFIG_DIR/opencode.json" || exit 72
grep -q SELECTED_APP_MARKER "$OPENCODE_CONFIG_DIR/opencode.json" || exit 73
grep -q SELECTED_RESOURCE_MARKER "$OPENCODE_CONFIG_DIR/resources/prompt.txt" || exit 74
grep -q STANDARD_CLI_MARKER "$OPENCODE_CONFIG_DIR/cli.json" || exit 75
if grep -q GLOBAL_APP_MUST_NOT_LEAK "$OPENCODE_CONFIG_DIR/opencode.json"; then exit 76; fi
test -z "${OPENCODE_CONFIG+x}" || exit 77
test -z "${OPENCODE_CONFIG_CONTENT+x}" || exit 78
test -z "${OPENCODE_SERVER+x}" || exit 79
test -z "${OPENCODE_SERVER_URL+x}" || exit 80
test "$OPENCODE_CLI_CONFIG_CONTENT" = "keep-cli-overlay" || exit 81
test "$AIX_KEEP_ME" = "keep-unrelated" || exit 82
test "$XDG_DATA_HOME" = "$AIX_EXPECTED_DATA_HOME" || exit 83
printf 'stage=%s\n' "$OPENCODE_CONFIG_DIR"
"#,
    );
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    write_file(
        &config_path,
        &format!(
            r#"
[endpoint]
base_url = "https://gateway.invalid"
[profiles.work]
api_key = "profile-key"
[profiles.work.env]
OPENCODE_CONFIG_DIR = {}
OPENCODE_CONFIG = "profile-explicit-config"
OPENCODE_CONFIG_CONTENT = "profile-inline-content"
OPENCODE_SERVER = "http://profile-server.invalid"
OPENCODE_CLI_CONFIG_CONTENT = "keep-cli-overlay"
AIX_KEEP_ME = "keep-unrelated"
[profiles.work.tool_configs.opencode]
config_file = "../profile-app/opencode.json"
[tools.opencode]
command = {}
api_format = "openai"
[tools.opencode.env]
OPENCODE_CONFIG_DIR = {}
OPENCODE_CONFIG = "tool-explicit-config"
OPENCODE_CONFIG_CONTENT = "tool-inline-content"
OPENCODE_SERVER_URL = "http://tool-server.invalid"
"#,
            toml_string(&profile_root),
            toml_string(&wrapper),
            toml_string(&tool_root),
        ),
    );
    let before = [
        app_source.join("opencode.json"),
        app_source.join("resources/prompt.txt"),
        tool_root.join("opencode.json"),
        tool_root.join("cli.json"),
    ]
    .map(|path| std::fs::read(path).unwrap());

    let output = cmd()
        .env("AIX_CONFIG", &config_path)
        .env("OPENCODE_CONFIG_DIR", &inherited_root)
        .env("OPENCODE_CONFIG", "inherited-explicit-config")
        .env("OPENCODE_CONFIG_CONTENT", "inherited-inline-content")
        .env("OPENCODE_SERVER", "http://inherited-server.invalid")
        .env("OPENCODE_SERVER_URL", "http://inherited-url.invalid")
        .env("OPENCODE_CLI_CONFIG_CONTENT", "keep-cli-overlay")
        .env("AIX_KEEP_ME", "keep-unrelated")
        .env("XDG_DATA_HOME", &data_home)
        .env("AIX_EXPECTED_DATA_HOME", &data_home)
        .current_dir(&cwd)
        .args(["opencode", "work", "--non-interactive"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let output = String::from_utf8(output).unwrap();
    let stage = output
        .lines()
        .find_map(|line| line.strip_prefix("stage="))
        .expect("wrapper reported staged config path");
    assert_ne!(PathBuf::from(stage), inherited_root);
    assert_ne!(PathBuf::from(stage), profile_root);
    assert_ne!(PathBuf::from(stage), tool_root);
    for (path, expected) in [
        (app_source.join("opencode.json"), &before[0]),
        (app_source.join("resources/prompt.txt"), &before[1]),
        (tool_root.join("opencode.json"), &before[2]),
        (tool_root.join("cli.json"), &before[3]),
    ] {
        assert_eq!(
            std::fs::read(&path).unwrap(),
            *expected,
            "source changed: {path:?}"
        );
    }

    let run_output = cmd()
        .env("AIX_CONFIG", &config_path)
        .env("OPENCODE_CONFIG_DIR", &inherited_root)
        .env("OPENCODE_CONFIG", "inherited-explicit-config")
        .env("OPENCODE_CONFIG_CONTENT", "inherited-inline-content")
        .env("OPENCODE_SERVER", "http://inherited-server.invalid")
        .env("OPENCODE_SERVER_URL", "http://inherited-url.invalid")
        .env("OPENCODE_CLI_CONFIG_CONTENT", "keep-cli-overlay")
        .env("AIX_KEEP_ME", "keep-unrelated")
        .env("XDG_DATA_HOME", &data_home)
        .env("AIX_EXPECTED_DATA_HOME", &data_home)
        .env("AIX_STATE_DIR", root_path.join("run-state"))
        .current_dir(&cwd)
        .args(["run", "--profile", "work", "--", "opencode"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let run_output = String::from_utf8(run_output).unwrap();
    let run_stage = run_output
        .lines()
        .find_map(|line| line.strip_prefix("stage="))
        .expect("configured aix run launched the OpenCode wrapper");
    assert_ne!(PathBuf::from(run_stage), inherited_root);
    assert!(
        !Path::new(run_stage).exists(),
        "run staging directory was not cleaned up"
    );
}

#[test]
fn configured_aix_run_uses_profile_config_and_rejects_conflicts_before_secrets() {
    let root = assert_fs::TempDir::new().unwrap();
    let config = root.path().join("aix.toml");
    let selected = root.path().join("profile/opencode.json");
    let executable = std::env::current_exe().unwrap();
    write_file(&selected, "{\"username\":\"not-printed\"}\n");
    write_file(
        &config,
        &format!(
            r#"
[endpoint]
base_url = "https://gateway.invalid"
[profiles.work]
api_key = {{ command = "exit 41" }}
[profiles.work.tool_configs.opencode]
config_file = {}
[tools.opencode]
command = {}
api_format = "openai"
"#,
            toml_string(&selected),
            toml_string(&executable),
        ),
    );

    let output = cmd()
        .env("AIX_CONFIG", &config)
        .args([
            "run",
            "--profile",
            "work",
            "--dry-run",
            "--lease",
            "--budget",
            "1",
            "--duration",
            "5m",
            "--",
            "opencode",
            "--model",
            "openai/test-model",
        ])
        .assert()
        .success()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("config_file: selected"),
        "missing selection: {stderr}"
    );
    assert!(stderr.contains(&selected.to_string_lossy().to_string()));
    assert!(stderr.contains("--standalone"));
    assert!(!stderr.contains("not-printed"));
    assert!(
        !stderr.contains("secret command"),
        "dry-run resolved a credential: {stderr}"
    );

    let conflict = cmd()
        .env("AIX_CONFIG", &config)
        .args([
            "run",
            "--profile",
            "work",
            "--dry-run",
            "--lease",
            "--budget",
            "1",
            "--duration",
            "5m",
            "--",
            "opencode",
            "--server",
            "http://external.invalid",
        ])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&conflict.stderr);
    assert!(
        stderr.contains("profile-specific"),
        "wrong conflict error: {stderr}"
    );
    assert!(
        !stderr.contains("secret command"),
        "resolved a credential before rejecting: {stderr}"
    );
}

#[test]
fn custom_opencode_config_rejects_external_server_and_config_arguments() {
    let root = assert_fs::TempDir::new().unwrap();
    let config = root.path().join("aix.toml");
    let selected = root.path().join("opencode.json");
    write_file(&selected, "{\"username\":\"not-printed\"}\n");
    write_file(
        &config,
        &format!(
            r#"
[endpoint]
base_url = "https://gateway.invalid"
[profiles.work]
api_key = {{ command = "exit 41" }}
[profiles.work.tool_configs.opencode]
config_file = {}
[tools.opencode]
command = "opencode"
api_format = "openai"
"#,
            toml_string(&selected),
        ),
    );

    for args in [
        vec![
            "opencode",
            "work",
            "--non-interactive",
            "--",
            "--server",
            "http://server.invalid",
        ],
        vec![
            "opencode",
            "work",
            "--non-interactive",
            "--",
            "--config",
            "elsewhere.json",
        ],
    ] {
        let output = cmd()
            .env("AIX_CONFIG", &config)
            .args(args)
            .assert()
            .failure()
            .get_output()
            .clone();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("profile-specific"),
            "missing actionable error: {stderr}"
        );
        assert!(
            !stderr.contains("not-printed"),
            "configuration contents leaked: {stderr}"
        );
    }
}

#[cfg(unix)]
#[test]
fn profile_config_stages_are_removed_when_named_or_managed_launches_fail() {
    use std::os::unix::fs::PermissionsExt;

    let root = assert_fs::TempDir::new().unwrap();
    let root_path = root.path();
    let config = root_path.join("aix.toml");
    let selected = root_path.join("profile/opencode.json");
    let wrapper = root_path.join("opencode-wrapper");
    write_file(&selected, "{\"username\":\"selected\"}\n");
    write_file(
        &wrapper,
        "#!/bin/sh\nprintf '%s\\n' \"$OPENCODE_CONFIG_DIR\" > \"$AIX_CAPTURE_STAGE\"\nexit 7\n",
    );
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    write_file(
        &config,
        &format!(
            r#"
[endpoint]
base_url = "https://gateway.invalid"
[profiles.work]
api_key = "test-key"
[profiles.work.tool_configs.opencode]
config_file = {}
[tools.opencode]
command = {}
api_format = "openai"
"#,
            toml_string(&selected),
            toml_string(&wrapper),
        ),
    );

    for (name, args) in [
        ("named", vec!["opencode", "work", "--non-interactive"]),
        ("run", vec!["run", "--profile", "work", "--", "opencode"]),
    ] {
        let stage_capture = root_path.join(format!("{name}-stage.txt"));
        cmd()
            .env("AIX_CONFIG", &config)
            .env("AIX_CAPTURE_STAGE", &stage_capture)
            .env("AIX_STATE_DIR", root_path.join("run-state"))
            .args(args)
            .assert()
            .failure()
            .code(7);

        let stage = std::fs::read_to_string(stage_capture).unwrap();
        assert!(
            !PathBuf::from(stage.trim()).exists(),
            "{name} left its temporary config stage behind"
        );
    }
}
