use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use serde_json::json;
use std::io::Write;
use std::process::Stdio;
use std::time::{Duration, Instant};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

fn write_config(dir: &TempDir, base_url: &str, extra: &str) -> std::path::PathBuf {
    let file = dir.child("aix.toml");
    file.write_str(&format!(
        r#"
default_profile = "test"

[endpoint]
base_url = "{base_url}"

[profiles.test]
api_key = "test-secret-key"

{extra}
"#
    ))
    .unwrap();
    file.path().to_path_buf()
}

#[test]
fn ask_non_interactive_without_a_profile_does_not_wait_for_piped_stdin() {
    let dir = TempDir::new().unwrap();
    let config = dir.child("aix.toml");
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://gateway.example"

[profiles.work]
api_key = "test-secret-key"
"#,
        )
        .unwrap();

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_aix"))
        .env_remove("AIX_PROFILE")
        .args([
            "--non-interactive",
            "--config",
            config.path().to_str().unwrap(),
            "ask",
            "question",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("ask waited for stdin instead of failing on missing profile");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("profile"), "{stderr}");
}

#[tokio::test]
async fn ask_sends_a_direct_prompt_with_the_configured_default_model() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Authorization", "Bearer test-secret-key"))
        .and(body_json(json!({
            "model": "gateway/general-model",
            "messages": [{ "role": "user", "content": "Explain TCP slow start" }]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "role": "assistant", "content": "TCP slow start begins with a small congestion window." } }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        &server.uri(),
        "[models]\ndefault = \"gateway/general-model\"",
    );
    let output = cmd()
        .args([
            "--config",
            config.to_str().unwrap(),
            "ask",
            "Explain TCP slow start",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        b"TCP slow start begins with a small congestion window.\n"
    );
    assert!(output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("test-secret-key"));
}

#[tokio::test]
async fn ask_keeps_a_piped_stdin_context_after_the_instruction() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_json(json!({
            "model": "gateway/general-model",
            "messages": [
                { "role": "user", "content": "Identify the likely failure" },
                { "role": "user", "content": "Context from stdin:\nnginx: upstream timed out" }
            ]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": "The upstream did not respond in time." } }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        &server.uri(),
        "[models]\ndefault = \"gateway/general-model\"",
    );
    let output = cmd()
        .args([
            "--config",
            config.to_str().unwrap(),
            "ask",
            "Identify the likely failure",
        ])
        .write_stdin("nginx: upstream timed out")
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"The upstream did not respond in time.\n");
}

#[tokio::test]
async fn ask_uses_stdin_as_the_user_message_when_no_prompt_is_given() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_json(json!({
            "model": "gateway/general-model",
            "messages": [{ "role": "user", "content": "hello from a pipe" }]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": "Hello from the gateway." } }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        &server.uri(),
        "[models]\ndefault = \"gateway/general-model\"",
    );
    let output = cmd()
        .args(["--config", config.to_str().unwrap(), "ask"])
        .write_stdin("hello from a pipe")
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"Hello from the gateway.\n");
}

#[tokio::test]
async fn ask_appends_multiple_explicit_files_after_stdin_in_cli_order() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let first = dir.child("first.txt");
    first.write_str("Invoice total: $42").unwrap();
    let second = dir.child("second.txt");
    second.write_str("Payment due: 2026-11-01").unwrap();

    let first_context = format!(
        "--- Begin file: {} ---\nInvoice total: $42\n--- End file: {} ---",
        first.path().display(),
        first.path().display()
    );
    let second_context = format!(
        "--- Begin file: {} ---\nPayment due: 2026-11-01\n--- End file: {} ---",
        second.path().display(),
        second.path().display()
    );
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_json(json!({
            "model": "gateway/general-model",
            "messages": [
                { "role": "user", "content": "Summarize the supplied material" },
                { "role": "user", "content": "Context from stdin:\nadditional note" },
                { "role": "user", "content": first_context },
                { "role": "user", "content": second_context }
            ]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": "The invoice is $42, due November 1." } }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let config = write_config(
        &dir,
        &server.uri(),
        "[models]\ndefault = \"gateway/general-model\"",
    );
    let output = cmd()
        .args([
            "--config",
            config.to_str().unwrap(),
            "ask",
            "--file",
            first.path().to_str().unwrap(),
            "--file",
            second.path().to_str().unwrap(),
            "Summarize the supplied material",
        ])
        .write_stdin("additional note")
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"The invoice is $42, due November 1.\n");
}

#[tokio::test]
async fn ask_includes_one_explicit_file_with_a_deterministic_boundary() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let file = dir.child("meeting-notes.txt");
    file.write_str("Decision: renew the lease in June.")
        .unwrap();
    let file_boundary = format!(
        "--- Begin file: {} ---\nDecision: renew the lease in June.\n--- End file: {} ---",
        file.path().display(),
        file.path().display()
    );
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_json(json!({
            "model": "gateway/general-model",
            "messages": [
                { "role": "user", "content": "Summarize the decision" },
                { "role": "user", "content": file_boundary }
            ]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": "The lease renewal is planned for June." } }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let config = write_config(
        &dir,
        &server.uri(),
        "[models]\ndefault = \"gateway/general-model\"",
    );
    let output = cmd()
        .args([
            "--config",
            config.to_str().unwrap(),
            "ask",
            "--file",
            file.path().to_str().unwrap(),
            "Summarize the decision",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"The lease renewal is planned for June.\n");
}

#[tokio::test]
async fn ask_resolves_aliases_and_passes_raw_model_ids_unchanged() {
    let server = MockServer::start().await;
    for (model, content) in [
        ("profile-fast-model", "Alias response."),
        ("provider/model-v7", "Raw model response."),
    ] {
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_json(json!({
                "model": model,
                "messages": [{ "role": "user", "content": "Explain this document" }]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{ "message": { "content": content } }]
            })))
            .expect(1)
            .mount(&server)
            .await;
    }

    let dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        &server.uri(),
        "[models]\ndefault = \"global-default\"\n[models.aliases]\nfast = \"global-fast-model\"\n[profiles.test.models.aliases]\nfast = \"profile-fast-model\"",
    );
    for (model, expected) in [
        ("fast", b"Alias response.\n".as_slice()),
        ("provider/model-v7", b"Raw model response.\n".as_slice()),
    ] {
        let output = cmd()
            .args([
                "--config",
                config.to_str().unwrap(),
                "ask",
                "--model",
                model,
                "Explain this document",
            ])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected);
    }
}

