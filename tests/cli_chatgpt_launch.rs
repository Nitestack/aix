use assert_cmd::Command;
use assert_fs::prelude::*;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

const TOOL_CONFIG: &str = r#"
default_profile = "personal"

[profiles.personal]
auth = { type = "chatgpt" }

[profiles.work]
auth = { type = "chatgpt" }

[profiles.personal.env]
ACCESS_TOKEN = "profile-token-override"
PROFILE_SETTING = "profile-value"

[tools.codex]
command = "sh"
api_format = "openai"

[tools.codex.chatgpt]
access_token_env = "ACCESS_TOKEN"
prepend_args = [
  "-c",
  'test "$ACCESS_TOKEN" = access-token-secret-sentinel && test "$PROFILE_SETTING" = profile-value && test "$TOOL_SETTING" = tool-value && test -z "${ANTHROPIC_API_KEY+x}" && test -z "${ANTHROPIC_BASE_URL+x}" && test -z "${OPENAI_API_KEY+x}" && test -z "${OPENAI_BASE_URL+x}" && test -z "${LITELLM_API_KEY+x}" && test -z "${LITELLM_BASE_URL+x}" && test -z "${CODEX_API_KEY+x}" && test -z "${AIX_CODEX_CLEAR+x}" && printf "%s|%s|%s\\n" "$0" "$1" "$2" && env',
  "from-config",
]
clear_env = ["OPENAI_API_KEY", "CODEX_API_KEY", "AIX_CODEX_CLEAR"]

[tools.codex.env]
ACCESS_TOKEN = "tool-token-override"
TOOL_SETTING = "tool-value"
"#;

const UNBOUND_CONFIG: &str = r#"
default_profile = "personal"

[profiles.personal]
auth = { type = "chatgpt" }

[tools.codex]
command = "sh"
api_format = "openai"
"#;

const CODEX_RECIPE: &str = r#"
default_profile = "personal"

[profiles.personal]
auth = { type = "chatgpt" }

[tools.codex]
command = "codex"
api_format = "openai"

[tools.codex.chatgpt]
access_token_env = "ACCESS_TOKEN"
prepend_args = [
  "app-server",
  "--listen",
  "stdio://",
  "-c", 'model_provider="openai_chatgpt_plan"',
  "-c", 'model_providers.openai_chatgpt_plan.name="ChatGPT plan"',
  "-c", 'model_providers.openai_chatgpt_plan.base_url="https://api.openai.com/v1"',
  "-c", 'model_providers.openai_chatgpt_plan.env_key="ACCESS_TOKEN"',
  "-c", 'model_providers.openai_chatgpt_plan.wire_api="responses"',
  "-c", "model_providers.openai_chatgpt_plan.requires_openai_auth=false",
  "-c", "model_providers.openai_chatgpt_plan.supports_websockets=false",
]
clear_env = ["OPENAI_API_KEY", "CODEX_API_KEY"]
"#;

const CODEX_LOCAL_GATEWAY_RECIPE: &str = r#"
default_profile = "personal"

[profiles.personal]
auth = { type = "chatgpt" }

[tools.codex]
command = "codex"
api_format = "openai"

[tools.codex.chatgpt]
transport = "local_gateway"
access_token_env = "ACCESS_TOKEN"
prepend_args = [
  "app-server",
  "--listen",
  "stdio://",
  "-c", 'model_provider="openai_chatgpt_plan"',
  "-c", 'model_providers.openai_chatgpt_plan.name="ChatGPT plan"',
  "-c", 'model_providers.openai_chatgpt_plan.base_url="https://api.openai.com/v1"',
  "-c", 'model_providers.openai_chatgpt_plan.env_key="ACCESS_TOKEN"',
  "-c", 'model_providers.openai_chatgpt_plan.wire_api="responses"',
  "-c", "model_providers.openai_chatgpt_plan.requires_openai_auth=false",
  "-c", "model_providers.openai_chatgpt_plan.supports_websockets=false",
]
clear_env = ["OPENAI_API_KEY", "CODEX_API_KEY"]
"#;

