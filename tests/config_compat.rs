/// Config compatibility tests — invariants guarding against accidental breakage.
///
/// Rule: TOML, YAML, JSON, and JSON5 configs expressing the same structure must
/// produce identical runtime behaviour from `aix env`.  Any schema change that
/// breaks one format must break all four fixtures here, forcing an explicit update.
use assert_cmd::Command;
use assert_fs::prelude::*;

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

// ---------------------------------------------------------------------------
// Equivalent configs in all four formats
// ---------------------------------------------------------------------------

const TOML: &str = r#"
default_profile = "work"

[endpoint]
base_url = "https://compat.example.com/v1"
api_format = "anthropic"
provider = "litellm"
gateway = "litellm"

[profiles.work]
label = "Work"
api_key = "sk-compat-work"

[profiles.local]
label = "Local"
api_key = "sk-compat-local"
"#;

const YAML: &str = r#"
default_profile: work
endpoint:
  base_url: "https://compat.example.com/v1"
  api_format: anthropic
  provider: litellm
  gateway: litellm
profiles:
  work:
    label: Work
    api_key: sk-compat-work
  local:
    label: Local
    api_key: sk-compat-local
"#;

const JSON: &str = r#"{
  "default_profile": "work",
  "endpoint": {
    "base_url": "https://compat.example.com/v1",
    "api_format": "anthropic",
    "provider": "litellm",
    "gateway": "litellm"
  },
  "profiles": {
    "work": { "label": "Work", "api_key": "sk-compat-work" },
    "local": { "label": "Local", "api_key": "sk-compat-local" }
  }
}"#;

const JSON5: &str = r#"{
  default_profile: "work",
  endpoint: {
    base_url: "https://compat.example.com/v1",
    api_format: "anthropic",
    provider: "litellm",
    gateway: "litellm",
  },
  profiles: {
    work: { label: "Work", api_key: "sk-compat-work" },
    local: { label: "Local", api_key: "sk-compat-local" },
  },
}"#;

// ---------------------------------------------------------------------------
// Helper: run `aix env work --format json` against a config file
// and return the parsed JSON output.
// ---------------------------------------------------------------------------

fn env_json_for(config: &str, ext: &str) -> serde_json::Value {
    let name = format!("aix.{ext}");
    let file = assert_fs::NamedTempFile::new(&name).unwrap();
    file.write_str(config).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "work", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    serde_json::from_slice(&out).expect("output must be valid JSON")
}

// ---------------------------------------------------------------------------
// Cross-format parity
// ---------------------------------------------------------------------------

#[test]
fn all_formats_emit_identical_env_vars() {
    let toml_out = env_json_for(TOML, "toml");
    let yaml_out = env_json_for(YAML, "yaml");
    let json_out = env_json_for(JSON, "json");
    let json5_out = env_json_for(JSON5, "json5");

    assert_eq!(toml_out, yaml_out, "TOML vs YAML mismatch");
    assert_eq!(toml_out, json_out, "TOML vs JSON mismatch");
    assert_eq!(toml_out, json5_out, "TOML vs JSON5 mismatch");
}

#[test]
fn all_formats_produce_correct_profile_name() {
    for (config, ext) in [
        (TOML, "toml"),
        (YAML, "yaml"),
        (JSON, "json"),
        (JSON5, "json5"),
    ] {
        let out = env_json_for(config, ext);
        assert_eq!(out["AIX_PROFILE"], "work", "wrong profile for {ext}: {out}");
    }
}

#[test]
fn all_formats_produce_correct_api_key() {
    for (config, ext) in [
        (TOML, "toml"),
        (YAML, "yaml"),
        (JSON, "json"),
        (JSON5, "json5"),
    ] {
        let out = env_json_for(config, ext);
        assert_eq!(
            out["AIX_API_KEY"], "sk-compat-work",
            "wrong api key for {ext}: {out}"
        );
    }
}

#[test]
fn all_formats_produce_correct_base_url() {
    for (config, ext) in [
        (TOML, "toml"),
        (YAML, "yaml"),
        (JSON, "json"),
        (JSON5, "json5"),
    ] {
        let out = env_json_for(config, ext);
        assert_eq!(
            out["AIX_BASE_URL"], "https://compat.example.com/v1",
            "wrong base_url for {ext}: {out}"
        );
    }
}

// ---------------------------------------------------------------------------
// api_format rendering contract
//
// These tests encode the exact variable set emitted for each api_format value.
// Adding a new ApiFormat variant must add a corresponding test here.
// ---------------------------------------------------------------------------

const CONFIG_ANTHROPIC: &str = r#"
[endpoint]
base_url = "https://gw.example.com/v1"
api_format = "anthropic"
[profiles.p]
api_key = "sk-anthro"
"#;

