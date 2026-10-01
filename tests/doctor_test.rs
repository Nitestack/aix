use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Output;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const API_KEY: &str = "sk-doctor-secret-key";

fn write_config(dir: &TempDir, contents: &str) -> PathBuf {
    let file = dir.child("aix.toml");
    file.write_str(contents).unwrap();
    file.path().to_path_buf()
}

fn config_for(base_url: &str, gateway: Option<&str>, api_key: &str) -> String {
    let gateway = gateway.map_or(String::new(), |gateway| {
        format!("gateway = \"{gateway}\"\n")
    });
    format!(
        r#"
default_profile = "work"

[endpoint]
base_url = "{base_url}"
{gateway}
[profiles.work]
api_key = "{api_key}"
"#
    )
}

fn invoke(config: &Path, args: &[&str], cache_dir: &Path) -> Output {
    invoke_with_env(config, args, cache_dir, &[])
}

fn invoke_with_env(
    config: &Path,
    args: &[&str],
    cache_dir: &Path,
    environment: &[(&str, &str)],
) -> Output {
    let mut command = Command::cargo_bin("aix").unwrap();
    command
        .env("AIX_CACHE_DIR", cache_dir)
        .env_remove("AIX_PROFILE")
        .env_remove("AIX_CONFIG");
    for (name, value) in environment {
        command.env(name, value);
    }
    command
        .args(["--config", config.to_str().unwrap()])
        .args(args)
        .output()
        .unwrap()
}

fn check<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["data"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == name)
        .unwrap_or_else(|| panic!("missing {name} check in {report}"))
}

fn json_report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

async fn mount_healthy_gateway(server: &MockServer, include_admin: bool) {
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "object": "list",
            "data": [{ "id": "example-model" }]
        })))
        .expect(1)
        .mount(server)
        .await;

    if include_admin {
        Mock::given(method("GET"))
            .and(path("/key/info"))
            .and(header("Authorization", format!("Bearer {API_KEY}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "info": { "spend": 1.23, "max_budget": 20.0 }
            })))
            .expect(1)
            .mount(server)
            .await;
    }
}

#[tokio::test]
async fn doctor_reports_all_checks_pass_for_a_healthy_litellm_config() {
    let server = MockServer::start().await;
    mount_healthy_gateway(&server, true).await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &config_for(&server.uri(), Some("litellm"), API_KEY));

    let output = invoke(&config, &["doctor", "--json"], cache_dir.path());

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = json_report(&output);
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["command"], "doctor");
    assert_eq!(report["data"]["checks"].as_array().unwrap().len(), 9);
    for name in [
        "config_discovery",
        "config_validation",
        "profile_selection",
        "api_key",
        "base_url",
        "gateway_auth",
        "model_discovery",
        "litellm_admin",
        "cache_directory",
    ] {
        assert_eq!(check(&report, name)["status"], "pass", "{name}: {report}");
    }
    assert!(check(&report, "gateway_auth")["duration_ms"].is_u64());
    assert!(check(&report, "model_discovery")["duration_ms"].is_u64());
    assert!(check(&report, "config_validation")["duration_ms"].is_null());
    assert!(output.stderr.is_empty());
}

#[test]
fn doctor_reports_missing_config_and_skips_config_dependent_checks() {
    let home = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("empty-config"))
        .env("APPDATA", home.path().join("empty-appdata"))
        .env("AIX_CACHE_DIR", cache_dir.path())
        .env_remove("AIX_CONFIG")
        .env_remove("AIX_PROFILE")
        .args(["--json", "doctor"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let report = json_report(&output);
    assert_eq!(check(&report, "config_discovery")["status"], "fail");
    assert_eq!(check(&report, "config_validation")["status"], "skipped");
    assert_eq!(check(&report, "profile_selection")["status"], "skipped");
    assert_eq!(check(&report, "cache_directory")["status"], "pass");
}

#[test]
fn doctor_reports_invalid_config_without_echoing_config_contents() {
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        "[endpoint]\nbase_url = \"http://invalid-config-secret\"\n[profiles.work\napi_key = \"never-print-this-secret\"\n",
    );

    let output = invoke(&config, &["doctor", "--json"], cache_dir.path());

    assert_eq!(output.status.code(), Some(2));
    let report = json_report(&output);
    assert_eq!(check(&report, "config_validation")["status"], "fail");
    assert_eq!(check(&report, "profile_selection")["status"], "skipped");
    let streams = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!streams.contains("never-print-this-secret"));
    assert!(!streams.contains("invalid-config-secret"));
}

