use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const API_KEY: &str = "sk-gate-parent-secret";

fn write_config(dir: &TempDir, base_url: &str, api_key: &str, policy: &str) -> std::path::PathBuf {
    let file = dir.child("aix.toml");
    file.write_str(&format!(
        r#"
default_profile = "work"

[endpoint]
base_url = "{base_url}"
gateway = "litellm"

[profiles.work]
api_key = "{api_key}"

[models.aliases]
smart = "provider/model-smart"
fast = "provider/model-fast"

[run_policies.implement]
max_budget = 3.0
max_duration = "2h"
allowed_models = ["smart", "fast"]
{policy}
"#
    ))
    .unwrap();
    file.path().to_path_buf()
}

fn check<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["data"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == name)
        .unwrap_or_else(|| panic!("missing {name} check in {report}"))
}

fn write_chatgpt_credentials(auth_dir: &std::path::Path) {
    std::fs::create_dir_all(auth_dir).unwrap();
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    std::fs::write(
        auth_dir.join("profile-706572736f6e616c.json"),
        format!(
            r#"{{
  "version": 1,
  "profile": "personal",
  "client_id": "gate-test-client",
  "subject": "gate-test-subject",
  "email": null,
  "scopes": ["openid", "chatgpt.tokens.use.direct"],
  "id_token": null,
  "access_token": "gate-test-access-token",
  "refresh_token": "gate-test-refresh-token",
  "expires_at": {expires_at},
  "earliest_refresh_at": null
}}"#
        ),
    )
    .unwrap();
}

#[tokio::test]
async fn a_satisfiable_policy_returns_a_stable_json_report_without_mutating_the_gateway() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": "provider/model-fast" }, { "id": "provider/model-smart" }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .and(header("Authorization", format!("Bearer {API_KEY}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "info": { "spend": 1.25, "max_budget": 10.0 }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), API_KEY, "");
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env_remove("AIX_PROFILE")
        .args([
            "--config",
            config.to_str().unwrap(),
            "gate",
            "--policy",
            "implement",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["command"], "gate");
    assert_eq!(
        report["data"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|check| check["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "config",
            "policy",
            "profile",
            "credentials",
            "gateway",
            "models",
            "duration",
            "local_gateway",
            "litellm",
            "budget_info",
            "budget"
        ]
    );
    for name in [
        "config",
        "policy",
        "profile",
        "credentials",
        "gateway",
        "models",
        "litellm",
        "budget_info",
        "budget",
    ] {
        assert_eq!(check(&report, name)["status"], "pass", "{name}: {report}");
    }
    assert_eq!(
        check(&report, "models")["details"]["required"],
        json!(["provider/model-smart", "provider/model-fast"])
    );
    let output_text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output_text.contains(API_KEY));
    assert!(!output_text.contains(&server.uri()));
    assert!(output.stderr.is_empty());

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| request.method == "GET"));
    assert!(requests.iter().all(|request| {
        request.url.path() == "/v1/models" || request.url.path() == "/key/info"
    }));
}

