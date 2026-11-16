use std::io::IsTerminal;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

// Herdr expires the label after an unclean exit (including SIGKILL). Refresh
// while the launch is active so even long-running agents retain their label.
const LABEL_TTL_MS: u64 = 120_000;
const LABEL_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

pub struct PaneLabel {
    pane: String,
    stop: Sender<()>,
    refresher: Option<JoinHandle<()>>,
}

impl Drop for PaneLabel {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(refresher) = self.refresher.take() {
            let _ = refresher.join();
        }
        let _ = metadata_command(&self.pane)
            .arg("--clear-display-agent")
            .status();
    }
}

/// Shows `<harness> · <profile>` as the display name of the current herdr pane.
///
/// Only for interactive launches (stdout is a terminal): piped runs and tests
/// inherit `HERDR_PANE_ID` from their shell and must not relabel that pane.
/// Best effort: a no-op outside a herdr pane or without a `herdr` binary. Keep
/// the returned guard alive for the launch and drop it before `process::exit`.
pub fn label_pane(harness: &str, profile: &str) -> Option<PaneLabel> {
    if !std::io::stdout().is_terminal() {
        return None;
    }
    let pane = std::env::var("HERDR_PANE_ID").ok()?;
    let _ = report_command(&pane, harness, profile).status();

    let (stop, stop_rx) = mpsc::channel();
    let refresh_pane = pane.clone();
    let harness = harness.to_owned();
    let profile = profile.to_owned();
    let refresher = thread::spawn(move || {
        while let Err(mpsc::RecvTimeoutError::Timeout) =
            stop_rx.recv_timeout(LABEL_REFRESH_INTERVAL)
        {
            let _ = report_command(&refresh_pane, &harness, &profile).status();
        }
    });
    Some(PaneLabel {
        pane,
        stop,
        refresher: Some(refresher),
    })
}

fn report_command(pane: &str, harness: &str, profile: &str) -> Command {
    let mut command = metadata_command(pane);
    command
        .arg("--display-agent")
        .arg(format!("{harness} · {profile}"))
        .arg("--ttl-ms")
        .arg(LABEL_TTL_MS.to_string());
    command
}

fn metadata_command(pane: &str) -> Command {
    let mut command = Command::new("herdr");
    command
        .args(["pane", "report-metadata", pane, "--source", "aix"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_command_sets_harness_and_profile_as_display_name() {
        let command = report_command("w1:p1", "claude", "work");
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "pane",
                "report-metadata",
                "w1:p1",
                "--source",
                "aix",
                "--display-agent",
                "claude · work",
                "--ttl-ms",
                "120000",
            ]
        );
    }
}
