use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use predicates::prelude::*;
use serde_json::json;

use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn write_config(dir: &TempDir, base_url: &str) -> std::path::PathBuf {
    let file = dir.child("aix.toml");
    file.write_str(&format!(
        r#"
[endpoint]
base_url = "{base_url}"
gateway = "litellm"

[profiles.test]
api_key = "sk-test-key"
"#
    ))
    .unwrap();
    file.path().to_path_buf()
}

#[tokio::test]
async fn info_shows_user_id_and_spend() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/info"))
        .and(header("Authorization", "Bearer sk-test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "user_id": "user-abc",
            "spend": 2.50,
            "max_budget": 10.0,
            "keys": [{"key": "sk-test-key"}]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    Command::cargo_bin("aix")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "info", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("user-abc"))
        .stdout(predicate::str::contains("2.50"));
}

#[tokio::test]
async fn info_json_flag_returns_raw_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"user_id": "u1"})))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    let output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "info",
            "test",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["user_id"], "u1");
}
