#![cfg(unix)]

use assert_cmd::Command;
use assert_fs::prelude::*;
use predicates::prelude::*;
use std::os::unix::fs::PermissionsExt;

const NATIVE: &str = r#"
default_profile = "chatgpt"
[endpoint]
base_url = { env = "AIX_SUBSCRIPTION_MISSING_GATEWAY" }
[profiles.work]
api_key = { env = "AIX_SUBSCRIPTION_MISSING_KEY" }
[profiles.chatgpt]
auth = "native"
[profiles.chatgpt.env]
SUBSCRIPTION_ENV = "configured"
[profiles.chatgpt.models]
default = "subscription-model"
[profiles.chatgpt.models.aliases]
fast = "subscription-fast"
"#;

fn cmd(config: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("aix").unwrap();
    cmd.env("AIX_CONFIG", config)
        .env_remove("AIX_PROFILE")
        .env_remove("AIX_SUBSCRIPTION_MISSING_GATEWAY")
        .env_remove("AIX_SUBSCRIPTION_MISSING_KEY");
    for key in [
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
        "LITELLM_API_KEY",
        "LITELLM_BASE_URL",
        "CODEX_API_KEY",
    ] {
        cmd.env(key, "inherited-sentinel");
    }
    cmd
}

fn fixture(extra: &str, script: &str) -> (assert_fs::TempDir, std::path::PathBuf) {
    let dir = assert_fs::TempDir::new().unwrap();
    dir.child("backend").write_str(script).unwrap();
    std::fs::set_permissions(
        dir.child("backend").path(),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let path = dir.child("backend").path().display().to_string();
    dir.child("aix.toml")
        .write_str(&format!("{NATIVE}\n{}", extra.replace("BACKEND", &path)))
        .unwrap();
    let config = dir.child("aix.toml").path().to_path_buf();
    (dir, config)
}

#[test]
fn native_named_tools_and_run_use_profile_wiring_and_remove_inherited_credentials() {
    let (dir, config) = fixture(
        r#"
[tools.pi]
command = "shared-tool-must-not-run"
api_format = "openai"
[profiles.chatgpt.tools.codex]
command = "BACKEND"
args = ["--provider", "openai", "--model", "{model}"]
[profiles.chatgpt.tools.opencode]
command = "BACKEND"
args = ["--model", "openai/{model}"]
[profiles.chatgpt.tools.pi]
command = "BACKEND"
args = ["--provider", "openai-codex", "--model", "{model}"]
[profiles.chatgpt.tools.pi.env]
SUBSCRIPTION_ENV = "tool-override"
"#,
        "#!/bin/sh\nprintf '%s\\n' \"$AIX_PROFILE\" \"$SUBSCRIPTION_ENV\" \"$@\"\nenv\n",
    );
    for args in [
        vec!["codex", "--", "user-argument"],
        vec!["opencode", "chatgpt", "--", "user-argument"],
        vec!["pi", "--", "user-argument"],
        vec!["run", "--", "pi", "user-argument"],
    ] {
        let output = cmd(&config)
            .env("AIX_STATE_DIR", dir.path().join("state"))
            .args(&args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let output = String::from_utf8(output).unwrap();
        assert!(output.starts_with("chatgpt\n"));
        assert!(output.contains("subscription-model"));
        assert!(output.contains("user-argument"));
        assert!(!output.contains("inherited-sentinel"));
        if args.contains(&"pi") {
            assert!(output.contains("openai-codex\n"));
            assert!(output.contains("tool-override\n"));
        }
    }
    cmd(&config)
        .args(["pi", "--dry-run"])
        .assert()
        .success()
        .stderr(predicate::str::contains("Would unset:"))
        .stderr(predicate::str::contains("inherited-sentinel").not());
}

#[test]
fn native_claude_profile_launches_and_asks_without_gateway_auth_or_cloud_selectors() {
    let (_dir, config) = fixture(
        r#"
[profiles.anthropic]
auth = "native"
[profiles.anthropic.models]
default = "sonnet"
[profiles.anthropic.tools.claude]
command = "BACKEND"
args = ["--model", "{model}", "--settings", '{"forceLoginMethod":"claudeai"}']
[profiles.anthropic.ask]
command = "BACKEND"
args = ["--print", "--tools", "", "--model", "{model}", "--system-prompt", "{system}"]
"#,
        "#!/bin/sh\nprintf '<%s>\\n' \"$@\"\nprintf '%s\\n' \"${ANTHROPIC_AUTH_TOKEN-unset}\" \"${CLAUDE_CODE_USE_BEDROCK-unset}\" \"${CLAUDE_CODE_USE_VERTEX-unset}\" \"${CLAUDE_CODE_USE_FOUNDRY-unset}\"\ncat\n",
    );
    cmd(&config)
        .args(["claude", "anthropic", "--", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("<sonnet>"))
        .stdout(predicate::str::contains(
            "<{\"forceLoginMethod\":\"claudeai\"}>",
        ))
        .stdout(predicate::str::contains("inherited-sentinel").not());
    cmd(&config)
        .args([
            "--profile",
            "anthropic",
            "ask",
            "--system",
            "Answer briefly",
            "user input",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("<--tools>\n<>\n"))
        .stdout(predicate::str::contains("<sonnet>"))
        .stdout(predicate::str::contains("<Answer briefly>"))
        .stdout(predicate::str::contains("user input"))
        .stdout(predicate::str::contains("inherited-sentinel").not());
}

#[test]
fn native_env_unsets_gateway_variables_in_every_shell_format() {
    let (_dir, config) = fixture("", "#!/bin/sh\nexit 0\n");
    for (format, unset) in [
        ("sh", "unset OPENAI_API_KEY"),
        ("fish", "set -e OPENAI_API_KEY"),
        ("nu", "hide-env --ignore-errors OPENAI_API_KEY"),
        ("powershell", "Remove-Item Env:OPENAI_API_KEY"),
        ("cmd", "set \"OPENAI_API_KEY=\""),
    ] {
        cmd(&config)
            .args(["env", "--format", format])
            .assert()
            .success()
            .stdout(predicate::str::contains(unset))
            .stdout(predicate::str::contains("inherited-sentinel").not());
    }
    let output = cmd(&config)
        .args(["env", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["AIX_PROFILE"], "chatgpt");
    assert!(json.get("OPENAI_API_KEY").unwrap().is_null());
    assert_eq!(json["SUBSCRIPTION_ENV"], "configured");
}

const ASK: &str = r#"
[profiles.chatgpt.ask]
command = "BACKEND"
args = ["--model", "{model}", "--system-prompt", "{system}"]
"#;

#[test]
fn native_ask_passes_model_alias_system_stdin_and_explicit_files() {
    let (dir, config) = fixture(ASK, "#!/bin/sh\nprintf '%s\\n' \"$@\"\nprintf '%s\\n' \"${OPENAI_API_KEY-unset}\" \"$SUBSCRIPTION_ENV\"\ncat\n");
    dir.child("context.txt").write_str("file context").unwrap();
    let output = cmd(&config)
        .args([
            "--json",
            "ask",
            "--model",
            "fast",
            "--system",
            "literal {model}; $(echo unsafe)",
            "--file",
        ])
        .arg(dir.child("context.txt").path())
        .arg("instruction")
        .write_stdin("piped context")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["data"]["model"], "subscription-fast");
    let content = json["data"]["content"].as_str().unwrap();
    for expected in [
        "subscription-fast",
        "literal {model}; $(echo unsafe)",
        "unset",
        "configured",
        "instruction",
        "piped context",
        "file context",
    ] {
        assert!(content.contains(expected), "missing {expected}: {content}");
    }
    assert!(json["data"]["usage"]["total_tokens"].is_null());
    assert!(!content.contains("inherited-sentinel"));
}

#[test]
fn native_ask_failures_do_not_leak_backend_diagnostics() {
    let (_dir, config) = fixture(ASK, "#!/bin/sh\necho 'secret-sentinel' >&2\nexit 7\n");
    cmd(&config)
        .args(["ask", "prompt-sentinel"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("exit code: Some(7)"))
        .stderr(predicate::str::contains("secret-sentinel").not())
        .stderr(predicate::str::contains("prompt-sentinel").not());
}

#[test]
fn native_ask_requires_backend_and_rejects_empty_output() {
    let (_dir, config) = fixture("", "#!/bin/sh\nexit 0\n");
    cmd(&config)
        .args(["ask", "prompt"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("requires profiles.<name>.ask"));
    let (_dir, config) = fixture(ASK, "#!/bin/sh\ncat >/dev/null\n");
    cmd(&config)
        .args(["ask", "prompt"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("non-empty UTF-8 text"));
}

#[test]
fn native_ask_handles_large_input_while_the_backend_writes_output_first() {
    let (_dir, config) = fixture(
        ASK,
        "#!/bin/sh\ndd if=/dev/zero bs=65536 count=4 2>/dev/null | tr '\\000' o\ncat\n",
    );
    let input = "i".repeat(262_144);
    let output = cmd(&config)
        .timeout(std::time::Duration::from_secs(10))
        .args(["ask"])
        .write_stdin(input.clone())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        output,
        format!("{}{input}\n", "o".repeat(262_144)).as_bytes()
    );
}

#[test]
fn native_gateway_commands_fail_before_resolving_gateway_secrets() {
    let (_dir, config) = fixture("", "#!/bin/sh\nexit 0\n");
    for args in [
        vec!["models"],
        vec!["status"],
        vec!["spend"],
        vec!["usage", "--since", "7d"],
    ] {
        cmd(&config)
            .args(args)
            .assert()
            .code(2)
            .stderr(predicate::str::contains("requires an API-key profile"));
    }
}
