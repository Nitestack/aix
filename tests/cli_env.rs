use assert_cmd::Command;
use assert_fs::prelude::*;

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

// Config with direct (non-env) secret values so tests don't depend on env vars.
const CONFIG_DIRECT: &str = r#"
[endpoint]
base_url = "https://ai.example.com"
api_format = "anthropic"

[profiles.swtb]
api_key = "sk-swtb-key"

[profiles.work]
api_key = "sk-work-key"
"#;

const CONFIG_WITH_DEFAULT: &str = r#"
default_profile = "swtb"

[endpoint]
base_url = "https://ai.example.com"
api_format = "anthropic"

[profiles.swtb]
api_key = "sk-swtb-key"
"#;

const CONFIG_OPENAI_COMPAT: &str = r#"
[endpoint]
base_url = "https://ai.example.com"
api_format = "both"

[profiles.swtb]
api_key = "sk-swtb-key"
"#;

const CONFIG_NO_ANTHROPIC: &str = r#"
[endpoint]
base_url = "https://ai.example.com"
api_format = "openai"

[profiles.swtb]
api_key = "sk-swtb-key"
"#;

const CONFIG_SPECIAL_CHARS: &str = r#"
[endpoint]
base_url = "https://ai.example.com?token=abc&id=1"
api_format = "anthropic"

[profiles.test]
api_key = "sk-it's a test"
"#;

// --- sh format ---

#[test]
fn env_sh_contains_aix_vars() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "sh"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    assert!(s.contains("export AIX_PROFILE='swtb'"), "got: {s}");
    // anthropic_env defaults to true
    assert!(
        s.contains("export ANTHROPIC_API_KEY='sk-swtb-key'"),
        "got: {s}"
    );
    assert!(
        s.contains("export ANTHROPIC_BASE_URL='https://ai.example.com'"),
        "got: {s}"
    );
    // openai_env defaults to false
    assert!(!s.contains("OPENAI_"), "got: {s}");
}

#[test]
fn env_sh_default_format_is_sh() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb"]) // no --format flag → defaults to sh
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("export AIX_PROFILE='swtb'"), "got: {s}");
}

#[test]
fn env_sh_special_chars_in_key_and_url() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_SPECIAL_CHARS).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "test", "--format", "sh"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    // Single quote in API key must be escaped as '\''
    assert!(s.contains(r"'sk-it'\''s a test'"), "got: {s}");
    // URL with & and = preserved inside single quotes
    assert!(
        s.contains("'https://ai.example.com?token=abc&id=1'"),
        "got: {s}"
    );
}

// --- json format ---

#[test]
fn env_json_produces_valid_json() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value =
        serde_json::from_slice(&out).expect("output must be valid JSON");
    assert_eq!(parsed["AIX_PROFILE"], "swtb");
    assert_eq!(parsed["ANTHROPIC_API_KEY"], "sk-swtb-key");
    assert!(parsed.get("OPENAI_API_KEY").is_none());
}

#[test]
fn env_json_special_chars_round_trip() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_SPECIAL_CHARS).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "test", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value =
        serde_json::from_slice(&out).expect("output must be valid JSON");
    assert_eq!(parsed["AIX_PROFILE"], "test");
}

// --- nu format ---

#[test]
fn env_nu_contains_env_assignments() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "nu"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    assert!(s.contains("$env.AIX_PROFILE = \"swtb\""), "got: {s}");
}

// --- fish format ---

#[test]
fn env_fish_contains_set_x_lines() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "fish"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    assert!(s.contains("set -x AIX_PROFILE 'swtb'"), "got: {s}");
}

#[test]
fn env_fish_special_chars_escaped() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_SPECIAL_CHARS).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "test", "--format", "fish"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    // fish: single quote escaped as \'
    assert!(s.contains(r"'sk-it\'s a test'"), "got: {s}");
}

// --- powershell format ---

#[test]
fn env_powershell_contains_env_colon_assignments() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "powershell"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    assert!(s.contains("$env:AIX_PROFILE = 'swtb'"), "got: {s}");
}

#[test]
fn env_powershell_single_quote_doubled() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_SPECIAL_CHARS).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "test", "--format", "powershell"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    // PowerShell: single quote doubled → ''
    assert!(s.contains("'sk-it''s a test'"), "got: {s}");
}

// --- compat flags ---

#[test]
fn env_openai_compat_enabled_includes_openai_vars() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_OPENAI_COMPAT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(parsed["OPENAI_API_KEY"], "sk-swtb-key");
    assert_eq!(parsed["OPENAI_BASE_URL"], "https://ai.example.com/v1");
}

