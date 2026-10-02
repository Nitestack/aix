use assert_cmd::Command;
use assert_fs::prelude::*;
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PARENT_KEY: &str = "sk-policy-parent";
const LEASE_KEY: &str = "sk-policy-lease";

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

#[test]
fn run_policy_configuration_passes_offline_validation() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "https://gateway.example"

[profiles.work]
api_key = "test-key"

[models.aliases]
smart = "provider/model-smart"
fast = "provider/model-fast"

[run_policies.implement]
profile = "work"
max_budget = 3.00
max_duration = "2h"
allowed_models = ["smart", "fast"]
tags = ["phase:implement"]
"#,
        )
        .unwrap();

    cmd()
        .env("AIX_CONFIG", config.path())
        .args(["config", "validate"])
        .assert()
        .success();
}

#[test]
fn run_policy_validation_rejects_unsafe_or_incomplete_constraints() {
    let invalid_policies = [
        (
            "empty name",
            r#"[run_policies.""]
max_budget = 1
max_duration = "1h"
"#,
            "names must not be empty",
        ),
        (
            "non-positive budget",
            r#"[run_policies.implement]
max_budget = 0
max_duration = "1h"
"#,
            "max_budget greater than zero",
        ),
        (
            "non-finite budget",
            r#"[run_policies.implement]
max_budget = inf
max_duration = "1h"
"#,
            "max_budget greater than zero",
        ),
        (
            "missing budget",
            r#"[run_policies.implement]
max_duration = "1h"
"#,
            "max_budget",
        ),
        (
            "missing duration",
            r#"[run_policies.implement]
max_budget = 1
"#,
            "max_duration",
        ),
        (
            "invalid duration",
            r#"[run_policies.implement]
max_budget = 1
max_duration = "forever"
"#,
            "valid positive max_duration",
        ),
        (
            "non-positive duration",
            r#"[run_policies.implement]
max_budget = 1
max_duration = "0h"
"#,
            "valid positive max_duration",
        ),
        (
            "empty model allowlist",
            r#"[run_policies.implement]
max_budget = 1
max_duration = "1h"
allowed_models = []
"#,
            "non-empty list",
        ),
        (
            "empty model name",
            r#"[run_policies.implement]
max_budget = 1
max_duration = "1h"
allowed_models = [""]
"#,
            "must not contain empty strings",
        ),
        (
            "empty tag",
            r#"[run_policies.implement]
max_budget = 1
max_duration = "1h"
tags = [""]
"#,
            "tags must not contain empty strings",
        ),
        (
            "undefined fixed profile",
            r#"[run_policies.implement]
profile = "missing"
max_budget = 1
max_duration = "1h"
"#,
            "references undefined profile",
        ),
    ];

    for (label, policy, expected_error) in invalid_policies {
        let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
        config
            .write_str(&format!(
                "[endpoint]\nbase_url = \"https://gateway.example\"\n\
                 [profiles.work]\napi_key = \"test-key\"\n\n{policy}"
            ))
            .unwrap();

        let output = cmd()
            .env("AIX_CONFIG", config.path())
            .args(["config", "validate"])
            .assert()
            .code(2)
            .get_output()
            .clone();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected_error), "{label}: {stderr}");
    }
}

#[test]
fn policy_list_and_show_are_offline_and_expose_resolved_models_as_json() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
default_profile = "work"

[endpoint]
base_url = "http://127.0.0.1:1"

[profiles.work]
api_key = "test-key"

[models.aliases]
smart = "provider/model-smart"
fast = "provider/model-fast"

[run_policies.implement]
profile = "work"
max_budget = 3.00
max_duration = "2h"
allowed_models = ["smart", "fast"]
tags = ["phase:implement"]