#[tokio::test]
async fn no_budget_policy_on_custom_gateway_skips_litellm_and_checks_local_model_enforcement() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": "provider/model-smart" }]
        })))
        .expect(0)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let file = dir.child("custom-gateway-policy.toml");
    file.write_str(&format!(
        r#"
default_profile = "work"
[endpoint]
base_url = "{}"
gateway = "custom-gateway"
[profiles.work]
api_key = "{}"
[models.aliases]
smart = "provider/model-smart"
[tools.review]
api_format = "openai"
local_gateway = true
[run_policies.implement]
max_duration = "2h"
allowed_models = ["smart"]
"#,
        server.uri(),
        API_KEY
    ))
    .unwrap();

    let output = Command::cargo_bin("aix")
        .unwrap()
        .env_remove("AIX_PROFILE")
        .args([
            "--config",
            file.path().to_str().unwrap(),
            "gate",
            "--policy",
            "implement",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(check(&report, "gateway")["status"], "pass");
    assert_eq!(check(&report, "models")["status"], "pass");
    assert_eq!(check(&report, "duration")["status"], "pass");
    assert_eq!(check(&report, "local_gateway")["status"], "pass");
    assert_eq!(check(&report, "litellm")["status"], "not_applicable");
    assert_eq!(check(&report, "budget_info")["status"], "not_applicable");
    assert_eq!(check(&report, "budget")["status"], "not_applicable");
    let requests = server.received_requests().await.unwrap();
    assert!(requests.is_empty());
}

#[test]
fn gate_fails_closed_when_allowed_models_have_no_local_enforcement_path() {
    let dir = TempDir::new().unwrap();
    let config = dir.child("unsupported-local-policy.toml");
    config
        .write_str(&format!(
            r#"
default_profile = "work"
[endpoint]
base_url = "http://127.0.0.1:1"
gateway = "custom-gateway"
[profiles.work]
api_key = "{}"
[run_policies.local]
max_duration = "1h"
allowed_models = ["model-a"]
"#,
            API_KEY
        ))
        .unwrap();

    let output = Command::cargo_bin("aix")
        .unwrap()
        .env_remove("AIX_PROFILE")
        .args([
            "--config",
            config.path().to_str().unwrap(),
            "gate",
            "--policy",
            "local",
            "--json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(check(&report, "models")["status"], "pass");
    assert_eq!(check(&report, "local_gateway")["status"], "fail");
    assert!(check(&report, "local_gateway")["message"]
        .as_str()
        .unwrap()
        .contains("requires a configured local-gateway tool"));
    assert_eq!(check(&report, "litellm")["status"], "not_applicable");
}

#[test]
fn chatgpt_policy_gate_checks_local_readiness_without_litellm_and_rejects_budget() {
    let dir = TempDir::new().unwrap();
    let file = dir.child("chatgpt-policy.toml");
    let auth_dir = dir.path().join("auth");
    write_chatgpt_credentials(&auth_dir);
    let base_config = r#"
default_profile = "personal"
[profiles.personal]
auth = { type = "chatgpt" }
[tools.opencode]
command = "opencode"
api_format = "openai"
[tools.opencode.chatgpt]
transport = "local_gateway"
access_token_env = "ACCESS_TOKEN"
[run_policies.local]
profile = "personal"
max_duration = "2h"
allowed_models = ["gpt-6-luna"]
"#;
    file.write_str(base_config).unwrap();

    let output = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_AUTH_DIR", &auth_dir)
        .args([
            "--config",
            file.path().to_str().unwrap(),
            "gate",
            "--policy",
            "local",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(check(&report, "credentials")["status"], "pass");
    assert_eq!(check(&report, "gateway")["status"], "pass");
    assert_eq!(check(&report, "models")["status"], "pass");
    assert_eq!(check(&report, "local_gateway")["status"], "pass");
    assert_eq!(check(&report, "litellm")["status"], "not_applicable");
    assert_eq!(check(&report, "budget")["status"], "not_applicable");

    file.write_str(&base_config.replace(
        "max_duration = \"2h\"",
        "max_budget = 2.0\nmax_duration = \"2h\"",
    ))
    .unwrap();
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_AUTH_DIR", &auth_dir)
        .args([
            "--config",
            file.path().to_str().unwrap(),
            "gate",
            "--policy",
            "local",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(6));
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(check(&report, "budget")["status"], "fail");
    assert!(check(&report, "budget")["message"]
        .as_str()
        .unwrap()
        .contains("monetary budget enforcement unavailable for this transport"));
}

fn run_chatgpt_local_model_gate(
    dir: &TempDir,
    tool_name: &str,
    binding: &str,
) -> std::process::Output {
    let auth_dir = dir.path().join("auth");
    write_chatgpt_credentials(&auth_dir);
    let config = dir.child("chatgpt-local-model-policy.toml");
    config
        .write_str(&format!(
            r#"
default_profile = "personal"
[profiles.personal]
auth = {{ type = "chatgpt" }}
[tools.{tool_name}]
command = "{tool_name}"
api_format = "openai"
[tools.{tool_name}.chatgpt]
access_token_env = "ACCESS_TOKEN"
{binding}
[run_policies.local]
profile = "personal"
max_duration = "2h"
allowed_models = ["gpt-6-luna"]
"#
        ))
        .unwrap();
    Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_AUTH_DIR", &auth_dir)
        .args([
            "--config",
            config.path().to_str().unwrap(),
            "gate",
            "--policy",
            "local",
            "--json",
        ])
        .output()
        .unwrap()
}

#[test]
fn codex_app_server_local_gateway_passes_chatgpt_model_capability_preflight() {
    let dir = TempDir::new().unwrap();
    let output = run_chatgpt_local_model_gate(
        &dir,
        "codex",
        "transport = \"local_gateway\"\nprepend_args = [\"app-server\", \"--listen\", \"stdio://\"]",
    );

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(check(&report, "gateway")["status"], "pass");
    assert_eq!(check(&report, "models")["status"], "pass");
    assert_eq!(check(&report, "local_gateway")["status"], "pass");
    assert!(check(&report, "local_gateway")["message"]
        .as_str()
        .unwrap()
        .contains("ChatGPT local Responses adapter"));
}

#[test]
fn direct_chatgpt_transport_fails_closed_for_allowed_model_enforcement() {
    let dir = TempDir::new().unwrap();
    let output = run_chatgpt_local_model_gate(&dir, "codex", "transport = \"direct\"");

    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(check(&report, "gateway")["status"], "fail");
    assert_eq!(check(&report, "local_gateway")["status"], "fail");
    assert!(check(&report, "local_gateway")["message"]
        .as_str()
        .unwrap()
        .contains("no configured ChatGPT local Responses adapter"));
}

#[test]
fn unsupported_chatgpt_binding_fails_closed_for_allowed_model_enforcement() {
    let dir = TempDir::new().unwrap();
    let output = run_chatgpt_local_model_gate(&dir, "other", "transport = \"direct\"");

    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(check(&report, "gateway")["status"], "fail");
    assert_eq!(check(&report, "local_gateway")["status"], "fail");
    assert!(!serde_json::to_string(&report).unwrap().contains("OpenCode"));
}

#[test]
fn an_unknown_policy_fails_with_config_exit_semantics_and_a_json_report() {
    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, "http://127.0.0.1:1", API_KEY, "");
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env_remove("AIX_PROFILE")
        .args([
            "--config",
            config.to_str().unwrap(),
            "gate",
            "--policy",
            "unknown",
            "--json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(report["command"], "gate");
    assert_eq!(check(&report, "policy")["status"], "fail");
    assert_eq!(check(&report, "profile")["status"], "pass");
    assert!(check(&report, "policy")["message"]
        .as_str()
        .unwrap()
        .contains("not defined"));
}

#[test]
fn an_invalid_config_is_reported_without_echoing_its_contents() {
    let dir = TempDir::new().unwrap();
    let file = dir.child("invalid.toml");
    file.write_str(
        "[endpoint]\nbase_url = \"https://config-secret.example/path\"\n\
         [profiles.work\napi_key = \"config-api-key-secret\"\n",
    )
    .unwrap();
    let output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            file.path().to_str().unwrap(),
            "gate",
            "--policy",
            "implement",
            "--json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(check(&report, "config")["status"], "fail");
    for name in ["policy", "profile", "credentials", "gateway", "models"] {
        assert_eq!(check(&report, name)["status"], "skipped");
    }
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(!text.contains("config-secret.example"));
    assert!(!text.contains("config-api-key-secret"));
}

#[test]
fn an_unresolvable_profile_fails_with_config_exit_semantics_without_prompting() {
    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, "http://127.0.0.1:1", API_KEY, "");
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env_remove("AIX_PROFILE")
        .args([
            "--config",
            config.to_str().unwrap(),
            "--profile",
            "missing",
            "gate",
            "--policy",
            "implement",
            "--json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(check(&report, "profile")["status"], "fail");
    assert_eq!(check(&report, "credentials")["status"], "skipped");
    assert_eq!(check(&report, "gateway")["status"], "skipped");
}

#[test]
fn an_unresolved_profile_secret_fails_with_secret_exit_semantics_and_redacted_output() {
    let dir = TempDir::new().unwrap();
    let file = dir.child("secret-config.toml");
    file.write_str(
        r#"
default_profile = "work"

[endpoint]
base_url = "https://gateway.example/private-path"
gateway = "litellm"

[profiles.work]
api_key = { env = "AIX_GATE_MISSING_KEY" }

[run_policies.implement]
max_budget = 3.0
max_duration = "2h"
"#,
    )
    .unwrap();
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env_remove("AIX_PROFILE")
        .env_remove("AIX_GATE_MISSING_KEY")
        .args([
            "--config",
            file.path().to_str().unwrap(),
            "gate",
            "--policy",
            "implement",
            "--json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(check(&report, "credentials")["status"], "fail");
    assert_eq!(check(&report, "gateway")["status"], "skipped");
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(!text.contains("https://gateway.example/private-path"));
    assert!(!text.contains("AIX_GATE_MISSING_KEY"));
}

#[tokio::test]
async fn gateway_authentication_failure_uses_auth_exit_semantics_without_secret_leaks() {
    let server = MockServer::start().await;
    let base_url = format!("{}/gateway-private", server.uri());
    Mock::given(method("GET"))
        .and(path("/gateway-private/v1/models"))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_string(format!("upstream-diagnostic {API_KEY} at {base_url}")),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/gateway-private/key/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "info": { "spend": 1.0, "max_budget": 10.0 }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &base_url, API_KEY, "");
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env_remove("AIX_PROFILE")
        .args([
            "--config",
            config.to_str().unwrap(),
            "gate",
            "--policy",
            "implement",
            "--json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(check(&report, "gateway")["status"], "fail");
    assert_eq!(check(&report, "budget_info")["status"], "pass");
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(!text.contains(API_KEY));
    assert!(!text.contains(&base_url));
    assert!(!text.contains("upstream-diagnostic"));
}

#[tokio::test]
async fn a_gateway_timeout_uses_network_exit_semantics_while_independent_checks_run() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_millis(400))
                .set_body_json(json!({ "data": [{ "id": "provider/model-smart" }] })),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "info": { "spend": 1.0, "max_budget": 10.0 }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), API_KEY, "");
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env_remove("AIX_PROFILE")
        .args([
            "--timeout",
            "50ms",
            "--config",
            config.to_str().unwrap(),
            "gate",
            "--policy",
            "implement",
            "--json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(check(&report, "gateway")["status"], "fail");
    assert_eq!(check(&report, "models")["status"], "skipped");
    assert_eq!(check(&report, "budget_info")["status"], "pass");
}

#[tokio::test]
async fn a_missing_required_model_fails_with_policy_exit_semantics() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": "provider/model-smart" }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "info": { "spend": 1.0, "max_budget": 10.0 }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), API_KEY, "");
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env_remove("AIX_PROFILE")
        .args([
            "--config",
            config.to_str().unwrap(),
            "gate",
            "--policy",
            "implement",
            "--json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(6));
    assert!(output.stdout.is_empty());
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(check(&report, "models")["status"], "fail");
    assert_eq!(
        check(&report, "models")["details"]["missing"],
        json!(["provider/model-fast"])
    );
    assert_eq!(check(&report, "budget")["status"], "pass");
}

#[tokio::test]
async fn insufficient_remaining_parent_budget_fails_closed_with_budget_exit_semantics() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "id": "provider/model-smart" },
                { "id": "provider/model-fast" }
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "info": { "spend": 0.5, "max_budget": 2.0 }
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), API_KEY, "");
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env_remove("AIX_PROFILE")
        .args([
            "--config",
            config.to_str().unwrap(),
            "gate",
            "--policy",
            "implement",
            "--json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(6));
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(check(&report, "budget_info")["status"], "pass");
    assert_eq!(check(&report, "budget")["status"], "fail");
    assert_eq!(check(&report, "budget")["details"]["remaining"], 1.5);
}

#[tokio::test]
async fn an_explicitly_unlimited_parent_budget_passes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "id": "provider/model-smart" },
                { "id": "provider/model-fast" }
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "info": { "spend": 123.0, "max_budget": null }
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), API_KEY, "");
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env_remove("AIX_PROFILE")
        .args([
            "--config",
            config.to_str().unwrap(),
            "gate",
            "--policy",
            "implement",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(check(&report, "budget_info")["status"], "pass");
    assert_eq!(
        check(&report, "budget_info")["details"]["max_budget"],
        Value::Null
    );
    assert_eq!(check(&report, "budget")["status"], "pass");
    assert!(check(&report, "budget")["message"]
        .as_str()
        .unwrap()
        .contains("unlimited"));
}

#[tokio::test]
async fn unavailable_parent_budget_information_fails_closed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "id": "provider/model-smart" },
                { "id": "provider/model-fast" }
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "message": "management endpoint unavailable"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), API_KEY, "");
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env_remove("AIX_PROFILE")
        .args([
            "--config",
            config.to_str().unwrap(),
            "gate",
            "--policy",
            "implement",
            "--json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(check(&report, "budget_info")["status"], "fail");
    assert_eq!(check(&report, "budget")["status"], "skipped");
}

#[tokio::test]
async fn a_non_litellm_gateway_is_rejected_without_management_or_mutating_requests() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "id": "provider/model-smart" },
                { "id": "provider/model-fast" }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let file = dir.child("custom-gateway.toml");
    file.write_str(&format!(
        r#"
default_profile = "work"
[endpoint]
base_url = "{}"
gateway = "custom-gateway"
[profiles.work]
api_key = "{}"
[models.aliases]
smart = "provider/model-smart"
fast = "provider/model-fast"
[run_policies.implement]
max_budget = 3.0
max_duration = "2h"
allowed_models = ["smart", "fast"]
"#,
        server.uri(),
        API_KEY
    ))
    .unwrap();
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env_remove("AIX_PROFILE")
        .args([
            "--config",
            file.path().to_str().unwrap(),
            "gate",
            "--policy",
            "implement",
            "--json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(check(&report, "gateway")["status"], "pass");
    assert_eq!(check(&report, "litellm")["status"], "fail");
    assert_eq!(check(&report, "budget_info")["status"], "skipped");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].url.path(), "/v1/models");
}

#[tokio::test]
async fn a_fixed_policy_profile_is_used_and_conflicting_override_fails_preflight() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "id": "provider/model-smart" },
                { "id": "provider/model-fast" }
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "info": { "spend": 1.0, "max_budget": 10.0 }
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let file = dir.child("fixed-profile.toml");
    file.write_str(&format!(
        r#"
[endpoint]
base_url = "{}"
gateway = "litellm"
[profiles.work]
api_key = "{}"
[models.aliases]
smart = "provider/model-smart"
fast = "provider/model-fast"
[run_policies.implement]
profile = "work"
max_budget = 3.0
max_duration = "2h"
allowed_models = ["smart", "fast"]
"#,
        server.uri(),
        API_KEY
    ))
    .unwrap();
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env_remove("AIX_PROFILE")
        .args([
            "--config",
            file.path().to_str().unwrap(),
            "--profile",
            "other",
            "gate",
            "--policy",
            "implement",
            "--json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(check(&report, "profile")["status"], "fail");
    assert_eq!(check(&report, "credentials")["status"], "pass");
    assert_eq!(check(&report, "gateway")["status"], "pass");
    assert_eq!(check(&report, "budget")["status"], "pass");
}

#[tokio::test]
async fn missing_parent_budget_fields_fail_closed_even_when_litellm_responds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "id": "provider/model-smart" },
                { "id": "provider/model-fast" }
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "info": { "spend": 1.0 }
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), API_KEY, "");
    let output = Command::cargo_bin("aix")
        .unwrap()
        .env_remove("AIX_PROFILE")
        .args([
            "--config",
            config.to_str().unwrap(),
            "gate",
            "--policy",
            "implement",
            "--json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(6));
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(check(&report, "budget_info")["status"], "fail");
    assert_eq!(check(&report, "budget")["status"], "skipped");
}
