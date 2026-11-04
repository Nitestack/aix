#![cfg(unix)]

use nix::pty::{openpty, OpenptyResult};
use std::fs::{self, File, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Pane {
    dir: assert_fs::TempDir,
    terminal: OpenptyResult,
}

impl Pane {
    fn new(local_gateway: bool) -> Self {
        let pane = Self {
            dir: assert_fs::TempDir::new().unwrap(),
            terminal: openpty(None, None).unwrap(),
        };
        fs::create_dir(pane.path("bin")).unwrap();
        pane.write_executable(
            "herdr",
            r#"#!/bin/sh
test "$1" = pane && test "$2" = report-metadata && test "$3" = w1:p1 &&
    test "$4" = --source && test "$5" = aix || exit 2
case "$6" in
    --display-agent)
        printf '%s\n' "$7" > "$AIX_TEST_HERDR_LABEL"
        printf 'set:%s\n' "$7" >> "$AIX_TEST_HERDR_REPORTS"
        ;;
    --clear-display-agent)
        : > "$AIX_TEST_HERDR_LABEL"
        printf 'clear\n' >> "$AIX_TEST_HERDR_REPORTS"
        ;;
    *) exit 2 ;;
esac
"#,
        );
        pane.write_executable(
            "test-agent",
            r#"#!/bin/sh
label=
if test -f "$AIX_TEST_HERDR_LABEL"; then
    IFS= read -r label < "$AIX_TEST_HERDR_LABEL"
fi
printf '%s\n' "${label:-$1}" >> "$AIX_TEST_AGENT_LABELS"
if test "$2" = interrupt; then
    kill "-$3" "$PPID"
    exec sleep 1 >/dev/null 2>&1
fi
exit "${2:-0}"
"#,
        );
        fs::write(
            pane.path("aix.toml"),
            format!(
                r#"
[endpoint]
base_url = "https://gateway.invalid"

[profiles.meta]
api_key = "test-key"

[tools.claude]
command = "test-agent"
api_format = "anthropic"
local_gateway = {local_gateway}

[tools.codex]
command = "test-agent"
api_format = "openai"
local_gateway = {local_gateway}
"#
            ),
        )
        .unwrap();
        pane
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn write_executable(&self, name: &str, script: &str) {
        let path = self.path("bin").join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(path, Permissions::from_mode(0o700)).unwrap();
    }

    fn command(&self, program: &str) -> Command {
        let inherited_path = std::env::var_os("PATH").unwrap_or_default();
        let paths = std::iter::once(self.path("bin")).chain(std::env::split_paths(&inherited_path));
        let mut command = Command::new(program);
        command
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("AIX_CONFIG", self.path("aix.toml"))
            .env("AIX_STATE_DIR", self.path("state"))
            .env("AIX_AUTH_DIR", self.path("auth"))
            .env("HERDR_PANE_ID", "w1:p1")
            .env("AIX_TEST_HERDR_LABEL", self.path("label"))
            .env("AIX_TEST_HERDR_REPORTS", self.path("reports"))
            .env("AIX_TEST_AGENT_LABELS", self.path("agent-labels"))
            .stdin(Stdio::null())
            .stdout(File::from(self.terminal.slave.try_clone().unwrap()))
            .stderr(Stdio::piped());
        command
    }

    fn launch(&self, tool: &str, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_aix"))
            .args(["--profile", "meta", tool, "--"])
            .args(args)
            .output()
            .unwrap()
    }

    fn assert_cleared(&self, tool: &str) {
        assert_eq!(
            fs::read_to_string(self.path("label")).unwrap(),
            "",
            "the label must not outlive its tool"
        );
        assert_eq!(
            fs::read_to_string(self.path("reports")).unwrap(),
            format!("set:{tool} · meta\nclear\n")
        );
    }
}

#[test]
fn sequential_agents_in_the_same_pane_do_not_inherit_the_previous_label() {
    for local_gateway in [false, true] {
        let pane = Pane::new(local_gateway);
        let output = pane.launch("claude", &["claude"]);
        assert!(output.status.success(), "{output:?}");

        // Start the next agent directly, without aix to replace the old metadata.
        let output = pane
            .command(pane.path("bin/test-agent").to_str().unwrap())
            .arg("codex")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            fs::read_to_string(pane.path("agent-labels")).unwrap(),
            "claude · meta\ncodex\n"
        );
        pane.assert_cleared("claude");
    }
}

#[test]
fn failed_child_start_clears_the_pane_label() {
    for local_gateway in [false, true] {
        let pane = Pane::new(local_gateway);
        // Pass executable validation, then fail spawn because the interpreter is missing.
        pane.write_executable("test-agent", "#!/aix-test-missing-interpreter\n");
        let output = pane.launch("claude", &[]);
        assert!(!output.status.success(), "{output:?}");
        assert!(!pane.path("agent-labels").exists());
        pane.assert_cleared("claude");
    }
}

#[test]
fn failed_pi_config_preparation_clears_the_pane_label() {
    for local_gateway in [false, true] {
        let pane = Pane::new(local_gateway);
        fs::create_dir(pane.path("pi-source")).unwrap();
        fs::write(pane.path("pi-data"), "blocks Pi data directory creation").unwrap();
        let config = fs::read_to_string(pane.path("aix.toml")).unwrap();
        fs::write(
            pane.path("aix.toml"),
            format!(
                r#"{config}
[profiles.meta.tool_configs.pi]
agent_dir = "pi-source"

[tools.pi]
command = "test-agent"
api_format = "openai"
local_gateway = {local_gateway}
"#
            ),
        )
        .unwrap();
        let output = pane
            .command(env!("CARGO_BIN_EXE_aix"))
            .env("PI_CODING_AGENT_DIR", pane.path("pi-data"))
            .args(["--profile", "meta", "pi"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("prepare shared Pi user data directories"),
            "{output:?}"
        );
        assert!(!pane.path("agent-labels").exists());
        pane.assert_cleared("pi");
    }
}

#[test]
fn nonzero_child_exit_clears_the_pane_label_and_preserves_the_exit_code() {
    for local_gateway in [false, true] {
        let pane = Pane::new(local_gateway);
        let output = pane.launch("claude", &["claude", "42"]);
        assert_eq!(output.status.code(), Some(42), "{output:?}");
        pane.assert_cleared("claude");
    }
}

#[test]
fn interrupted_tool_clears_the_pane_label() {
    for local_gateway in [false, true] {
        for (signal, exit_code) in [("INT", 130), ("TERM", 143)] {
            let pane = Pane::new(local_gateway);
            let output = pane.launch("claude", &["claude", "interrupt", signal]);
            assert_eq!(output.status.code(), Some(exit_code), "{output:?}");
            pane.assert_cleared("claude");
        }
    }
}