fn write_config(dir: &assert_fs::TempDir, text: &str) -> std::path::PathBuf {
    dir.child("aix.toml").write_str(text).unwrap();
    dir.child("aix.toml").path().to_path_buf()
}

fn write_credentials(auth_dir: &std::path::Path) {
    std::fs::create_dir_all(auth_dir).unwrap();
    let expires_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    std::fs::write(
        auth_dir.join("profile-706572736f6e616c.json"),
        format!(
            r#"{{
  "version": 1,
  "profile": "personal",
  "client_id": "client-id-secret-sentinel",
  "subject": "verified-subject",
  "email": "person@example.test",
  "scopes": ["openid", "chatgpt.tokens.use.direct"],
  "id_token": "id-token-secret-sentinel",
  "access_token": "access-token-secret-sentinel",
  "refresh_token": "refresh-token-secret-sentinel",
  "expires_at": {expires_at},
  "earliest_refresh_at": null
}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        auth_dir.join("profile-776f726b.json"),
        format!(
            r#"{{
  "version": 1,
  "profile": "work",
  "client_id": "other-profile-client-id",
  "subject": "other-verified-subject",
  "email": null,
  "scopes": ["openid", "chatgpt.tokens.use.direct"],
  "id_token": "other-profile-id-token-sentinel",
  "access_token": "other-profile-access-token-sentinel",
  "refresh_token": "other-profile-refresh-token-sentinel",
  "expires_at": {expires_at},
  "earliest_refresh_at": null
}}"#
        ),
    )
    .unwrap();
}

