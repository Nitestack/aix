use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use predicates::prelude::*;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
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
        .and(path("/key/info"))
        .and(header("Authorization", "Bearer sk-test1111"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "key": "server-key",
            "info": {
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
            }
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
        .and(path("/key/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "key": "server-key",
            "info": {
                "spend": 99.0,
                "max_budget": 200.0,
                "keys": []
            }
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
async fn spend_json_returns_key_info_without_secret() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "key": "server-key",
            "info": {
                "spend": 1.23,
                "token": "server-key",
                "key_name": "sk-...1234"
            }
        })))
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
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["spend"], 1.23);
    assert!(!stdout.contains("server-key"));
}

#[tokio::test]
async fn spend_second_call_uses_cache() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/key/info"))
        .and(header("Authorization", "Bearer sk-testT3S4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "key": "server-key",
            "info": {
                "user_id": "u-cached",
                "spend": 4.20,
                "max_budget": 10.0,
                "keys": [{ "key_name": "sk-...T3S4", "spend": 4.20, "max_budget": 10.0 }]
            }
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
async fn spend_warms_cache_for_sibling_keys() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .and(header("Authorization", "Bearer sk-firstK111"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "key": "server-key",
            "info": {
                "user_id": "u-shared",
                "spend": 10.0,
                "max_budget": 100.0
            }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/list"))
        .and(header("Authorization", "Bearer sk-firstK111"))
        .and(query_param("user_id", "u-shared"))
        .and(query_param("page", "1"))
        .and(query_param("size", "100"))
        .and(query_param("return_full_object", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "keys": [
                {
                    "api_key": "server-key-K111",
                    "spend": 10.0,
                    "max_budget": 100.0
                },
                {
                    "api_key": "server-key-K222",
                    "spend": 20.0,
                    "max_budget": 100.0
                }
            ],
            "total_pages": 1
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = dir.child("aix.toml");
    config
        .write_str(&format!(
            r#"
[endpoint]
base_url = "{}"

[profiles.first]
api_key = "sk-firstK111"

[profiles.second]
api_key = "sk-secondK222"
"#,
            server.uri()
        ))
        .unwrap();

    Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args([
            "--config",
            config.path().to_str().unwrap(),
            "spend",
            "first",
            "--no-cache",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("10.00"));

    Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args([
            "--config",
            config.path().to_str().unwrap(),
            "spend",
            "second",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("20.00"))
        .stdout(predicate::str::contains("80.00"));

    for endpoint in std::fs::read_dir(cache_dir.path()).unwrap() {
        let endpoint = endpoint.unwrap().path();
        if endpoint.is_dir() {
            for entry in std::fs::read_dir(endpoint).unwrap() {
                let contents = std::fs::read_to_string(entry.unwrap().path()).unwrap();
                assert!(!contents.contains("server-key"));
                assert!(!contents.contains("api_key"));
                assert!(!contents.contains("token"));
            }
        }
    }
}

#[tokio::test]
async fn spend_no_cache_flag_always_fetches() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/key/info"))
        .and(header("Authorization", "Bearer sk-testN0C1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "key": "server-key",
            "info": {
                "user_id": "u-nocache",
                "spend": 1.0,
                "max_budget": 5.0,
                "keys": [{ "key_name": "sk-...N0C1", "spend": 1.0, "max_budget": 5.0 }]
            }
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

#[tokio::test]
async fn spend_output_has_no_ansi_codes_when_piped() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "key": "server-key",
            "info": {
                "spend": 90.0,
                "max_budget": 100.0,
                "keys": []
            }
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), "sk-colortest1");

    let output = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args(["--config", config.to_str().unwrap(), "spend", "test"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    // assert_cmd captures stdout to a pipe, not a TTY, so owo-colors'
    // supports-colors detection must suppress ANSI escapes here.
    assert!(
        !stdout.contains('\x1b'),
        "expected no ANSI codes, got: {stdout:?}"
    );
    assert!(stdout.contains("90% used"));
}
