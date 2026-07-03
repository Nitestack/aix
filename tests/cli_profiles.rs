use assert_cmd::Command;
use assert_fs::prelude::*;

fn cmd() -> Command {
    Command::cargo_bin("aix").expect("binary exists")
}

// Two profiles with labels. "fast" sorts before "work" alphabetically.
const CONFIG: &str = r#"
[endpoint]
base_url = "https://ai.example.com/v1"
api_format = "anthropic"

[profiles.work]
label = "Work account"
api_key = "sk-work"

[profiles.fast]
label = "Fast model"
api_key = "sk-fast"
"#;

// Profiles with no labels — name is used as display label.
const CONFIG_NO_LABELS: &str = r#"
[endpoint]
base_url = "https://ai.example.com/v1"
api_format = "anthropic"

[profiles.alpha]
api_key = "sk-alpha"

[profiles.beta]
api_key = "sk-beta"
"#;

// Profile with no label + another profile whose explicit label clashes with the first's name.
// validate() must reject this: "work" (implicit label for profiles.work) == "work" (explicit label for profiles.other).
const CONFIG_LABEL_NONE_CLASH: &str = r#"
[endpoint]
base_url = "https://ai.example.com/v1"
api_format = "anthropic"

[profiles.work]
api_key = "sk-work"

[profiles.other]
label = "work"
api_key = "sk-other"
"#;

// Two profiles with the same label — should error at validate time.
const CONFIG_DUPLICATE_LABEL: &str = r#"
[endpoint]
base_url = "https://ai.example.com/v1"
api_format = "anthropic"

[profiles.work]
label = "Shared label"
api_key = "sk-work"

[profiles.work2]
label = "Shared label"
api_key = "sk-work2"
"#;

// --- profiles text output ---

#[test]
fn profiles_text_shows_name_and_label_sorted_by_name() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .arg("profiles")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    // fast sorts before work alphabetically; both show "name  (label)" format
    assert_eq!(s, "fast  (Fast model)\nwork  (Work account)\n", "got: {s}");
}

#[test]
fn profiles_text_uses_name_when_no_label() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_NO_LABELS).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .arg("profiles")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    // When name == effective label, just the name is shown (no parenthetical)
    assert_eq!(s, "alpha\nbeta\n", "got: {s}");
}

// --- profiles --json ---

#[test]
fn profiles_json_is_valid_json_array_sorted_by_name() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["profiles", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value =
        serde_json::from_slice(&out).expect("output must be valid JSON");
    let arr = parsed.as_array().expect("must be an array");
    assert_eq!(arr.len(), 2);
    // sorted by name: fast < work
    assert_eq!(arr[0]["name"], "fast");
    assert_eq!(arr[0]["label"], "Fast model");
    assert_eq!(arr[1]["name"], "work");
    assert_eq!(arr[1]["label"], "Work account");
}

#[test]
fn profiles_json_contains_no_secrets() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["profiles", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(!s.contains("sk-"), "output must not contain secrets: {s}");
    assert!(
        !s.contains("api_key"),
        "output must not contain api_key field: {s}"
    );
}

#[test]
fn profiles_json_label_falls_back_to_name() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_NO_LABELS).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["profiles", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr[0]["name"], "alpha");
    assert_eq!(arr[0]["label"], "alpha");
}

// --- duplicate label validation ---

#[test]
fn duplicate_label_causes_failure_for_profiles_command() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DUPLICATE_LABEL).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .arg("profiles")
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("Shared label"), "got: {s}");
}

#[test]
fn duplicate_label_causes_failure_for_env_command() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_DUPLICATE_LABEL).unwrap();

    cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "work"])
        .assert()
        .failure();
}

// --- label=None vs explicit label clash ---

#[test]
fn label_none_vs_explicit_clash_is_rejected() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG_LABEL_NONE_CLASH).unwrap();

    let err = cmd()
        .env("AIX_CONFIG", file.path())
        .arg("profiles")
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&err).unwrap();
    assert!(
        s.contains("work"),
        "error must mention the duplicate label 'work': {s}"
    );
}

// --- unknown profile error includes available names ---

#[test]
fn unknown_profile_error_lists_available_profiles() {
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let err = cmd()
        .env("AIX_CONFIG", file.path())
        .args(["env", "nonexistent-profile-xyz"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&err).unwrap();
    assert!(
        s.contains("nonexistent-profile-xyz"),
        "error must mention the requested profile: {s}"
    );
    // CONFIG has profiles "fast" and "work" — both must appear in the error
    assert!(s.contains("fast"), "error must list profile 'fast': {s}");
    assert!(s.contains("work"), "error must list profile 'work': {s}");
}

// --- env non-interactive error (no profile, no default, piped stdin) ---

#[test]
fn env_non_interactive_no_profile_errors_clearly() {
    // CONFIG has no default_profile. assert_cmd pipes stdin so is_terminal() → false.
    let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
    file.write_str(CONFIG).unwrap();

    let out = cmd()
        .env("AIX_CONFIG", file.path())
        .env_remove("AIX_PROFILE")
        .arg("env") // no positional profile
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("profile"), "error must mention 'profile': {s}");
}
