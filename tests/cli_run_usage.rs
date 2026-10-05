use assert_cmd::Command;
use assert_fs::prelude::*;
use serde_json::Value;
use std::path::Path;
#[cfg(unix)]
use std::time::Duration;
use wiremock::MockServer;

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

fn gateway_config(
    upstream: &MockServer,
    executable: &Path,
    api_format: &str,
    profile_key: &str,
) -> assert_fs::NamedTempFile {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(&format!(
            r#"
[endpoint]
base_url = {:?}

[profiles.work]
api_key = {:?}

[tools.gateway_test]
command = {:?}
api_format = {:?}
local_gateway = true
"#,
            upstream.uri(),
            profile_key,
            executable.to_string_lossy(),
            api_format,
        ))
        .unwrap();
    config
}

fn gateway_child_command(
    config: &assert_fs::NamedTempFile,
    state: &assert_fs::TempDir,
    mode: &str,
) -> Command {
    let mut command = cmd();
    command
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_GATEWAY_RUN_TEST_CHILD", mode)
        .args([
            "run",
            "--profile",
            "work",
            "--",
            "gateway_test",
            "--exact",
            "gateway_run_child_sends_private_requests",
            "--nocapture",
        ]);
    command
}

fn run_id_from_output(output: &[u8]) -> uuid::Uuid {
    String::from_utf8_lossy(output)
        .lines()
        .find_map(|line| uuid::Uuid::parse_str(line).ok())
        .expect("run printed its ID")
}

#[tokio::test]
#[cfg(unix)]
async fn gateway_managed_run_persists_mixed_protocol_usage_without_content() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chat-response-private-marker",
            "model": "gpt-run-summary",
            "choices": [{"message": {"role": "assistant", "content": "response-secret-marker"}}],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 10,
                "total_tokens": 110,
                "prompt_tokens_details": {"cached_tokens": 30}
            }
        })))
        .expect(2)
        .mount(&upstream)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "message-response-private-marker",
            "model": "claude-run-summary",
            "content": [{"type": "text", "text": "anthropic-response-secret-marker"}],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 7,
                "cache_read_input_tokens": 5,
                "cache_creation_input_tokens": 3
            }
        })))
        .expect(1)
        .mount(&upstream)
        .await;

    let state = assert_fs::TempDir::new().unwrap();
    let executable = std::env::current_exe().unwrap();
    let config = gateway_config(&upstream, &executable, "both", "profile-api-secret-marker");

    let output = gateway_child_command(&config, &state, "1")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let run_id = run_id_from_output(&output);

    let output = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", &run_id.to_string(), "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let envelope: Value = serde_json::from_slice(&output).unwrap();
    let record = &envelope["data"];
    let usage = &record["usage"];
    assert_eq!(record["schema_version"], 3);
    assert_eq!(record["status"], "succeeded");
    assert_eq!(usage["request_count"], 3);
    assert_eq!(usage["successful_requests"], 3);
    assert_eq!(usage["failed_requests"], 0);
    assert_eq!(usage["input_tokens_total"], 218);
    assert_eq!(usage["input_tokens_uncached"], 150);
    assert_eq!(usage["cache_read_input_tokens"], 65);
    assert!(usage["cache_write_input_tokens"].is_null());
    assert_eq!(usage["output_tokens"], 27);
    assert_eq!(usage["total_tokens"], 245);
    assert_eq!(
        usage["models"],
        serde_json::json!(["claude-run-summary", "gpt-run-summary"])
    );
    assert_eq!(
        usage["protocols"],
        serde_json::json!(["anthropic_messages", "openai_chat_completions"])
    );

    let output = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let list: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(list["data"][0]["usage"]["request_count"], 3);

    let output = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", &run_id.to_string()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let human = String::from_utf8(output).unwrap();
    assert!(human.contains("Input tokens total: 218"));
    assert!(human.contains("Cache-read input tokens: 65"));
    assert!(human.contains("Output tokens: 27"));
    assert!(human.contains("Models: [\"claude-run-summary\", \"gpt-run-summary\"]"));
    assert!(human.contains("Protocols: [\"anthropic_messages\", \"openai_chat_completions\"]"));

    fn collect_files(path: &std::path::Path, contents: &mut Vec<u8>) {
        if path.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                collect_files(&entry.unwrap().path(), contents);
            }
        } else {
            contents.extend(std::fs::read(path).unwrap());
        }
    }
    let mut persisted = Vec::new();
    collect_files(state.path(), &mut persisted);
    let persisted = String::from_utf8_lossy(&persisted);
    for marker in [
        "prompt-secret-marker",
        "response-secret-marker",
        "anthropic-prompt-secret-marker",
        "anthropic-response-secret-marker",
        "profile-api-secret-marker",
        "chat-response-private-marker",
        "message-response-private-marker",
    ] {
        assert!(!persisted.contains(marker), "persisted marker {marker}");
    }
}

