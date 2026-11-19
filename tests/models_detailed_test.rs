//! `aix models --detailed`: LiteLLM model metadata, pagination, and caching.

use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const API_KEY: &str = "sk-models-detailed-secret-4242";

fn write_config(dir: &TempDir, base_url: &str) -> std::path::PathBuf {
    let file = dir.child("aix.toml");
    file.write_str(&format!(
        r#"
[endpoint]
base_url = "{base_url}"

[profiles.test]
api_key = "{API_KEY}"
"#
    ))
    .unwrap();
    file.path().to_path_buf()
}

async fn mount_ids(server: &MockServer, ids: &[&str], expected: u64) {
    let data: Vec<Value> = ids.iter().map(|id| json!({ "id": id })).collect();
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(query_param("return_wildcard_routes", "true"))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": data })))
        .expect(expected)
        .mount(server)
        .await;
}

async fn mount_info_page(server: &MockServer, page: u64, body: Value, expected: u64) {
    Mock::given(method("GET"))
        .and(path("/v2/model/info"))
        .and(query_param("page", page.to_string()))
        .and(query_param("size", "100"))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .expect(expected)
        .mount(server)
        .await;
}

async fn mount_status(server: &MockServer, route: &str, status: u16, expected: u64) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(status).set_body_json(json!({})))
        .expect(expected)
        .mount(server)
        .await;
}

fn deployment(name: &str, info: Value) -> Value {
    json!({
        "model_name": name,
        "litellm_params": { "model": name, "api_key": "upstream-secret", "api_base": "https://private.example" },
        "model_info": info
    })
}

fn run(config: &std::path::Path, cache: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache)
        .args(["--config", config.to_str().unwrap()])
        .args(args)
        .output()
        .unwrap()
}

fn run_json(config: &std::path::Path, cache: &std::path::Path, args: &[&str]) -> Value {
    let output = run(config, cache, args);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for secret in [API_KEY, "upstream-secret", "private.example"] {
        assert!(!stdout.contains(secret), "leaked {secret}: {stdout}");
    }
    serde_json::from_str(&stdout).unwrap()
}

#[tokio::test]
async fn detailed_models_merge_pages_and_normalize_metadata() {
    let server = MockServer::start().await;
    mount_ids(&server, &["claude-opus-5-5*", "gpt-6-sol", "openai/*"], 1).await;
    mount_info_page(
        &server,
        1,
        json!({
            "data": [deployment("claude-opus-5-5*", json!({
                "mode": "chat",
                "litellm_provider": "Google",
                "max_input_tokens": 1000000,
                "max_output_tokens": 128000,
                "input_cost_per_token": 0.000004,
                "output_cost_per_token": 0.00002,
                "cache_read_input_token_cost": 2e-7,
                "cache_creation_input_token_cost": 0.000005,
                "supports_reasoning": true,
                "reasoning_effort_levels": ["low", "medium", "high", "xhigh", "max"]
            }))],
            "total_pages": 2,
            "current_page": 1
        }),
        1,
    )
    .await;
    mount_info_page(
        &server,
        2,
        json!({
            "data": [deployment("gpt-6-sol", json!({
                "mode": "chat",
                "litellm_provider": "Azure",
                "max_input_tokens": 922000,
                "supports_vision": true,
                "reasoning_effort_levels": ["none", "low", "high"],
                "default_reasoning_effort": "low"
            }))],
            "total_pages": 2,
            "current_page": 2
        }),
        1,
    )
    .await;
    mount_status(&server, "/model_group/info", 404, 1).await;
    let dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    let report = run_json(
        &config,
        cache.path(),
        &["models", "test", "--detailed", "--json"],
    );
    let data = &report["data"];

    assert_eq!(report["command"], "models");
    assert_eq!(data["cache"]["source"], "live");
    assert_eq!(
        data["sources"],
        json!({ "model_info": true, "model_group_info": false })
    );
    assert_eq!(data["wildcard_routes"], json!(["openai/*"]));
    assert_eq!(data["filter"], Value::Null);
    assert_eq!(
        data["models"][0],
        json!({
            "id": "claude-opus-5-5",
            "route": "claude-opus-5-5*",
            "mode": "chat",
            "provider": "google",
            "display_name": null,
            "deployments": 1,
            "context_tokens": 1000000,
            "max_output_tokens": 128000,
            "capabilities": {},
            "reasoning": {
                "efforts": ["low", "medium", "high", "xhigh", "max"],
                "default_effort": null
            },
            "pricing": {
                "input_per_million": 4.0,
                "output_per_million": 20.0,
                "cache_read_per_million": 0.2,
                "cache_write_per_million": 5.0
            }
        })
    );
    let gpt = &data["models"][1];
    assert_eq!(gpt["id"], "gpt-6-sol");
    assert_eq!(gpt["route"], Value::Null);
    assert_eq!(gpt["capabilities"], json!({ "vision": true }));
    assert_eq!(gpt["reasoning"]["efforts"], json!(["none", "low", "high"]));
    assert_eq!(gpt["reasoning"]["default_effort"], "low");
    assert_eq!(gpt["pricing"], Value::Null);
}