#[test]
fn env_no_anthropic_compat_omits_anthropic_vars() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_NO_ANTHROPIC).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert!(parsed.get("ANTHROPIC_API_KEY").is_none());
    assert_eq!(parsed["OPENAI_API_KEY"], "sk-swtb-key");
}

// --- profile selection ---

#[test]
fn env_uses_default_profile_when_none_specified() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_WITH_DEFAULT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("AIX_PROFILE") // prevent real env leaking in
        .args(["env", "--format", "json"]) // no positional profile
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(parsed["AIX_PROFILE"], "swtb");
}

#[test]
fn env_errors_when_no_profile_and_no_default() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap(); // no default_profile

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("AIX_PROFILE") // prevent real env leaking in
        .args(["env", "--format", "json"]) // no positional profile
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("profile") || s.contains("default"), "got: {s}");
}

#[test]
fn env_errors_when_named_profile_not_in_config() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "nonexistent", "--format", "json"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("nonexistent"), "got: {s}");
}

#[test]
fn env_no_config_file_exits_nonzero() {
    let home = assert_fs::TempDir::new().unwrap();
    cmd()
        .env("HOME", home.path())
        .env_remove("AIX_CONFIG")
        .env_remove("XDG_CONFIG_HOME")
        .args(["env", "swtb", "--format", "json"])
        .assert()
        .failure();
}

// --- missing / empty profiles ---

const CONFIG_NO_PROFILES: &str = r#"
[endpoint]
base_url = "https://example.com"
api_format = "anthropic"
"#;

const CONFIG_EMPTY_PROFILES: &str = r#"
[endpoint]
base_url = "https://example.com"
api_format = "anthropic"

[profiles]
"#;

#[test]
fn env_errors_on_config_with_no_profiles_section() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_NO_PROFILES).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "json"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(
        s.contains("[profiles]"),
        "expected mention of [profiles], got: {s}"
    );
    assert!(
        s.contains("profile"),
        "expected mention of profile, got: {s}"
    );
}

#[test]
fn env_errors_on_config_with_empty_profiles() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_EMPTY_PROFILES).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "json"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(
        s.contains("[profiles]"),
        "expected mention of [profiles], got: {s}"
    );
}

// --- unknown profile list formatting ---

const CONFIG_WITH_LABELS: &str = r#"
default_profile = "swtb"

[endpoint]
base_url = "https://example.com"
api_format = "anthropic"

[profiles.swtb]
label = "SWTB"
api_key = "sk-swtb"

[profiles.work]
label = "Work account"
api_key = "sk-work"
"#;

#[test]
fn env_unknown_profile_list_uses_single_space_before_label() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_WITH_LABELS).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "typo", "--format", "json"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("swtb (SWTB)"), "expected single space, got: {s}");
    assert!(
        s.contains("work (Work account)"),
        "expected single space, got: {s}"
    );
    assert!(!s.contains("swtb  ("), "found double space: {s}");
}

#[test]
fn env_unknown_profile_list_omits_parens_when_no_label() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap(); // CONFIG_DIRECT has no labels

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "typo", "--format", "json"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("swtb"), "should list profile name: {s}");
    assert!(
        !s.contains("swtb (swtb)"),
        "should not show name in parens: {s}"
    );
}

// --- cmd format ---

#[test]
fn env_cmd_contains_set_quoted_lines() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "cmd"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    assert!(s.contains(r#"set "AIX_PROFILE=swtb""#), "got: {s}");
    assert!(
        s.contains(r#"set "ANTHROPIC_API_KEY=sk-swtb-key""#),
        "got: {s}"
    );
}

#[test]
fn env_cmd_no_debug_on_stdout() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "cmd"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    for line in s.lines() {
        if !line.is_empty() {
            assert!(line.starts_with("set \""), "unexpected line: {line}");
        }
    }
}

#[test]
fn env_cmd_percent_in_url_is_doubled() {
    const CONFIG_PERCENT: &str = r#"
[endpoint]
base_url = "https://ai.example.com?token=abc%20def"
api_format = "anthropic"

[profiles.test]
api_key = "sk-key"
"#;
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_PERCENT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "test", "--format", "cmd"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    assert!(
        s.contains(r#"set "ANTHROPIC_BASE_URL=https://ai.example.com?token=abc%%20def""#),
        "got: {s}"
    );
}

// --- no debug/config data leaks ---

#[test]
fn env_output_does_not_contain_unrelated_config_fields() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DIRECT).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "swtb", "--format", "sh"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();

    assert!(!s.contains("api_format"), "got: {s}");
    assert!(!s.contains("aix.toml"), "got: {s}");
}
