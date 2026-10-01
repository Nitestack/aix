use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn write_config(dir: &TempDir, base_url: &str, api_key: &str) -> std::path::PathBuf {
    let file = dir.child("aix.toml");
    file.write_str(&format!(
        r#"
[endpoint]
base_url = "{base_url}"

[profiles.test]
api_key = "{api_key}"
"#
    ))
    .unwrap();
    file.path().to_path_buf()
}

fn write_secret_config(dir: &TempDir) -> std::path::PathBuf {
    let file = dir.child("aix.toml");
    file.write_str(
        r#"
[endpoint]
base_url = { env = "AIX_MODELS_BASE_URL" }

[profiles.test]
api_key = { env = "AIX_MODELS_API_KEY" }
"#,
    )
    .unwrap();
    file.path().to_path_buf()
}

#[tokio::test]
async fn models_uses_openai_path_bearer_auth_and_sorts_ids() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("Authorization", "Bearer test-model-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "object": "list",
            "data": [
                { "id": "z-model" },
                { "id": "a-model" }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), "test-model-key");
    let output = Command::cargo_bin("aix")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "models", "test"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "a-model\nz-model\n"
    );
}

#[tokio::test]
async fn models_filter_is_case_insensitive() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "id": "OpenAI/GPT-4o" },
                { "id": "claude-3-5-sonnet" },
                { "id": "gpt-4o-mini" }
            ]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), "test-model-key");
    Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "models",
            "test",
            "--filter",
            "AI/GPT-4O",
        ])
        .assert()
        .success()
        .stdout("OpenAI/GPT-4o\n");
}

#[tokio::test]
async fn models_filter_with_no_matches_succeeds_with_empty_output() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": "example-model" }]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), "test-model-key");
    Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "models",
            "test",
            "--filter",
            "not-present",
        ])
        .assert()
        .success()
        .stdout("");
}

#[tokio::test]
async fn models_json_uses_stable_envelope_and_only_exposes_ids() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "id": "z-model", "created": 123, "owned_by": "vendor" },
                { "id": "a-model", "created": 456, "owned_by": "vendor" }
            ]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), "test-model-key");
    let output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--json",
            "--config",
            config.to_str().unwrap(),
            "models",
            "test",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(parsed["command"], "models");
    assert_eq!(parsed["data"]["filter"], Value::Null);
    assert_eq!(
        parsed["data"]["models"],
        json!([{ "id": "a-model" }, { "id": "z-model" }])
    );
}

#[tokio::test]
async fn models_json_includes_and_applies_the_filter() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": "a-model" }, { "id": "b-model" }]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), "test-model-key");
    let output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "models",
            "test",
            "--filter",
            "A-MODEL",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["data"]["filter"], "A-MODEL");
    assert_eq!(parsed["data"]["models"], json!([{ "id": "a-model" }]));
}

#[tokio::test]
async fn models_auth_failure_uses_auth_exit_code_and_redacts_secrets() {
    let server = MockServer::start().await;
    let api_key = "sk-model-auth-secret";
    let base_url = format!("{}/base-url-secret", server.uri());
    Mock::given(method("GET"))
        .and(path("/base-url-secret/v1/models"))
        .and(header("Authorization", format!("Bearer {api_key}")))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "message": format!("rejected {api_key} at {base_url}")
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_secret_config(&dir);
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_MODELS_BASE_URL", &base_url)
        .env("AIX_MODELS_API_KEY", api_key)
        .args(["--config", config.to_str().unwrap(), "models", "test"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains(api_key), "API key leaked: {stderr}");
    assert!(!stderr.contains(&base_url), "base URL leaked: {stderr}");
    assert!(stderr.contains("403"));
}

#[tokio::test]
async fn models_malformed_json_and_response_shape_are_protocol_errors() {
    for response in [
        ResponseTemplate::new(200).set_body_string("not-json"),
        ResponseTemplate::new(200).set_body_json(json!({ "object": "list" })),
        ResponseTemplate::new(200).set_body_json(json!({ "data": [{ "id": 42 }] })),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(response)
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let config = write_config(&dir, &server.uri(), "sk-malformed-secret");
        let output = Command::cargo_bin("aix")
            .unwrap()
            .args(["--config", config.to_str().unwrap(), "models", "test"])
            .output()
            .unwrap();

        assert_eq!(output.status.code(), Some(5));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.to_lowercase().contains("malformed"), "{stderr}");
        assert!(
            !stderr.contains("sk-malformed-secret"),
            "secret leaked: {stderr}"
        );
    }
}

#[tokio::test]
async fn models_transport_failure_does_not_leak_secret_key_or_base_url() {
    let dir = TempDir::new().unwrap();
    let config = write_secret_config(&dir);
    let api_key = "sk-model-transport-secret";
    let base_url = "http://127.0.0.1:1/base-url-transport-secret";
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_MODELS_BASE_URL", base_url)
        .env("AIX_MODELS_API_KEY", api_key)
        .args(["--config", config.to_str().unwrap(), "models", "test"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains(api_key), "API key leaked: {stderr}");
    assert!(!stderr.contains(base_url), "base URL leaked: {stderr}");
}

#[tokio::test]
async fn models_fetches_fresh_data_on_each_invocation() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": "live-model" }]
        })))
        .expect(2)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), "test-model-key");
    for _ in 0..2 {
        Command::cargo_bin("aix")
            .unwrap()
            .args(["--config", config.to_str().unwrap(), "models", "test"])
            .assert()
            .success()
            .stdout("live-model\n");
    }
}