#[tokio::test]
async fn detailed_models_are_cached_and_honour_max_age_and_refresh() {
    let server = MockServer::start().await;
    mount_ids(&server, &["m-1"], 2).await;
    mount_info_page(
        &server,
        1,
        json!({ "data": [deployment("m-1", json!({ "mode": "chat" }))], "total_pages": 1 }),
        2,
    )
    .await;
    mount_status(&server, "/model_group/info", 404, 2).await;
    let dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    let first = run_json(
        &config,
        cache.path(),
        &["models", "--detailed", "--json", "test"],
    );
    assert_eq!(first["data"]["cache"]["source"], "live");
    let cached = run_json(
        &config,
        cache.path(),
        &["models", "--detailed", "--json", "--max-age", "1h", "test"],
    );
    assert_eq!(cached["data"]["cache"]["source"], "cache");
    assert_eq!(cached["data"]["models"], first["data"]["models"]);
    let refreshed = run_json(
        &config,
        cache.path(),
        &["models", "--detailed", "--json", "--refresh", "test"],
    );
    assert_eq!(refreshed["data"]["cache"]["source"], "live");

    // The budget and model caches are separate records.
    let files: Vec<_> = walk(cache.path());
    assert!(
        files.iter().any(|name| name.ends_with(".models.json")),
        "{files:?}"
    );
    for file in &files {
        let contents = std::fs::read_to_string(file).unwrap();
        assert!(!contents.contains("upstream-secret"), "{file}: {contents}");
        assert!(!contents.contains(API_KEY), "{file}: {contents}");
    }
}

fn walk(dir: &std::path::Path) -> Vec<String> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path.to_string_lossy().into_owned());
        }
    }
    files
}

#[tokio::test]
async fn group_info_supplies_reasoning_efforts() {
    let server = MockServer::start().await;
    mount_ids(&server, &["m-1"], 1).await;
    mount_info_page(
        &server,
        1,
        json!({ "data": [deployment("m-1", json!({ "mode": "chat", "supports_reasoning": true }))] }),
        1,
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/model_group/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "model_group": "m-1", "supported_reasoning_efforts": ["high", "minimal"] }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    let report = run_json(
        &config,
        cache.path(),
        &["models", "test", "--detailed", "--json"],
    );
    assert_eq!(report["data"]["sources"]["model_group_info"], true);
    assert_eq!(
        report["data"]["models"][0]["reasoning"]["efforts"],
        json!(["minimal", "high"])
    );
}

#[tokio::test]
async fn gateways_without_model_info_still_list_ids() {
    let server = MockServer::start().await;
    mount_ids(&server, &["b-model", "a-model"], 1).await;
    mount_status(&server, "/v2/model/info", 404, 1).await;
    mount_status(&server, "/model_group/info", 200, 0).await;
    let dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    let report = run_json(
        &config,
        cache.path(),
        &["models", "test", "--detailed", "--json"],
    );
    let data = &report["data"];
    assert_eq!(data["sources"]["model_info"], false);
    assert_eq!(data["models"][0]["id"], "a-model");
    assert_eq!(data["models"][0]["deployments"], 0);
    assert_eq!(data["models"][1]["id"], "b-model");
}

#[tokio::test]
async fn inconsistent_pagination_is_a_protocol_error() {
    let server = MockServer::start().await;
    mount_ids(&server, &["m-1"], 1).await;
    mount_info_page(
        &server,
        1,
        json!({ "data": [deployment("m-1", json!({}))], "total_pages": 2 }),
        1,
    )
    .await;
    mount_info_page(&server, 2, json!({ "data": [], "total_pages": 3 }), 1).await;
    let dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    let output = run(
        &config,
        cache.path(),
        &[
            "--error-format",
            "json",
            "models",
            "test",
            "--detailed",
            "--json",
        ],
    );
    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "gateway_protocol_error");
}

#[tokio::test]
async fn human_detailed_output_summarizes_each_model() {
    let server = MockServer::start().await;
    mount_ids(&server, &["m-1"], 1).await;
    mount_info_page(
        &server,
        1,
        json!({ "data": [deployment("m-1", json!({
            "mode": "chat",
            "max_input_tokens": 200000,
            "max_output_tokens": 64000,
            "input_cost_per_token": 0.000003,
            "output_cost_per_token": 0.000015,
            "reasoning_effort_levels": ["low", "high"]
        }))] }),
        1,
    )
    .await;
    mount_status(&server, "/model_group/info", 404, 1).await;
    let dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    let output = run(&config, cache.path(), &["models", "test", "--detailed"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "m-1  chat · 200K ctx · 64K out · reasoning low..high · $3.00/$15.00 per 1M\n"
    );
}

#[test]
fn refresh_and_max_age_require_detailed() {
    for flag in [&["--refresh"][..], &["--max-age", "5m"][..]] {
        let output = Command::cargo_bin("aix")
            .unwrap()
            .arg("models")
            .args(flag)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{flag:?}");
    }
}
