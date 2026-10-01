use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use serde_json::json;
use std::io::Write;
use std::process::Stdio;
use wiremock::matchers::{body_json, method, path};
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
fn prompt_list_is_sorted_and_does_not_expose_preset_bodies() {
    let dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        "http://127.0.0.1:1",
        r#"
[prompts.z-diagnose]
prompt = "PRIVATE DIAGNOSTIC INSTRUCTION"
system = "PRIVATE SYSTEM INSTRUCTION"
model = "smart"

[prompts.a-summarize]
prompt = "PRIVATE SUMMARY INSTRUCTION"
"#,
    );

    let output = cmd()
        .args([
            "--config",
            config.to_str().unwrap(),
            "prompt",
            "--json",
            "--list",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        parsed,
        json!({
            "schema_version": 1,
            "command": "prompt",
            "data": [
                { "name": "a-summarize", "model": null, "has_system": false },
                { "name": "z-diagnose", "model": "smart", "has_system": true }
            ]
        })
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("PRIVATE"));
}

#[test]
fn prompt_human_list_shows_names_and_model_overrides_only() {
    let dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        "http://127.0.0.1:1",
        r#"
[prompts.z-diagnose]
prompt = "PRIVATE INSTRUCTION"
model = "fast"

[prompts.a-summarize]
prompt = "PRIVATE SUMMARY"
"#,
    );

    let output = cmd()
        .args(["--config", config.to_str().unwrap(), "prompt", "--list"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "a-summarize\nz-diagnose (model: fast)\n"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE"));
}

#[test]
fn unknown_prompt_names_include_sorted_available_presets() {
    let dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        "http://127.0.0.1:1",
        r#"
[prompts.z-last]
prompt = "Last"
[prompts.a-first]
prompt = "First"
"#,
    );

    let output = cmd()
        .args(["--config", config.to_str().unwrap(), "prompt", "missing"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("prompt preset \"missing\" is not defined"));
    assert!(stderr.find("a-first").unwrap() < stderr.find("z-last").unwrap());
    assert!(!stderr.contains("Last"));
}

#[tokio::test]
async fn prompt_uses_cli_preset_profile_and_global_model_precedence() {
    let server = MockServer::start().await;
    let cases = [
        ("global-cli-model", "cli takes precedence"),
        ("global-preset-model", "preset takes precedence"),
        ("profile-default-model", "profile default"),
        ("global-default-model", "global default"),
    ];
    for (model, instruction) in cases {
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_json(json!({
                "model": model,
                "messages": [{ "role": "user", "content": instruction }]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{ "message": { "content": "ok" } }]
            })))
            .expect(1)
            .mount(&server)
            .await;
    }

    let dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        &server.uri(),
        r#"
[models]
default = "global-default-model"
[models.aliases]
cli = "global-cli-model"
preset = "global-preset-model"

[profiles.test.models]
default = "profile-default-model"

[profiles.no-profile-default]
api_key = "test-secret-key"

[prompts.cli-wins]
prompt = "cli takes precedence"
model = "preset"

[prompts.preset-wins]
prompt = "preset takes precedence"
model = "preset"

[prompts.profile-wins]
prompt = "profile default"