[run_policies.research]
max_budget = 1.50
max_duration = "1h"
allowed_models = ["fast", "smart"]
tags = ["phase:research"]
"#,
        )
        .unwrap();

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .args(["policies", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let envelope: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(envelope["command"], "policies");
    assert_eq!(envelope["data"][0]["name"], "implement");
    assert_eq!(
        envelope["data"][0]["resolved_models"],
        serde_json::json!(["provider/model-smart", "provider/model-fast"])
    );
    assert_eq!(envelope["data"][1]["name"], "research");
    assert!(!envelope.to_string().contains("test-key"));

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .arg("policies")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let human_list = String::from_utf8(output).unwrap();
    assert!(human_list.find("implement").unwrap() < human_list.find("research").unwrap());
    assert!(human_list.contains("Max budget: $3.00"));

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .args(["policy", "show", "implement", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let envelope: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(envelope["command"], "policy show");
    assert_eq!(envelope["data"]["name"], "implement");
    assert_eq!(envelope["data"]["max_budget"], 3.0);

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .args(["policy", "show", "implement"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let human = String::from_utf8(output).unwrap();
    assert!(human.contains("Max budget: $3.00"));
    assert!(human.contains("provider/model-smart"));
    assert!(!human.contains("test-key"));
}

#[test]
fn policy_runs_default_to_caps_and_allow_only_lower_budget_and_duration() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "http://127.0.0.1:1"

[profiles.work]
api_key = "test-key"

[run_policies.implement]
profile = "work"
max_budget = 3.0
max_duration = "2h"
"#,
        )
        .unwrap();

    let defaulted = cmd()
        .env("AIX_CONFIG", config.path())
        .args(["run", "--policy", "implement", "--dry-run", "--", "true"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&defaulted.stderr);
    assert!(stderr.contains("budget: $3.00"));
    assert!(stderr.contains("duration: 2h"));
    assert!(stderr.contains("AIX_RUN_POLICY"));

    cmd()
        .env("AIX_CONFIG", config.path())
        .args([
            "run",
            "--policy",
            "implement",
            "--budget",
            "1.50",
            "--duration",
            "90m",
            "--dry-run",
            "--",
            "true",
        ])
        .assert()
        .success();

    for (option, value, expected_error) in [
        ("--budget", "3.01", "exceeds run policy"),
        ("--duration", "121m", "exceeds run policy"),
    ] {
        let output = cmd()
            .env("AIX_CONFIG", config.path())
            .args([
                "run",
                "--policy",
                "implement",
                option,
                value,
                "--dry-run",
                "--",
                "true",
            ])
            .assert()
            .code(2)
            .get_output()
            .clone();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(expected_error),
            "{option} {value}: {stderr}"
        );
    }
}

#[test]
fn fixed_policy_profile_cannot_be_overridden_and_unfixed_policy_uses_normal_profile() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
default_profile = "work"

[endpoint]
base_url = "http://127.0.0.1:1"

[profiles.work]
api_key = "work-key"

[profiles.personal]
api_key = "personal-key"

[run_policies.fixed]
profile = "work"
max_budget = 1
max_duration = "1h"

[run_policies.flexible]
max_budget = 1
max_duration = "1h"
"#,
        )
        .unwrap();

    let conflict = cmd()
        .env("AIX_CONFIG", config.path())
        .args([
            "run",
            "--profile",
            "personal",
            "--policy",
            "fixed",
            "--dry-run",
            "--",
            "true",
        ])
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert!(
        String::from_utf8_lossy(&conflict.stderr).contains("conflicts with the global --profile")
    );

    let fixed = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_PROFILE", "personal")
        .args(["run", "--policy", "fixed", "--dry-run", "--", "true"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(String::from_utf8_lossy(&fixed.stderr).contains("profile: work"));

    let env_conflict = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_PROFILE", "personal")
        .args([
            "run",
            "--profile",
            "personal",
            "--policy",
            "fixed",
            "--dry-run",
            "--",
            "true",
        ])
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert!(String::from_utf8_lossy(&env_conflict.stderr)
        .contains("conflicts with the global --profile"));

    let flexible = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_PROFILE", "personal")
        .args(["run", "--policy", "flexible", "--dry-run", "--", "true"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(String::from_utf8_lossy(&flexible.stderr).contains("profile: personal"));
}

#[test]
fn policy_model_aliases_are_resolved_and_cli_allowlist_cannot_widen_policy() {
    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(
            r#"
[endpoint]
base_url = "http://127.0.0.1:1"

[profiles.work]
api_key = "test-key"

[profiles.work.models.aliases]
fast = "provider/fast-v2"
smart = "provider/smart-v3"

[run_policies.review]
profile = "work"
max_budget = 1
max_duration = "30m"
allowed_models = ["smart", "fast"]
"#,
        )
        .unwrap();

    let defaulted = cmd()
        .env("AIX_CONFIG", config.path())
        .args(["run", "--policy", "review", "--dry-run", "--", "true"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&defaulted.stderr);
    assert!(stderr.contains("provider/smart-v3, provider/fast-v2"));

    let subset = cmd()
        .env("AIX_CONFIG", config.path())
        .args([
            "run",
            "--policy",
            "review",
            "--allow-model",
            "fast",
            "--dry-run",
            "--",
            "true",
        ])
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(String::from_utf8_lossy(&subset.stderr).contains("provider/fast-v2"));
    assert!(!String::from_utf8_lossy(&subset.stderr).contains("provider/smart-v3"));

    let widened = cmd()
        .env("AIX_CONFIG", config.path())
        .args([
            "run",
            "--policy",
            "review",
            "--allow-model",
            "provider/outside-v1",
            "--dry-run",
            "--",
            "true",
        ])
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert!(
        String::from_utf8_lossy(&widened.stderr).contains("not allowed by the selected run policy")
    );

    let credential_in_model = cmd()
        .env("AIX_CONFIG", config.path())
        .args([
            "run",
            "--policy",
            "review",
            "--allow-model",
            "test-key",
            "--dry-run",
            "--",
            "true",
        ])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&credential_in_model.stderr);
    assert!(stderr.contains("not allowed by the selected run policy"));
    assert!(!stderr.contains("test-key"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(unix)]
async fn policy_run_uses_a_lease_and_records_effective_guardrails_and_tags() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/key/generate"))
        .and(header("Authorization", format!("Bearer {PARENT_KEY}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "key": LEASE_KEY,
            "expires": "2030-01-02T03:04:05Z"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .and(header("Authorization", format!("Bearer {PARENT_KEY}")))
        .and(query_param("key", LEASE_KEY))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "info": { "spend": 0.25 }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/key/delete"))
        .and(header("Authorization", format!("Bearer {PARENT_KEY}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "deleted": true })))
        .expect(1)
        .mount(&server)
        .await;

    let config = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    config
        .write_str(&format!(
            r#"
[endpoint]
base_url = "{}"
gateway = "litellm"

[profiles.work]
api_key = {{ env = "AIX_PARENT_KEY" }}

[profiles.work.models.aliases]
smart = "provider/model-smart"

[run_policies.implement]
profile = "work"
max_budget = 3.00
max_duration = "2h"
allowed_models = ["smart"]
tags = ["phase:implement", "duplicate"]
"#,
            server.uri()
        ))
        .unwrap();
    let state = assert_fs::TempDir::new().unwrap();
    let script = r#"test "$OPENAI_API_KEY" = sk-policy-lease && test "$LITELLM_API_KEY" = sk-policy-lease && test "$AIX_PROFILE" = work && test "$AIX_RUN_POLICY" = implement && printf '%s\n%s\n' "$AIX_RUN_ID" "$AIX_RUN_TAGS""#;

    let output = cmd()
        .env("AIX_CONFIG", config.path())
        .env("AIX_STATE_DIR", state.path())
        .env("AIX_PARENT_KEY", PARENT_KEY)
        .args([
            "run",
            "--policy",
            "implement",
            "--budget",
            "1.50",
            "--duration",
            "45m",
            "--allow-model",
            "smart",
            "--tag",
            "duplicate",
            "--tag",
            "issue:17",
            "--",
            "sh",
            "-c",
            script,
        ])
        .assert()
        .success()
        .get_output()
        .clone();
    let child_output = String::from_utf8(output.stdout.clone()).unwrap();
    let mut lines = child_output.lines();
    let run_id = lines.next().unwrap();
    assert!(uuid::Uuid::parse_str(run_id).is_ok());
    let child_tags: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    let expected_tags = json!([
        "phase:implement",
        "duplicate",
        "issue:17",
        format!("aix:run:{run_id}")
    ]);
    assert_eq!(child_tags, expected_tags);
    let output_text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output_text.contains(PARENT_KEY));
    assert!(!output_text.contains(LEASE_KEY));

    let record_output = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", run_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let envelope: Value = serde_json::from_slice(&record_output).unwrap();
    let record = &envelope["data"];
    assert_eq!(record["schema_version"], 2);
    assert_eq!(record["policy"]["name"], "implement");
    assert_eq!(record["policy"]["effective_budget"], 1.5);
    assert_eq!(record["policy"]["effective_duration"], "45m");
    assert_eq!(
        record["policy"]["effective_allowed_models"],
        json!(["provider/model-smart"])
    );
    assert_eq!(record["policy"]["effective_tags"], expected_tags);
    assert_eq!(
        record["tags"],
        json!(["phase:implement", "duplicate", "issue:17"])
    );
    assert_eq!(record["lease"]["budget"], 1.5);
    assert_eq!(record["lease"]["duration"], "45m");
    assert_eq!(
        record["lease"]["allowed_models"],
        json!(["provider/model-smart"])
    );
    assert!(!envelope.to_string().contains(PARENT_KEY));
    assert!(!envelope.to_string().contains(LEASE_KEY));

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    let generate: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(generate["max_budget"], 1.5);
    assert_eq!(generate["duration"], "45m");
    assert_eq!(generate["models"], json!(["provider/model-smart"]));
    assert_eq!(generate["metadata"]["tags"], expected_tags);

    let human_record = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", run_id])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let human_record = String::from_utf8(human_record).unwrap();
    assert!(human_record.contains("Policy: \"implement\""));
    assert!(human_record.contains("Effective budget: $1.50"));
}

#[test]
fn run_policy_configuration_is_compatible_across_supported_formats() {
    let configs = [
        (
            "toml",
            r#"
default_profile = "work"
[endpoint]
base_url = "https://gateway.example"
[profiles.work]
api_key = "test-key"
[models.aliases]
quick = "provider/model-quick"
[run_policies.research]
max_budget = 1.5
max_duration = "1h"
allowed_models = ["quick"]
tags = ["phase:research"]
"#,
        ),
        (
            "yaml",
            r#"
default_profile: work
endpoint:
  base_url: https://gateway.example
profiles:
  work:
    api_key: test-key
models:
  aliases:
    quick: provider/model-quick
run_policies:
  research:
    max_budget: 1.5
    max_duration: 1h
    allowed_models: [quick]
    tags: [phase:research]
"#,
        ),
        (
            "json",
            r#"{
  "default_profile": "work",
  "endpoint": { "base_url": "https://gateway.example" },
  "profiles": { "work": { "api_key": "test-key" } },
  "models": { "aliases": { "quick": "provider/model-quick" } },
  "run_policies": {
    "research": {
      "max_budget": 1.5,
      "max_duration": "1h",
      "allowed_models": ["quick"],
      "tags": ["phase:research"]
    }
  }
}"#,
        ),
        (
            "json5",
            r#"{
  default_profile: "work",
  endpoint: { base_url: "https://gateway.example" },
  profiles: { work: { api_key: "test-key" } },
  models: { aliases: { quick: "provider/model-quick" } },
  run_policies: {
    research: {
      max_budget: 1.5,
      max_duration: "1h",
      allowed_models: ["quick"],
      tags: ["phase:research"],
    },
  },
}"#,
        ),
    ];

    let mut baseline = None;
    for (extension, content) in configs {
        let config = assert_fs::NamedTempFile::new(format!("aix.{extension}")).unwrap();
        config.write_str(content).unwrap();
        let output = cmd()
            .env("AIX_CONFIG", config.path())
            .args(["policy", "show", "research", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let policy: Value = serde_json::from_slice(&output).unwrap();
        if let Some(baseline) = &baseline {
            assert_eq!(&policy, baseline, "output differs for {extension}");
        } else {
            assert_eq!(
                policy["data"]["resolved_models"],
                json!(["provider/model-quick"])
            );
            baseline = Some(policy);
        }
    }
}

#[test]
fn run_history_without_policy_metadata_remains_readable() {
    let state = assert_fs::TempDir::new().unwrap();
    let run_id = "4b9a85df-51d9-49a4-9a17-69d7f0dc91f1";
    let run_dir = state.path().join("runs").join(run_id);
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(
        run_dir.join("00000000000000000001.json"),
        format!(
            r#"{{
  "schema_version": 1,
  "run_id": "{run_id}",
  "name": null,
  "workflow": null,
  "task_id": null,
  "tags": [],
  "profile": "work",
  "logical_tool_name": null,
  "executable_name": "sh",
  "started_at_unix_ms": 100,
  "finished_at_unix_ms": 101,
  "duration_ms": 1,
  "process_exit_code": 0,
  "status": "succeeded",
  "lease": null
}}"#
        ),
    )
    .unwrap();

    let output = cmd()
        .env("AIX_STATE_DIR", state.path())
        .args(["runs", "show", run_id, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let envelope: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(envelope["data"]["schema_version"], 1);
    assert!(envelope["data"]["policy"].is_null());
}
