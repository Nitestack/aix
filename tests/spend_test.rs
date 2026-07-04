use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use predicates::prelude::*;
use serde_json::json;
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

#[tokio::test]
async fn spend_shows_matching_key_by_suffix() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/info"))
        .and(header("Authorization", "Bearer sk-testABCD"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "keys": [
                {
                    "key_name": "sk-...ABCD",
                    "spend": 41.53,
                    "max_budget": 500.0,
                    "metadata": { "key_name": "Nhan Pham" }
                },
                {
                    "key_name": "sk-...ZZZZ",
                    "spend": 100.0,
                    "max_budget": 1000.0,
                    "metadata": { "key_name": "Someone Else" }
                }
            ]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), "sk-testABCD");

    Command::cargo_bin("aix")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "spend", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Nhan Pham"))
        .stdout(predicate::str::contains("41.53"))
        .stdout(predicate::str::contains("500.00"))
        .stdout(predicate::str::contains("458.47"))
        .stdout(predicate::str::contains("Someone Else").not());
}

#[tokio::test]
async fn spend_falls_back_to_user_totals_when_no_key_match() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "spend": 99.0,
            "max_budget": 200.0,
            "keys": []
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), "sk-nomatch1234");

    Command::cargo_bin("aix")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "spend", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("99.00"))
        .stdout(predicate::str::contains("200.00"));
}

#[tokio::test]
async fn spend_json_returns_full_user_info() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"spend": 1.23})))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), "sk-test1234");

    let output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "spend",
            "test",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["spend"], 1.23);
}
