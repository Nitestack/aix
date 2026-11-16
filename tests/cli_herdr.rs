#![cfg(unix)]

use nix::libc;
use nix::pty::{openpty, OpenptyResult};
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::fs::{self, File, Permissions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

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
        test "$8" = --ttl-ms && test "$9" = 120000 || exit 2
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
if test "$2" = ctrl-c; then
    test "$3" = trap && trap ': > "$AIX_TEST_DIR/caught"' INT
    : > "$AIX_TEST_DIR/ready"
    while ! test -f "$AIX_TEST_DIR/release"; do sleep 0.05; done
    exit 0
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
            .env("AIX_TEST_DIR", self.dir.path())
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

    fn wait_for(&self, name: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.path(name).exists() {
            assert!(Instant::now() < deadline, "timed out waiting for {name}");
            std::thread::sleep(Duration::from_millis(20));
        }
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
    // A plain launch leaves SIGINT to the child; see the terminal Ctrl+C test.
    for (local_gateway, signal, exit_code) in [
        (false, "TERM", 143),
        (true, "INT", 130),
        (true, "TERM", 143),
    ] {
        let pane = Pane::new(local_gateway);
        let output = pane.launch("claude", &["claude", "interrupt", signal]);
        assert_eq!(output.status.code(), Some(exit_code), "{output:?}");
        pane.assert_cleared("claude");
    }
}

fn wait_for_exit(mut child: std::process::Child) -> Output {
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("timed out waiting for aix to exit");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn process_directed_sigint_reaches_plain_launch_child() {
    let pane = Pane::new(false);
    let aix = pane
        .command(env!("CARGO_BIN_EXE_aix"))
        .args([
            "--profile",
            "meta",
            "claude",
            "--",
            "claude",
            "ctrl-c",
            "default",
        ])
        .spawn()
        .unwrap();
    pane.wait_for("ready");

    // Only aix receives the signal. It must forward it to the child.
    kill(Pid::from_raw(aix.id() as i32), Signal::SIGINT).unwrap();
    let output = wait_for_exit(aix);
    assert_eq!(output.status.code(), Some(130), "{output:?}");
    pane.assert_cleared("claude");
}

#[test]
fn terminal_ctrl_c_is_left_to_a_plain_launch_child() {
    for (child_handles_ctrl_c, exit_code) in [(true, 0), (false, 130)] {
        let pane = Pane::new(false);
        let mode = if child_handles_ctrl_c {
            "trap"
        } else {
            "default"
        };
        let mut command = pane.command(env!("CARGO_BIN_EXE_aix"));
        command
            .args([
                "--profile",
                "meta",
                "claude",
                "--",
                "claude",
                "ctrl-c",
                mode,
            ])
            .stdin(File::from(pane.terminal.slave.try_clone().unwrap()));

        // Make the PTY a controlling terminal with aix's process group in the
        // foreground. A Ctrl+C byte then generates a kernel-origin SIGINT for
        // both aix and its child, rather than a process-origin kill(-pgid).
        // SAFETY: these libc calls are async-signal-safe in the pre-exec child.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::tcsetpgrp(libc::STDIN_FILENO, libc::getpgrp()) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let aix = command.spawn().unwrap();
        pane.wait_for("ready");

        let mut master = File::from(pane.terminal.master.try_clone().unwrap());
        master.write_all(b"\x03").unwrap();
        if child_handles_ctrl_c {
            pane.wait_for("caught");
            assert_eq!(
                fs::read_to_string(pane.path("label")).unwrap(),
                "claude · meta\n",
                "the label must stay while the tool keeps running"
            );
            File::create(pane.path("release")).unwrap();
        }

        let output = wait_for_exit(aix);
        assert_eq!(output.status.code(), Some(exit_code), "{output:?}");
        pane.assert_cleared("claude");
    }
}