const CONFIG_OPENAI: &str = r#"
[endpoint]
base_url = "https://gw.example.com/v1"
api_format = "openai"
[profiles.p]
api_key = "sk-oai"
"#;

const CONFIG_BOTH: &str = r#"
[endpoint]
base_url = "https://gw.example.com/v1"
api_format = "both"
[profiles.p]
api_key = "sk-both"
"#;

fn env_for(config: &str) -> serde_json::Value {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(config).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "p", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    serde_json::from_slice(&out).expect("output must be valid JSON")
}

#[test]
fn api_format_anthropic_emits_exactly_5_vars() {
    let out = env_for(CONFIG_ANTHROPIC);
    let keys: Vec<&str> = out
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys.len(), 5, "expected 5 vars, got: {keys:?}");
}

#[test]
fn api_format_anthropic_emits_correct_var_names() {
    let out = env_for(CONFIG_ANTHROPIC);
    for key in [
        "AIX_PROFILE",
        "AIX_API_KEY",
        "AIX_BASE_URL",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
    ] {
        assert!(out.get(key).is_some(), "missing {key} for anthropic: {out}");
    }
    assert!(
        out.get("OPENAI_API_KEY").is_none(),
        "unexpected OPENAI_API_KEY: {out}"
    );
    assert!(
        out.get("OPENAI_BASE_URL").is_none(),
        "unexpected OPENAI_BASE_URL: {out}"
    );
}

#[test]
fn api_format_openai_emits_exactly_5_vars() {
    let out = env_for(CONFIG_OPENAI);
    let keys: Vec<&str> = out
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys.len(), 5, "expected 5 vars, got: {keys:?}");
}

#[test]
fn api_format_openai_emits_correct_var_names() {
    let out = env_for(CONFIG_OPENAI);
    for key in [
        "AIX_PROFILE",
        "AIX_API_KEY",
        "AIX_BASE_URL",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
    ] {
        assert!(out.get(key).is_some(), "missing {key} for openai: {out}");
    }
    assert!(
        out.get("ANTHROPIC_API_KEY").is_none(),
        "unexpected ANTHROPIC_API_KEY: {out}"
    );
    assert!(
        out.get("ANTHROPIC_BASE_URL").is_none(),
        "unexpected ANTHROPIC_BASE_URL: {out}"
    );
}

#[test]
fn api_format_both_emits_exactly_7_vars() {
    let out = env_for(CONFIG_BOTH);
    let keys: Vec<&str> = out
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys.len(), 7, "expected 7 vars, got: {keys:?}");
}

#[test]
fn api_format_both_emits_all_var_names() {
    let out = env_for(CONFIG_BOTH);
    for key in [
        "AIX_PROFILE",
        "AIX_API_KEY",
        "AIX_BASE_URL",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
    ] {
        assert!(out.get(key).is_some(), "missing {key} for both: {out}");
    }
}

#[test]
fn api_format_anthropic_and_openai_mirror_aix_values() {
    let out = env_for(CONFIG_BOTH);
    assert_eq!(out["AIX_API_KEY"], out["ANTHROPIC_API_KEY"]);
    assert_eq!(out["AIX_API_KEY"], out["OPENAI_API_KEY"]);
    assert_eq!(out["AIX_BASE_URL"], out["ANTHROPIC_BASE_URL"]);
    assert_eq!(out["AIX_BASE_URL"], out["OPENAI_BASE_URL"]);
}

// ---------------------------------------------------------------------------
// Secret non-leakage invariant
//
// Secret values must not appear in any non-env-output command.
// ---------------------------------------------------------------------------

const CONFIG_SECRET_LEAK_CHECK: &str = r#"
[endpoint]
base_url = "https://gw.example.com/v1"
api_format = "anthropic"
[profiles.p]
api_key = "sk-SENTINEL-MUST-NOT-LEAK"
"#;

#[test]
fn profiles_command_never_prints_secret_value() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_SECRET_LEAK_CHECK).unwrap();

    let output = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["profiles", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    assert!(
        !stdout.contains("sk-SENTINEL-MUST-NOT-LEAK"),
        "secret leaked to stdout: {stdout}"
    );
    assert!(
        !stderr.contains("sk-SENTINEL-MUST-NOT-LEAK"),
        "secret leaked to stderr: {stderr}"
    );
}

#[test]
#[cfg(unix)]
fn exec_dry_run_never_prints_secret_value() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_SECRET_LEAK_CHECK).unwrap();

    let output = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["exec", "p", "--dry-run", "--", "true"])
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    assert!(
        !stdout.contains("sk-SENTINEL-MUST-NOT-LEAK"),
        "secret leaked to stdout: {stdout}"
    );
    assert!(
        !stderr.contains("sk-SENTINEL-MUST-NOT-LEAK"),
        "secret leaked to stderr: {stderr}"
    );
}