#[test]
fn doctor_skips_dependent_checks_when_a_configured_env_file_cannot_load() {
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let missing_env_file = toml::Value::String(
        dir.path()
            .join("missing-secrets.env")
            .to_string_lossy()
            .into_owned(),
    );
    let config = write_config(
        &dir,
        &format!(
            r#"
env_files = [{missing_env_file}]
default_profile = "work"
[endpoint]
base_url = "http://127.0.0.1:1"
[profiles.work]
api_key = "doctor-env-file-secret"
"#
        ),
    );

    let output = invoke(&config, &["doctor", "--json"], cache_dir.path());

    assert_eq!(output.status.code(), Some(2));
    let report = json_report(&output);
    assert_eq!(check(&report, "config_validation")["status"], "fail");
    assert_eq!(check(&report, "profile_selection")["status"], "pass");
    assert_eq!(check(&report, "api_key")["status"], "skipped");
    assert_eq!(check(&report, "base_url")["status"], "skipped");
    assert_eq!(check(&report, "gateway_auth")["status"], "skipped");
    assert_eq!(check(&report, "model_discovery")["status"], "skipped");
    assert_eq!(check(&report, "litellm_admin")["status"], "skipped");
    assert_eq!(check(&report, "cache_directory")["status"], "pass");
}

#[test]
fn doctor_reports_unknown_profile_and_skips_secret_and_gateway_checks() {
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        &config_for("http://127.0.0.1:1", Some("openai-compatible"), API_KEY),
    );

    let output = invoke(&config, &["doctor", "unknown", "--json"], cache_dir.path());

    assert_eq!(output.status.code(), Some(2));
    let report = json_report(&output);
    assert_eq!(check(&report, "profile_selection")["status"], "fail");
    assert_eq!(check(&report, "api_key")["status"], "skipped");
    assert_eq!(check(&report, "base_url")["status"], "skipped");
    assert_eq!(check(&report, "gateway_auth")["status"], "skipped");
    assert_eq!(check(&report, "model_discovery")["status"], "skipped");
    assert_eq!(check(&report, "litellm_admin")["status"], "not_applicable");
}

#[test]
fn doctor_without_a_resolvable_profile_fails_without_waiting_for_a_picker() {
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        r#"
[endpoint]
base_url = "http://127.0.0.1:1"

[profiles.work]
api_key = "doctor-profile-secret"
"#,
    );

    let output = invoke(&config, &["doctor", "--json"], cache_dir.path());

    assert_eq!(output.status.code(), Some(2));
    let report = json_report(&output);
    assert_eq!(check(&report, "profile_selection")["status"], "fail");
    assert!(check(&report, "profile_selection")["message"]
        .as_str()
        .unwrap()
        .contains("without an interactive picker"));
    assert_eq!(check(&report, "api_key")["status"], "skipped");
}

#[test]
fn doctor_does_not_print_secret_command_stdout_when_secret_resolution_fails() {
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let command = if cfg!(windows) {
        "echo doctor-secret-command-output & exit /b 7"
    } else {
        "printf 'doctor-secret-command-output'; exit 7"
    };
    let config = write_config(
        &dir,
        &format!(
            r#"
default_profile = "work"
[endpoint]
base_url = "http://127.0.0.1:1"
[profiles.work]
api_key = {{ command = "{command}" }}
"#
        ),
    );

    let output = invoke(&config, &["doctor", "--json"], cache_dir.path());

    assert_eq!(output.status.code(), Some(3));
    let report = json_report(&output);
    assert_eq!(check(&report, "api_key")["status"], "fail");
    assert_eq!(check(&report, "base_url")["status"], "pass");
    assert_eq!(check(&report, "gateway_auth")["status"], "skipped");
    assert_eq!(check(&report, "model_discovery")["status"], "skipped");
    assert_eq!(check(&report, "litellm_admin")["status"], "skipped");
    let streams = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!streams.contains("doctor-secret-command-output"));
}