#[tokio::test]
async fn ask_sends_one_optional_system_message_before_user_content() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_json(json!({
            "model": "gateway/general-model",
            "messages": [
                { "role": "system", "content": "Use concise plain language." },
                { "role": "user", "content": "Summarize these obligations" }
            ]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": "The document sets two obligations." } }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        &server.uri(),
        "[models]\ndefault = \"gateway/general-model\"",
    );
    let output = cmd()
        .args([
            "--config",
            config.to_str().unwrap(),
            "ask",
            "--system",
            "Use concise plain language.",
            "Summarize these obligations",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"The document sets two obligations.\n");
}

#[tokio::test]
async fn ask_json_returns_the_stable_envelope_with_usage_and_no_input_fields() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "upstream-reported-model",
            "choices": [{ "message": { "content": "Three deadlines are listed." } }],
            "usage": {
                "prompt_tokens": 120,
                "completion_tokens": null
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        &server.uri(),
        "[models]\ndefault = \"gateway/general-model\"",
    );
    let output = cmd()
        .args([
            "--config",
            config.to_str().unwrap(),
            "--json",
            "ask",
            "Summarize private contract section",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        parsed,
        json!({
            "schema_version": 1,
            "command": "ask",
            "data": {
                "model": "gateway/general-model",
                "content": "Three deadlines are listed.",
                "usage": {
                    "prompt_tokens": 120,
                    "completion_tokens": null,
                    "total_tokens": null
                }
            }
        })
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("Summarize private contract section"));
    assert!(!stdout.contains("test-secret-key"));
    assert!(!stdout.contains("upstream-reported-model"));
}

#[test]
fn ask_without_a_prompt_or_piped_input_fails_with_usage_guidance() {
    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, "http://127.0.0.1:1", "");
    let output = cmd()
        .args(["--config", config.to_str().unwrap(), "ask"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("non-empty piped stdin"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn ask_with_terminal_stdin_and_no_prompt_fails_before_interaction() {
    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, "http://127.0.0.1:1", "");
    let terminal = nix::pty::openpty(None, None).expect("can create a pseudoterminal");
    drop(terminal.master);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_aix"))
        .args(["--config", config.to_str().unwrap(), "ask"])
        .stdin(std::fs::File::from(terminal.slave))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("non-empty piped stdin"), "{stderr}");
}

#[test]
fn ask_help_exposes_only_the_instruction_as_a_positional_argument() {
    let output = cmd().args(["ask", "--help"]).output().unwrap();

    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("Usage: aix ask [OPTIONS] [PROMPT]"));
    assert!(help.contains("--file <PATH>"));
    assert!(!help.contains("[PROFILE]"));
}

#[test]
fn ask_requires_an_instruction_when_only_files_are_supplied() {
    let dir = TempDir::new().unwrap();
    let file = dir.child("notes.txt");
    file.write_str("private context that must not be read without an instruction")
        .unwrap();
    let output = cmd()
        .args(["ask", "--file", file.path().to_str().unwrap()])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("files alone are context"));
    assert!(!stderr.contains("private context"));
}

#[test]
fn ask_reports_unreadable_explicit_files_as_local_input_errors() {
    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, "http://127.0.0.1:1", "");
    let file = dir.path().join("missing.txt");
    let output = cmd()
        .args([
            "--config",
            config.to_str().unwrap(),
            "ask",
            "--file",
            file.to_str().unwrap(),
            "Summarize the provided text",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("failed to read explicit input file"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("missing.txt"));
}

#[test]
fn ask_requires_an_explicit_or_configured_model() {
    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, "http://127.0.0.1:1", "");
    let output = cmd()
        .args([
            "--config",
            config.to_str().unwrap(),
            "ask",
            "Summarize this document",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no model specified"));
    assert!(stderr.contains("--model <MODEL>"));
}

#[tokio::test]
async fn ask_maps_authentication_failures_to_the_auth_exit_category() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": { "message": "invalid credential", "api_key": "test-secret-key" }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        &server.uri(),
        "[models]\ndefault = \"gateway/general-model\"",
    );
    let output = cmd()
        .args([
            "--config",
            config.to_str().unwrap(),
            "--json",
            "ask",
            "Summarize this document",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("HTTP 401"));
    assert!(!stderr.contains("test-secret-key"));
}

#[tokio::test]
async fn ask_maps_connection_failures_to_the_network_exit_category() {
    let dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        "http://127.0.0.1:1",
        "[models]\ndefault = \"gateway/general-model\"",
    );
    let output = cmd()
        .args([
            "--config",
            config.to_str().unwrap(),
            "ask",
            "Summarize this document",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("HTTP request failed"));
}

#[tokio::test]
async fn ask_treats_success_without_assistant_text_as_a_gateway_protocol_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": null } }],
            "debug_payload": "must not be printed"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        &server.uri(),
        "[models]\ndefault = \"gateway/general-model\"",
    );
    let output = cmd()
        .args([
            "--config",
            config.to_str().unwrap(),
            "ask",
            "Summarize this document",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no assistant text"));
    assert!(!stderr.contains("must not be printed"));
}

#[tokio::test]
async fn ask_treats_a_successful_non_json_response_as_a_protocol_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string("invalid upstream payload"))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        &server.uri(),
        "[models]\ndefault = \"gateway/general-model\"",
    );
    let output = cmd()
        .args([
            "--config",
            config.to_str().unwrap(),
            "ask",
            "Summarize this document",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("protocol error"));
    assert!(!stderr.contains("invalid upstream payload"));
}