#[tokio::test]
#[cfg(unix)]
async fn interrupted_gateway_run_keeps_usage_written_before_interrupt() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "response-id",
            "model": "gpt-interrupt-test",
            "choices": [{"message": {"role": "assistant", "content": "ok"}}],
            "usage": {
                "prompt_tokens": 21,
                "completion_tokens": 5,
                "total_tokens": 26
            }
        })))
        .expect(1)
        .mount(&upstream)
        .await;

    let state = assert_fs::TempDir::new().unwrap();
    let executable = std::env::current_exe().unwrap();
    let config = gateway_config(&upstream, &executable, "openai", "sk-interrupt-test");

    let output = gateway_child_command(&config, &state, "interrupt")
        .assert()
        .code(130)
        .get_output()
        .stdout
        .clone();
    let run_id = run_id_from_output(&output);

    let output = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", &run_id.to_string(), "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let envelope: Value = serde_json::from_slice(&output).unwrap();
    let record = &envelope["data"];
    assert_eq!(record["status"], "interrupted");
    assert_eq!(record["usage"]["request_count"], 1);
    assert_eq!(record["usage"]["successful_requests"], 1);
    assert_eq!(record["usage"]["total_tokens"], 26);
}

#[tokio::test]
#[cfg(unix)]
async fn gateway_run_child_sends_private_requests() {
    let mode = std::env::var("AIX_GATEWAY_RUN_TEST_CHILD").unwrap_or_default();
    if mode != "1" && mode != "interrupt" {
        return;
    }
    let interrupt_after_request = mode == "interrupt";
    let client = reqwest::Client::new();
    let openai_url = std::env::var("OPENAI_BASE_URL").unwrap();
    let openai_key = std::env::var("OPENAI_API_KEY").unwrap();
    for _ in 0..if interrupt_after_request { 1 } else { 2 } {
        let response = client
            .post(format!("{openai_url}/chat/completions"))
            .bearer_auth(&openai_key)
            .json(&serde_json::json!({
                "model": "gpt-run-summary",
                "messages": [{"role": "user", "content": "prompt-secret-marker"}]
            }))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        response.bytes().await.unwrap();
    }

    if interrupt_after_request {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::getppid;
        use std::io::Write;

        println!("{}", std::env::var("AIX_RUN_ID").unwrap());
        std::io::stdout().flush().unwrap();
        kill(getppid(), Signal::SIGINT).unwrap();
        std::thread::sleep(Duration::from_secs(5));
        return;
    }

    let anthropic_url = std::env::var("ANTHROPIC_BASE_URL").unwrap();
    let anthropic_key = std::env::var("ANTHROPIC_API_KEY").unwrap();
    let response = client
        .post(format!("{anthropic_url}/v1/messages"))
        .header("x-api-key", anthropic_key)
        .header("anthropic-version", "2023-06-01")
        .json(&serde_json::json!({
            "model": "claude-run-summary",
            "max_tokens": 20,
            "messages": [{"role": "user", "content": "anthropic-prompt-secret-marker"}]
        }))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    response.bytes().await.unwrap();
    println!("{}", std::env::var("AIX_RUN_ID").unwrap());
}