#[test]
#[cfg(unix)]
fn configured_chatgpt_tool_gets_only_the_selected_access_token_and_ordered_args() {
    let dir = assert_fs::TempDir::new().unwrap();
    let config = write_config(&dir, TOOL_CONFIG);
    let auth_dir = dir.path().join("auth");
    write_credentials(&auth_dir);

    let output = cmd()
        .env("AIX_CONFIG", &config)
        .env("AIX_AUTH_DIR", &auth_dir)
        .env("AIX_PROFILE", "stale-profile")
        .env("ANTHROPIC_API_KEY", "ambient-anthropic-key")
        .env("ANTHROPIC_BASE_URL", "https://ambient.invalid")
        .env("OPENAI_API_KEY", "ambient-openai-key")
        .env("OPENAI_BASE_URL", "https://ambient.invalid/v1")
        .env("LITELLM_API_KEY", "ambient-litellm-key")
        .env("LITELLM_BASE_URL", "https://ambient.invalid/v1")
        .env("CODEX_API_KEY", "ambient-codex-key")
        .env("AIX_CODEX_CLEAR", "ambient-clear-value")
        .args(["codex", "personal", "--", "caller-one", "caller-two"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let child = String::from_utf8(output).unwrap();
    assert!(
        child.contains("from-config|caller-one|caller-two"),
        "{child}"
    );
    assert!(child.contains("ACCESS_TOKEN=access-token-secret-sentinel"));
    assert_eq!(
        child
            .lines()
            .filter(|line| line.contains("access-token-secret-sentinel"))
            .collect::<Vec<_>>(),
        ["ACCESS_TOKEN=access-token-secret-sentinel"]
    );
    for secret in [
        "refresh-token-secret-sentinel",
        "id-token-secret-sentinel",
        "client-id-secret-sentinel",
        "other-profile-access-token-sentinel",
        "profile-token-override",
        "tool-token-override",
    ] {
        assert!(!child.contains(secret), "child received {secret}: {child}");
    }
}

#[test]
fn chatgpt_tool_dry_run_is_offline_and_shows_only_effective_names_and_args() {
    let dir = assert_fs::TempDir::new().unwrap();
    let config = write_config(&dir, CODEX_RECIPE);
    let auth_dir = dir.path().join("auth");

    let output = cmd()
        .env("AIX_CONFIG", &config)
        .env("AIX_AUTH_DIR", &auth_dir)
        .args(["codex", "personal", "--dry-run", "--", "caller-arg"])
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
        rendered.contains("Would run: codex app-server --listen stdio://"),
        "{rendered}"
    );
    assert!(rendered.contains("openai_chatgpt_plan"), "{rendered}");
    assert!(rendered.contains("ACCESS_TOKEN"), "{rendered}");
    assert!(!rendered.contains("access-token-secret-sentinel"));
    assert!(
        !auth_dir.exists(),
        "dry-run must not read or create auth state"
    );
}

#[test]
fn local_gateway_dry_run_shows_dynamic_codex_provider_without_auth_or_secrets() {
    let dir = assert_fs::TempDir::new().unwrap();
    let config = write_config(&dir, CODEX_LOCAL_GATEWAY_RECIPE);
    let auth_dir = dir.path().join("auth");

    let output = cmd()
        .env("AIX_CONFIG", &config)
        .env("AIX_AUTH_DIR", &auth_dir)
        .args(["codex", "personal", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .clone();

    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(rendered.contains("transport: local_gateway"), "{rendered}");
    assert!(
        rendered.contains("dynamic local Responses provider/base URL"),
        "{rendered}"
    );
    assert!(rendered.contains("ACCESS_TOKEN"), "{rendered}");
    assert!(!rendered.contains("http://127.0.0.1:"), "{rendered}");
    assert!(!rendered.contains("access-token-secret-sentinel"));
    assert!(
        !auth_dir.exists(),
        "dry-run must not read or create auth state"
    );
}

#[test]
#[cfg(unix)]
fn codex_local_gateway_receives_only_a_per_launch_bearer_and_dynamic_provider_config() {
    use std::os::unix::fs::PermissionsExt;

    let dir = assert_fs::TempDir::new().unwrap();
    let fake_codex = dir.child("fake-codex");
    fake_codex
        .write_str(
            "#!/bin/sh\nprintf 'ACCESS_TOKEN=%s\\n' \"$ACCESS_TOKEN\"\nfor arg in \"$@\"; do printf 'ARG=%s\\n' \"$arg\"; done\nfor name in OPENAI_API_KEY CODEX_API_KEY ANTHROPIC_API_KEY LITELLM_API_KEY; do eval 'value=${'\"$name\"'-unset}'; printf '%s=%s\\n' \"$name\" \"$value\"; done\n",
        )
        .unwrap();
    std::fs::set_permissions(fake_codex.path(), std::fs::Permissions::from_mode(0o700)).unwrap();

    let command = toml::Value::String(fake_codex.path().to_string_lossy().to_string());
    let config_text =
        CODEX_LOCAL_GATEWAY_RECIPE.replace("command = \"codex\"", &format!("command = {command}"));
    let config = write_config(&dir, &config_text);
    let auth_dir = dir.path().join("auth");
    write_credentials(&auth_dir);
    let auth_file = auth_dir.join("profile-706572736f6e616c.json");
    let auth_before = std::fs::read(&auth_file).unwrap();
    let codex_home = dir.path().join("codex-home");
    let codex_sessions = codex_home.join("sessions");
    std::fs::create_dir_all(&codex_sessions).unwrap();
    let codex_config = codex_home.join("config.toml");
    let codex_auth = codex_home.join("auth.json");
    let codex_session = codex_sessions.join("saved-session.json");
    std::fs::write(&codex_config, "model = 'existing-model'\n").unwrap();
    std::fs::write(&codex_auth, r#"{"marker":"existing-auth"}"#).unwrap();
    std::fs::write(&codex_session, r#"{"marker":"existing-session"}"#).unwrap();
    let codex_files_before = [
        std::fs::read(&codex_config).unwrap(),
        std::fs::read(&codex_auth).unwrap(),
        std::fs::read(&codex_session).unwrap(),
    ];

    let output = cmd()
        .env("AIX_CONFIG", &config)
        .env("AIX_AUTH_DIR", &auth_dir)
        .env("CODEX_HOME", &codex_home)
        .env("ACCESS_TOKEN", "ambient-token-secret-sentinel")
        .env("OPENAI_API_KEY", "ambient-openai-key-secret-sentinel")
        .env("OPENAI_BASE_URL", "https://ambient.invalid/v1")
        .env("CODEX_API_KEY", "ambient-codex-key-secret-sentinel")
        .env("ANTHROPIC_API_KEY", "ambient-anthropic-key-secret-sentinel")
        .env("LITELLM_API_KEY", "ambient-litellm-key-secret-sentinel")
        .args([
            "codex",
            "personal",
            "--",
            "-c",
            "model_providers.aix_chatgpt_plan.base_url=\"https://attacker.invalid/v1\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let output = String::from_utf8(output).unwrap();
    let child_token = output
        .lines()
        .find_map(|line| line.strip_prefix("ACCESS_TOKEN="))
        .expect("Codex provider credential is set");
    assert_eq!(child_token.len(), 64);
    for secret in [
        "access-token-secret-sentinel",
        "refresh-token-secret-sentinel",
        "id-token-secret-sentinel",
        "ambient-token-secret-sentinel",
        "ambient-openai-key-secret-sentinel",
        "ambient-codex-key-secret-sentinel",
        "ambient-anthropic-key-secret-sentinel",
        "ambient-litellm-key-secret-sentinel",
    ] {
        assert!(
            !output.contains(secret),
            "child received {secret}: {output}"
        );
    }

    let args: Vec<_> = output
        .lines()
        .filter_map(|line| line.strip_prefix("ARG="))
        .collect();
    let base_url_override = args
        .windows(2)
        .position(|window| {
            window[0] == "-c"
                && window[1].starts_with("model_providers.aix_chatgpt_plan.base_url=")
                && window[1].contains("http://127.0.0.1:")
        })
        .expect("dynamic loopback provider base URL is passed to Codex");
    let caller_override = args
        .iter()
        .position(|arg| arg.contains("https://attacker.invalid/v1"))
        .expect("caller-supplied provider values are present for precedence testing");
    assert!(base_url_override > caller_override);
    assert!(args
        .iter()
        .any(|arg| { *arg == "model_provider=\"aix_chatgpt_plan\"" }));
    assert!(args
        .iter()
        .any(|arg| { *arg == "model_providers.aix_chatgpt_plan.env_key=\"ACCESS_TOKEN\"" }));
    assert_eq!(
        output
            .lines()
            .filter(|line| line.starts_with("ACCESS_TOKEN="))
            .count(),
        1,
        "only the local bearer may enter the child: {output}"
    );
    for name in [
        "OPENAI_API_KEY=unset",
        "CODEX_API_KEY=unset",
        "ANTHROPIC_API_KEY=unset",
        "LITELLM_API_KEY=unset",
    ] {
        assert!(output.contains(name), "missing {name}: {output}");
    }
    assert_eq!(std::fs::read(&auth_file).unwrap(), auth_before);
    assert_eq!(
        [
            std::fs::read(&codex_config).unwrap(),
            std::fs::read(&codex_auth).unwrap(),
            std::fs::read(&codex_session).unwrap(),
        ],
        codex_files_before,
        "aix must leave Codex config, auth, and session state untouched"
    );
}

#[test]
#[cfg(unix)]
fn codex_local_gateway_runs_are_scoped_and_do_not_require_child_owned_auth() {
    use std::os::unix::fs::PermissionsExt;

    let dir = assert_fs::TempDir::new().unwrap();
    let fake_codex = dir.child("fake-codex");
    fake_codex
        .write_str(
            "#!/bin/sh\nprintf 'AIX_RUN_ID=%s\\n' \"$AIX_RUN_ID\"\nprintf 'ACCESS_TOKEN=%s\\n' \"$ACCESS_TOKEN\"\n",
        )
        .unwrap();
    std::fs::set_permissions(fake_codex.path(), std::fs::Permissions::from_mode(0o700)).unwrap();

    let command = toml::Value::String(fake_codex.path().to_string_lossy().to_string());
    let config_text =
        CODEX_LOCAL_GATEWAY_RECIPE.replace("command = \"codex\"", &format!("command = {command}"));
    let config = write_config(&dir, &config_text);
    let auth_dir = dir.path().join("empty-auth");
    let state_dir = dir.path().join("state");

    let output = cmd()
        .env("AIX_CONFIG", &config)
        .env("AIX_AUTH_DIR", &auth_dir)
        .env("AIX_STATE_DIR", &state_dir)
        .args(["--profile", "personal", "run", "--", "codex", "task"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();
    let run_id = output
        .lines()
        .find_map(|line| line.strip_prefix("AIX_RUN_ID="))
        .expect("Codex child receives the run ID");
    let token = output
        .lines()
        .find_map(|line| line.strip_prefix("ACCESS_TOKEN="))
        .expect("Codex child receives its local credential");
    assert_eq!(token.len(), 64);
    assert!(
        !auth_dir.exists(),
        "launch must not read or create OAuth state"
    );

    let record = cmd()
        .env("AIX_STATE_DIR", &state_dir)
        .args(["runs", "show", run_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let record: Value = serde_json::from_slice(&record).unwrap();
    assert_eq!(record["data"]["run_id"], run_id);
    assert_eq!(record["data"]["profile"], "personal");
    assert_eq!(record["data"]["logical_tool_name"], "codex");
    assert!(!record.to_string().contains(token));
}

#[test]
#[cfg(unix)]
fn codex_app_server_recipe_passes_documented_provider_args_and_only_access_token() {
    use std::os::unix::fs::PermissionsExt;

    let dir = assert_fs::TempDir::new().unwrap();
    let fake_codex = dir.child("fake-codex");
    fake_codex
        .write_str(
            "#!/bin/sh\nprintf 'ACCESS_TOKEN=%s\\n' \"$ACCESS_TOKEN\"\nfor arg in \"$@\"; do printf 'ARG=%s\\n' \"$arg\"; done\nprintf 'OPENAI_API_KEY=%s\\n' \"${OPENAI_API_KEY-unset}\"\nprintf 'ANTHROPIC_API_KEY=%s\\n' \"${ANTHROPIC_API_KEY-unset}\"\nprintf 'LITELLM_API_KEY=%s\\n' \"${LITELLM_API_KEY-unset}\"\nprintf 'CODEX_API_KEY=%s\\n' \"${CODEX_API_KEY-unset}\"\n",
        )
        .unwrap();
    std::fs::set_permissions(fake_codex.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let command = toml::Value::String(fake_codex.path().to_string_lossy().to_string());
    let config_text = CODEX_RECIPE.replace("command = \"codex\"", &format!("command = {command}"));
    let config = write_config(&dir, &config_text);
    let auth_dir = dir.path().join("auth");
    write_credentials(&auth_dir);

    let output = cmd()
        .env("AIX_CONFIG", &config)
        .env("AIX_AUTH_DIR", &auth_dir)
        .env("ANTHROPIC_API_KEY", "ambient-anthropic-key")
        .env("ANTHROPIC_BASE_URL", "https://ambient.invalid")
        .env("OPENAI_API_KEY", "ambient-openai-key")
        .env("OPENAI_BASE_URL", "https://ambient.invalid/v1")
        .env("LITELLM_API_KEY", "ambient-litellm-key")
        .env("LITELLM_BASE_URL", "https://ambient.invalid/v1")
        .env("CODEX_API_KEY", "ambient-codex-key")
        .args(["codex", "personal"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();
    let actual_args: Vec<_> = output
        .lines()
        .filter_map(|line| line.strip_prefix("ARG="))
        .collect();
    assert_eq!(
        actual_args,
        [
            "app-server",
            "--listen",
            "stdio://",
            "-c",
            "model_provider=\"openai_chatgpt_plan\"",
            "-c",
            "model_providers.openai_chatgpt_plan.name=\"ChatGPT plan\"",
            "-c",
            "model_providers.openai_chatgpt_plan.base_url=\"https://api.openai.com/v1\"",
            "-c",
            "model_providers.openai_chatgpt_plan.env_key=\"ACCESS_TOKEN\"",
            "-c",
            "model_providers.openai_chatgpt_plan.wire_api=\"responses\"",
            "-c",
            "model_providers.openai_chatgpt_plan.requires_openai_auth=false",
            "-c",
            "model_providers.openai_chatgpt_plan.supports_websockets=false",
        ]
    );
    assert!(output.contains("ACCESS_TOKEN=access-token-secret-sentinel"));
    assert!(output.contains("OPENAI_API_KEY=unset"));
    assert!(output.contains("ANTHROPIC_API_KEY=unset"));
    assert!(output.contains("LITELLM_API_KEY=unset"));
    assert!(output.contains("CODEX_API_KEY=unset"));
    assert_eq!(
        output
            .lines()
            .filter(|line| line.contains("access-token-secret-sentinel"))
            .collect::<Vec<_>>(),
        ["ACCESS_TOKEN=access-token-secret-sentinel"]
    );
    for secret in [
        "refresh-token-secret-sentinel",
        "id-token-secret-sentinel",
        "client-id-secret-sentinel",
        "other-profile-access-token-sentinel",
    ] {
        assert!(
            !output.contains(secret),
            "child received {secret}: {output}"
        );
    }
}

#[test]
fn chatgpt_profiles_reject_tools_without_an_explicit_binding() {
    let dir = assert_fs::TempDir::new().unwrap();
    let config = write_config(&dir, UNBOUND_CONFIG);
    let auth_dir = dir.path().join("auth");

    cmd()
        .env("AIX_CONFIG", &config)
        .env("AIX_AUTH_DIR", &auth_dir)
        .args(["codex", "personal", "--dry-run"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains(
            "not configured to consume ChatGPT-plan credentials",
        ));
    assert!(!auth_dir.exists());
}

#[test]
fn chatgpt_profiles_do_not_use_the_unknown_tool_api_key_fallback() {
    let dir = assert_fs::TempDir::new().unwrap();
    let config = write_config(&dir, UNBOUND_CONFIG);
    let auth_dir = dir.path().join("auth");

    cmd()
        .env("AIX_CONFIG", &config)
        .env("AIX_AUTH_DIR", &auth_dir)
        .args(["unconfigured-tool", "personal", "--dry-run"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains(
            "not configured to consume ChatGPT-plan credentials",
        ));
    assert!(!auth_dir.exists());
}

#[test]
#[cfg(unix)]
fn plain_managed_run_uses_chatgpt_binding_without_persisting_credentials() {
    let dir = assert_fs::TempDir::new().unwrap();
    let config = write_config(&dir, TOOL_CONFIG);
    let auth_dir = dir.path().join("auth");
    write_credentials(&auth_dir);
    let state_dir = dir.path().join("state");

    let output = cmd()
        .env("AIX_CONFIG", &config)
        .env("AIX_AUTH_DIR", &auth_dir)
        .env("AIX_STATE_DIR", &state_dir)
        .args([
            "--profile",
            "personal",
            "run",
            "--",
            "codex",
            "caller-one",
            "caller-two",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let child = String::from_utf8(output).unwrap();
    assert!(
        child.contains("from-config|caller-one|caller-two"),
        "{child}"
    );
    assert!(child.contains("ACCESS_TOKEN=access-token-secret-sentinel"));

    let run_id = child
        .lines()
        .find_map(|line| line.strip_prefix("AIX_RUN_ID="))
        .expect("child should receive run ID")
        .to_owned();
    let record = cmd()
        .env("AIX_STATE_DIR", &state_dir)
        .args(["runs", "show", &run_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let record: Value = serde_json::from_slice(&record).unwrap();
    let record = record.to_string();
    for secret in [
        "access-token-secret-sentinel",
        "refresh-token-secret-sentinel",
        "id-token-secret-sentinel",
        "client-id-secret-sentinel",
    ] {
        assert!(!record.contains(secret), "run history leaked {secret}");
    }
}

#[test]
fn chatgpt_profiles_reject_leased_runs_before_gateway_or_auth_network_access() {
    let dir = assert_fs::TempDir::new().unwrap();
    let config = write_config(&dir, TOOL_CONFIG);
    let auth_dir = dir.path().join("auth");

    cmd()
        .env("AIX_CONFIG", &config)
        .env("AIX_AUTH_DIR", &auth_dir)
        .args([
            "--profile",
            "personal",
            "run",
            "--lease",
            "--budget",
            "1",
            "--dry-run",
            "--",
            "codex",
        ])
        .assert()
        .code(2)
        .stderr(predicates::str::contains(
            "LiteLLM lease and run-policy flows require an API-key profile",
        ));
    assert!(!auth_dir.exists());
}

#[test]
fn chatgpt_run_policy_with_budget_fails_with_explicit_unavailable_capability() {
    let dir = assert_fs::TempDir::new().unwrap();
    let config_text = format!(
        "{TOOL_CONFIG}\n[run_policies.bounded]\nprofile = 'personal'\nmax_budget = 3.0\nmax_duration = '1h'\n"
    );
    let config = write_config(&dir, &config_text);
    let auth_dir = dir.path().join("auth");

    cmd()
        .env("AIX_CONFIG", &config)
        .env("AIX_AUTH_DIR", &auth_dir)
        .args([
            "--profile",
            "personal",
            "run",
            "--policy",
            "bounded",
            "--",
            "codex",
        ])
        .assert()
        .code(2)
        .stderr(predicates::str::contains(
            "monetary budget enforcement unavailable for this transport",
        ));
    assert!(!auth_dir.exists());
}

#[test]
#[cfg(unix)]
fn chatgpt_opencode_run_policy_without_budget_launches_and_records_no_budget() {
    let dir = assert_fs::TempDir::new().unwrap();
    let state_dir = dir.path().join("state");
    let config_text = format!(
        r#"{TOOL_CONFIG}

[tools.opencode]
command = "sh"
api_format = "openai"
local_gateway = true

[tools.opencode.chatgpt]
access_token_env = "ACCESS_TOKEN"

[run_policies.local]
profile = "personal"
max_duration = "1h"
allowed_models = ["gpt-6-luna"]
tags = ["workflow:implement"]
"#
    );
    let config = write_config(&dir, &config_text);
    let auth_dir = dir.path().join("auth");
    let script = r#"test -n "$OPENCODE_CONFIG_CONTENT" && test "$AIX_RUN_POLICY" = local && test -n "$AIX_RUN_TAGS" && test -z "${OPENAI_API_KEY+x}" && test -z "${ANTHROPIC_API_KEY+x}""#;

    cmd()
        .env("AIX_CONFIG", &config)
        .env("AIX_AUTH_DIR", &auth_dir)
        .env("AIX_STATE_DIR", &state_dir)
        .args([
            "--profile",
            "personal",
            "run",
            "--policy",
            "local",
            "--",
            "opencode",
            "-c",
            script,
        ])
        .assert()
        .success();

    let output = cmd()
        .env("AIX_STATE_DIR", &state_dir)
        .args(["runs", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let runs: Value = serde_json::from_slice(&output).unwrap();
    let run = &runs["data"][0];
    assert_eq!(run["policy"]["name"], "local");
    assert_eq!(run["policy"]["effective_duration"], "1h");
    assert!(run["policy"].get("effective_budget").is_none());
    assert!(run["lease"].is_null());
    assert!(!auth_dir.exists());
}