#[tokio::test]
async fn doctor_reports_auth_rejection_without_showing_upstream_body() {
    let server = MockServer::start().await;
    let base_url = format!("{}/doctor-url-secret", server.uri());
    Mock::given(method("GET"))
        .and(path("/doctor-url-secret/v1/models"))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "message": format!("rejected {API_KEY} at {base_url} upstream-body-secret"),
            "access_token": "upstream-token-secret"
        })))
        .expect(2)
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    let api_key_file = dir.child("api-key");
    api_key_file.write_str(API_KEY).unwrap();
    let api_key_path = toml::Value::String(api_key_file.path().to_string_lossy().into_owned());
    let config = write_config(
        &dir,
        &format!(
            r#"
default_profile = "work"
[endpoint]
base_url = {{ env = "AIX_DOCTOR_BASE_URL" }}
gateway = "openai-compatible"
[profiles.work]
api_key = {{ file = {api_key_path} }}
"#
        ),
    );

    for (args, json_mode) in [(&["doctor"][..], false), (&["--json", "doctor"][..], true)] {
        let cache_dir = TempDir::new().unwrap();
        let output = invoke_with_env(
            &config,
            args,
            cache_dir.path(),
            &[("AIX_DOCTOR_BASE_URL", &base_url)],
        );
        assert_eq!(output.status.code(), Some(4));
        if json_mode {
            let report = json_report(&output);
            assert_eq!(check(&report, "gateway_auth")["status"], "fail");
        } else {
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("FAIL gateway_auth"), "{stdout}");
        }
        let streams = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        for secret in [
            API_KEY,
            base_url.as_str(),
            "upstream-body-secret",
            "upstream-token-secret",
        ] {
            assert!(!streams.contains(secret), "leaked {secret}: {streams}");
        }
    }
}

#[test]
fn doctor_reports_unreachable_gateway_and_skips_model_discovery() {
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let base_url = "http://127.0.0.1:1/doctor-network-secret";
    let config = write_config(
        &dir,
        &config_for(base_url, Some("openai-compatible"), API_KEY),
    );

    let output = invoke(&config, &["doctor", "--json"], cache_dir.path());

    assert_eq!(output.status.code(), Some(5));
    let report = json_report(&output);
    assert_eq!(check(&report, "gateway_auth")["status"], "fail");
    assert_eq!(check(&report, "model_discovery")["status"], "skipped");
    assert_eq!(check(&report, "litellm_admin")["status"], "not_applicable");
    let streams = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!streams.contains("doctor-network-secret"));
}

#[tokio::test]
async fn doctor_separates_gateway_success_from_malformed_models_payload() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": 42 }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "info": { "spend": 1.23 }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &config_for(&server.uri(), Some("litellm"), API_KEY));

    let output = invoke(&config, &["doctor", "--json"], cache_dir.path());

    assert_eq!(output.status.code(), Some(5));
    let report = json_report(&output);
    assert_eq!(check(&report, "gateway_auth")["status"], "pass");
    assert_eq!(check(&report, "model_discovery")["status"], "fail");
    assert_eq!(check(&report, "litellm_admin")["status"], "pass");
}

#[tokio::test]
async fn doctor_treats_invalid_models_json_as_a_discovery_failure_not_auth_failure() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
        .respond_with(ResponseTemplate::new(200).set_body_string("malformed-model-response"))
        .expect(1)
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        &config_for(&server.uri(), Some("openai-compatible"), API_KEY),
    );

    let output = invoke(&config, &["doctor", "--json"], cache_dir.path());

    assert_eq!(output.status.code(), Some(5));
    let report = json_report(&output);
    assert_eq!(check(&report, "gateway_auth")["status"], "pass");
    assert_eq!(check(&report, "model_discovery")["status"], "fail");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("malformed-model-response"));
}

#[tokio::test]
async fn doctor_marks_admin_check_not_applicable_for_explicit_non_litellm_gateway() {
    let server = MockServer::start().await;
    mount_healthy_gateway(&server, false).await;
    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        &config_for(&server.uri(), Some("openai-compatible"), API_KEY),
    );

    let output = invoke(&config, &["doctor", "--json"], cache_dir.path());

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = json_report(&output);
    assert_eq!(check(&report, "litellm_admin")["status"], "not_applicable");
    assert!(check(&report, "litellm_admin")["duration_ms"].is_null());
}

#[tokio::test]
async fn doctor_reports_unwritable_cache_path_without_deleting_existing_data() {
    let server = MockServer::start().await;
    mount_healthy_gateway(&server, false).await;
    let dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        &config_for(&server.uri(), Some("openai-compatible"), API_KEY),
    );
    let cache_file = dir.child("cache-is-a-file");
    cache_file
        .write_str("preserve-existing-cache-data")
        .unwrap();

    let output = invoke(&config, &["doctor", "--json"], cache_file.path());

    assert_eq!(output.status.code(), Some(1));
    let report = json_report(&output);
    assert_eq!(check(&report, "gateway_auth")["status"], "pass");
    assert_eq!(check(&report, "cache_directory")["status"], "fail");
    assert_eq!(
        std::fs::read_to_string(cache_file.path()).unwrap(),
        "preserve-existing-cache-data"
    );
}