#[tokio::test]
async fn ask_does_not_echo_prompt_context_or_secret_in_gateway_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(502).set_body_json(json!({
            "error": {
                "message": "prompt=Summarize this confidential document; context=private context",
                "authorization": "Bearer test-secret-key",
                "api_key": "test-secret-key"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        &server.uri(),
        "[models]\ndefault = \"gateway/general-model\"",
    );
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_aix"))
        .args([
            "--config",
            config.to_str().unwrap(),
            "--json",
            "ask",
            "Summarize this confidential document",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"private context")
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    for private_text in [
        "Summarize this confidential document",
        "private context",
        "test-secret-key",
        "authorization",
    ] {
        assert!(
            !stderr.contains(private_text),
            "leaked {private_text}: {stderr}"
        );
    }
}

#[tokio::test]
async fn ask_does_not_discover_neighboring_files_or_git_state() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_json(json!({
            "model": "gateway/general-model",
            "messages": [{ "role": "user", "content": "Explain TCP slow start" }]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": "It gradually increases the sending rate." } }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    dir.child("contract.txt")
        .write_str("neighboring document must not be sent")
        .unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::write(
        dir.path().join(".git/config"),
        "git metadata must not be inspected",
    )
    .unwrap();
    let config = write_config(
        &dir,
        &server.uri(),
        "[models]\ndefault = \"gateway/general-model\"",
    );
    let output = cmd()
        .current_dir(dir.path())
        .args([
            "--config",
            config.to_str().unwrap(),
            "ask",
            "Explain TCP slow start",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"It gradually increases the sending rate.\n");
}
