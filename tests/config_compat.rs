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
base_url = "https://compat.example.com"
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
  base_url: "https://compat.example.com"
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
    "base_url": "https://compat.example.com",
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
    base_url: "https://compat.example.com",
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
            out["ANTHROPIC_API_KEY"], "sk-compat-work",
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
            out["ANTHROPIC_BASE_URL"], "https://compat.example.com",
            "wrong base_url for {ext}: {out}"
        );
    }
}

// ---------------------------------------------------------------------------
// env variable emission contract
//
// `aix env` / `aix shell` / `aix exec` always emit all 5 variables regardless
// of the gateway config, because these commands don't know what tool will run.
// Named-tool dispatch (`aix claude`, `aix pi`) selects the format by tool name.
// ---------------------------------------------------------------------------

const CONFIG_SIMPLE: &str = r#"
[endpoint]
base_url = "https://gw.example.com"
[profiles.p]
api_key = "sk-test"
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
fn env_always_emits_exactly_7_vars() {
    let out = env_for(CONFIG_SIMPLE);
    let keys: Vec<&str> = out
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys.len(), 7, "expected 7 vars, got: {keys:?}");
}

#[test]
fn env_always_emits_all_var_names() {
    let out = env_for(CONFIG_SIMPLE);
    for key in [
        "AIX_PROFILE",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "LITELLM_API_KEY",
        "LITELLM_BASE_URL",
    ] {
        assert!(out.get(key).is_some(), "missing {key}: {out}");
    }
    assert!(
        out.get("AIX_API_KEY").is_none(),
        "unexpected AIX_API_KEY: {out}"
    );
    assert!(
        out.get("AIX_BASE_URL").is_none(),
        "unexpected AIX_BASE_URL: {out}"
    );
}

#[test]
fn env_openai_url_has_v1_suffix() {
    let out = env_for(CONFIG_SIMPLE);
    let anthropic_url = out["ANTHROPIC_BASE_URL"].as_str().unwrap();
    let openai_url = out["OPENAI_BASE_URL"].as_str().unwrap();
    assert_eq!(openai_url, format!("{anthropic_url}/v1"));
}

// ---------------------------------------------------------------------------
// Secret non-leakage invariant
//
// Secret values must not appear in any non-env-output command.
// ---------------------------------------------------------------------------

const CONFIG_SECRET_LEAK_CHECK: &str = r#"
[endpoint]
base_url = "https://gw.example.com"
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

#[test]
fn model_configuration_is_supported_in_all_file_formats() {
    let configs = [
        (
            "toml",
            r#"
[endpoint]
base_url = "https://compat.example.com"
[models]
default = "global-model"
[models.aliases]
fast = "global-fast-model"
[profiles.work]
api_key = "sk-test"
[profiles.work.models]
default = "work-model"
[profiles.work.models.aliases]
fast = "work-fast-model"
"#,
        ),
        (
            "yaml",
            r#"
endpoint:
  base_url: https://compat.example.com
models:
  default: global-model
  aliases:
    fast: global-fast-model
profiles:
  work:
    api_key: sk-test
    models:
      default: work-model
      aliases:
        fast: work-fast-model
"#,
        ),
        (
            "json",
            r#"{
  "endpoint": { "base_url": "https://compat.example.com" },
  "models": {
    "default": "global-model",
    "aliases": { "fast": "global-fast-model" }
  },
  "profiles": {
    "work": {
      "api_key": "sk-test",
      "models": {
        "default": "work-model",
        "aliases": { "fast": "work-fast-model" }
      }
    }
  }
}"#,
        ),
        (
            "json5",
            r#"{
  endpoint: { base_url: "https://compat.example.com" },
  models: {
    default: "global-model",
    aliases: { fast: "global-fast-model" },
  },
  profiles: {
    work: {
      api_key: "sk-test",
      models: {
        default: "work-model",
        aliases: { fast: "work-fast-model" },
      },
    },
  },
}"#,
        ),
    ];

    for (ext, config) in configs {
        let file = assert_fs::NamedTempFile::new(format!("aix.{ext}")).unwrap();
        file.write_str(config).unwrap();

        cmd()
            .env("AIX_CONFIG", file.path())
            .args(["config", "validate"])
            .assert()
            .success();
    }
}
