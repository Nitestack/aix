use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn help_no_longer_advertises_key_management_or_upstream_usage() {
    Command::cargo_bin("aix")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("\n  lease").not());

    Command::cargo_bin("aix")
        .unwrap()
        .args(["run", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--lease").not())
        .stdout(predicate::str::contains("--budget").not());

    Command::cargo_bin("aix")
        .unwrap()
        .args(["usage", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--source").not())
        .stdout(predicate::str::contains("locally observed usage"));
}

#[test]
fn removed_key_commands_fail_before_config_or_external_tool_dispatch() {
    for args in [
        vec!["lease", "create", "--budget", "1", "--output", "unused"],
        vec!["lease", "revoke", "old-lease"],
        vec!["lease", "show", "old-lease"],
        vec!["leases"],
    ] {
        for json in [false, true] {
            let mut command = Command::cargo_bin("aix").unwrap();
            command.args(["--config", "missing-removed-feature-config.toml"]);
            if json {
                command.arg("--json");
            }
            command
                .args(&args)
                .assert()
                .code(2)
                .stdout(predicate::str::is_empty())
                .stderr(predicate::str::contains("has been removed"))
                .stderr(predicate::str::contains("provider's portal"));
        }
    }
}

#[test]
fn removed_run_flags_are_rejected_before_launch() {
    for args in [vec!["--lease"], vec!["--budget", "1"]] {
        Command::cargo_bin("aix")
            .unwrap()
            .arg("run")
            .args(args)
            .args(["--", "must-not-launch"])
            .assert()
            .code(2)
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains("unexpected argument"));
    }
}

#[test]
fn usage_no_longer_accepts_upstream_source_selection() {
    for source in ["auto", "local", "litellm", "all"] {
        Command::cargo_bin("aix")
            .unwrap()
            .args(["usage", "--source", source])
            .assert()
            .code(2)
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains("unexpected argument '--source'"));
    }
}

#[test]
fn remaining_run_constraint_overrides_require_a_policy() {
    for args in [
        vec!["--duration", "1m"],
        vec!["--allow-model", "model-test"],
        vec!["--dry-run"],
    ] {
        Command::cargo_bin("aix")
            .unwrap()
            .arg("run")
            .args(args)
            .args(["--", "must-not-launch"])
            .assert()
            .code(2)
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains("--policy"));
    }
}
