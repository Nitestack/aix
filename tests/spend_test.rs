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
        .and(header("Authorization", "Bearer sk-test1111"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "keys": [
                {
                    "key_name": "sk-...1111",
                    "spend": 41.53,
                    "max_budget": 500.0,
                    "metadata": { "key_name": "alice" }
                },
                {
                    "key_name": "sk-...2222",
                    "spend": 100.0,
                    "max_budget": 1000.0,
                    "metadata": { "key_name": "bob" }
                }
            ]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), "sk-test1111");

    Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args(["--config", config.to_str().unwrap(), "spend", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("41.53"))
        .stdout(predicate::str::contains("500.00"))
        .stdout(predicate::str::contains("458.47"))
        .stdout(predicate::str::contains("available"))
        .stdout(predicate::str::contains("█"))
        .stdout(predicate::str::contains("% used"))
        .stdout(predicate::str::contains("bob").not());
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
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), "sk-nomatch1234");

    Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args(["--config", config.to_str().unwrap(), "spend", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("99.00"))
        .stdout(predicate::str::contains("200.00"))
        .stdout(predicate::str::contains("█"))
        .stdout(predicate::str::contains("% used"));
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
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), "sk-test1234");

    let output = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
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

#[tokio::test]
async fn spend_second_call_uses_cache() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/user/info"))
        .and(header("Authorization", "Bearer sk-testT3S4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "user_id": "u-cached",
            "spend": 4.20,
            "max_budget": 10.0,
            "keys": [{ "key_name": "sk-...T3S4", "spend": 4.20, "max_budget": 10.0 }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), "sk-testT3S4");

    Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args(["--config", config.to_str().unwrap(), "spend", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("4.20"));

    Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args(["--config", config.to_str().unwrap(), "spend", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("4.20"));
    // wiremock verifies .expect(1) on server drop
}

#[tokio::test]
async fn spend_no_cache_flag_always_fetches() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/user/info"))
        .and(header("Authorization", "Bearer sk-testN0C1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "user_id": "u-nocache",
            "spend": 1.0,
            "max_budget": 5.0,
            "keys": [{ "key_name": "sk-...N0C1", "spend": 1.0, "max_budget": 5.0 }]
        })))
        .expect(2)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), "sk-testN0C1");

    for _ in 0..2 {
        Command::cargo_bin("aix")
            .unwrap()
            .env("AIX_CACHE_DIR", cache_dir.path())
            .args([
                "--config",
                config.to_str().unwrap(),
                "spend",
                "test",
                "--no-cache",
            ])
            .assert()
            .success();
    }
    // wiremock verifies .expect(2) on server drop
}
