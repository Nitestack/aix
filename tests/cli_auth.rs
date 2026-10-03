use assert_cmd::Command;
use assert_fs::prelude::*;
use predicates::prelude::*;
use serde_json::Value;

const CHATGPT_CONFIG: &str = r#"
default_profile = "personal"

[profiles.personal]
label = "Personal ChatGPT"
auth = { type = "chatgpt" }

[profiles.work]
auth = { type = "chatgpt" }

[models]
default = "openai/test-model"

[prompts.demo]
prompt = "Test prompt"
"#;

fn write_config(dir: &assert_fs::TempDir, content: &str) -> std::path::PathBuf {
    dir.child("aix.toml").write_str(content).unwrap();
    dir.child("aix.toml").path().to_path_buf()
}

fn command(config: &std::path::Path, auth_dir: &std::path::Path) -> Command {
    let mut command = Command::cargo_bin("aix").unwrap();
    command
        .env("AIX_CONFIG", config)
        .env("AIX_AUTH_DIR", auth_dir)
        .env_remove("AIX_PROFILE");
    command
}

#[test]
fn chatgpt_auth_status_is_offline_and_has_a_secret_free_json_envelope() {
    let dir = assert_fs::TempDir::new().unwrap();
    let config = write_config(&dir, CHATGPT_CONFIG);
    let auth_dir = dir.path().join("auth");

    let output = command(&config, &auth_dir)
        .args(["auth", "status", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["command"], "auth status");
    assert_eq!(value["data"]["profile"], "personal");
    assert_eq!(value["data"]["auth_type"], "chatgpt");
    assert_eq!(value["data"]["state"], "signed_out");
    assert_eq!(value["data"]["connected"], false);
    assert!(value["data"]["access_token_expires_at"].is_null());
    assert!(value["data"]["email"].is_null());

    command(&config, &auth_dir)
        .args(["auth", "status", "work"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Profile: work"))
        .stdout(predicate::str::contains("Authentication: ChatGPT"))
        .stdout(predicate::str::contains("Status: signed_out"));
}

#[test]
fn chatgpt_auth_status_reports_only_nonsecret_saved_state() {
    let dir = assert_fs::TempDir::new().unwrap();
    let config = write_config(&dir, CHATGPT_CONFIG);
    let auth_dir = dir.child("auth");
    auth_dir.create_dir_all().unwrap();
    auth_dir
        .child("profile-706572736f6e616c.json")
        .write_str(
            r#"{
  "version": 1,
  "profile": "personal",
  "client_id": "issued-client-id",
  "subject": "verified-subject",
  "email": "person@example.test",
  "scopes": ["openid", "chatgpt.tokens.use.direct"],
  "id_token": "id-token-secret-sentinel",
  "access_token": "access-token-secret-sentinel",
  "refresh_token": "refresh-token-secret-sentinel",
  "expires_at": 1800000000,
  "earliest_refresh_at": 1700000000
}"#,
        )
        .unwrap();

    let output = command(&config, auth_dir.path())
        .args(["auth", "status", "personal", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output_text = String::from_utf8(output.clone()).unwrap();
    for token in [
        "id-token-secret-sentinel",
        "access-token-secret-sentinel",
        "refresh-token-secret-sentinel",
    ] {
        assert!(!output_text.contains(token));
    }
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["data"]["state"], "connected");
    assert_eq!(value["data"]["chatgpt_plan_usage_enabled"], true);
    assert_eq!(value["data"]["access_token_expires_at"], 1800000000);
    assert_eq!(value["data"]["email"], "person@example.test");
}

#[test]
fn chatgpt_login_rejects_non_interactive_mode_and_logout_needs_no_network_when_signed_out() {
    let dir = assert_fs::TempDir::new().unwrap();
    let config = write_config(&dir, CHATGPT_CONFIG);
    let auth_dir = dir.path().join("auth");
    command(&config, &auth_dir)
        .args(["--non-interactive", "auth", "login", "personal"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "requires interactive browser authorization",
        ));
    command(&config, &auth_dir)
        .args(["auth", "logout", "personal"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already signed out"));
}

#[test]
fn commands_without_chatgpt_auth_support_fail_with_an_explicit_capability_error() {
    let dir = assert_fs::TempDir::new().unwrap();
    let config = write_config(&dir, CHATGPT_CONFIG);
    let auth_dir = dir.path().join("auth");
    for args in [
        vec!["env", "personal"],
        vec!["shell", "personal", "--dry-run"],
        vec!["exec", "personal", "--dry-run", "--", "echo"],
        vec!["models", "personal"],
        vec!["status", "personal"],
        vec!["spend", "personal"],
        vec!["usage", "personal", "--since", "7d"],
        vec!["doctor", "personal"],
        vec!["--profile", "personal", "ask", "hello"],
        vec!["--profile", "personal", "prompt", "demo"],
        vec!["--profile", "personal", "run", "--", "echo", "hello"],
        vec!["--profile", "personal", "gate", "--policy", "unknown"],
        vec!["--profile", "personal", "claude", "--dry-run"],
    ] {
        command(&config, &auth_dir)
            .args(args.clone())
            .assert()
            .code(2)
            .stderr(predicate::str::contains(
                "requires an API-key profile; ChatGPT authentication is not supported",
            ));
    }
}

#[test]
fn api_key_profile_auth_status_is_local_and_reports_its_configured_auth_type() {
    let dir = assert_fs::TempDir::new().unwrap();
    let config = write_config(
        &dir,
        r#"
default_profile = "work"
[endpoint]
base_url = "https://gateway.example.test"
[profiles.work]
api_key = { env = "AIX_UNSET_API_KEY" }
"#,
    );
    command(&config, &dir.path().join("auth"))
        .args(["auth", "status", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("api_key"))
        .stdout(predicate::str::contains("configured"));
}