[prompts.global-wins]
prompt = "global default"
"#,
    );

    for args in [
        vec!["prompt", "cli-wins", "--model", "cli"],
        vec!["prompt", "preset-wins"],
        vec!["prompt", "profile-wins"],
        vec!["--profile", "no-profile-default", "prompt", "global-wins"],
    ] {
        let mut command = cmd();
        command.args(["--config", config.to_str().unwrap()]);
        command.args(args);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[tokio::test]
async fn prompt_reuses_ask_context_rules_and_never_discovers_local_context() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let explicit_file = dir.child("invoice.txt");
    explicit_file.write_str("Invoice total: $42").unwrap();
    let file_context = format!(
        "--- Begin file: {} ---\nInvoice total: $42\n--- End file: {} ---",
        explicit_file.path().display(),
        explicit_file.path().display()
    );
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_json(json!({
            "model": "general-model",
            "messages": [
                { "role": "system", "content": "Be precise." },
                { "role": "user", "content": "Extract payment details." },
                { "role": "user", "content": "Context from stdin:\nDue on Friday." },
                { "role": "user", "content": file_context }
            ]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": "Amount: $42; due Friday." } }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    dir.child("neighbor.txt")
        .write_str("must not be read")
        .unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::write(dir.path().join(".git/config"), "git must not be inspected").unwrap();
    let config = write_config(
        &dir,
        &server.uri(),
        r#"
[prompts.extract-actions]
prompt = "Extract payment details."
system = "Be precise."
model = "general-model"
"#,
    );

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_aix"))
        .current_dir(dir.path())
        .args([
            "--config",
            config.to_str().unwrap(),
            "prompt",
            "extract-actions",
            "--file",
            explicit_file.path().to_str().unwrap(),
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
        .write_all(b"Due on Friday.")
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"Amount: $42; due Friday.\n");
}

#[tokio::test]
async fn prompt_execution_json_uses_the_shared_inference_contract() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_json(json!({
            "model": "provider/model-v2",
            "messages": [{ "role": "user", "content": "Summarize arbitrary notes." }]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": "Two items are listed." } }],
            "usage": { "prompt_tokens": 20, "completion_tokens": 6, "total_tokens": 26 }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        &server.uri(),
        "[prompts.summarize]\nprompt = \"Summarize arbitrary notes.\"\nmodel = \"provider/model-v2\"",
    );

    let output = cmd()
        .args([
            "--config",
            config.to_str().unwrap(),
            "--json",
            "prompt",
            "summarize",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        parsed,
        json!({
            "schema_version": 1,
            "command": "prompt",
            "data": {
                "model": "provider/model-v2",
                "content": "Two items are listed.",
                "usage": {
                    "prompt_tokens": 20,
                    "completion_tokens": 6,
                    "total_tokens": 26
                }
            }
        })
    );
}

#[tokio::test]
async fn code_oriented_names_and_prompt_text_have_no_special_interpretation() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let code_file = dir.child("change.diff");
    code_file.write_str("+let answer = 42;").unwrap();
    let file_context = format!(
        "--- Begin file: {} ---\n+let answer = 42;\n--- End file: {} ---",
        code_file.path().display(),
        code_file.path().display()
    );
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_json(json!({
            "model": "general-model",
            "messages": [
                {
                    "role": "user",
                    "content": "Review supplied text literally: {{input}}; $HOME; $(printf sentinel)."
                },
                { "role": "user", "content": file_context }
            ]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": "The supplied line assigns 42." } }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let config = write_config(
        &dir,
        &server.uri(),
        r#"
[prompts.review-code]
prompt = "Review supplied text literally: {{input}}; $HOME; $(printf sentinel)."
model = "general-model"
"#,
    );

    let output = cmd()
        .args([
            "--config",
            config.to_str().unwrap(),
            "prompt",
            "review-code",
            "--file",
            code_file.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"The supplied line assigns 42.\n");
}

#[test]
fn config_validation_rejects_empty_prompt_fields_offline() {
    for (extra, expected) in [
        (
            "[prompts.\" \" ]\nprompt = \"valid\"",
            "prompt names must not be empty",
        ),
        ("[prompts.bad]\nprompt = \"  \"", "non-empty prompt"),
        (
            "[prompts.bad]\nprompt = \"valid\"\nsystem = \" \"",
            "non-empty system",
        ),
        (
            "[prompts.bad]\nprompt = \"valid\"\nmodel = \"\"",
            "non-empty model",
        ),
    ] {
        let dir = TempDir::new().unwrap();
        let config = write_config(&dir, "http://127.0.0.1:1", extra);
        let output = cmd()
            .args(["--config", config.to_str().unwrap(), "config", "validate"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "expected {expected:?}, got {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
