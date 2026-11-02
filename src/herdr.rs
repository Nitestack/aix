use std::io::IsTerminal;
use std::process::{Command, Stdio};

/// Shows `<harness> · <profile>` as the display name of the current herdr pane.
///
/// Only for interactive launches (stdout is a terminal): piped runs and tests
/// inherit `HERDR_PANE_ID` from their shell and must not relabel that pane.
/// Best effort: a no-op outside a herdr pane or without a `herdr` binary. herdr
/// drops the display name again when the agent exits, so nothing is cleared here.
pub fn label_pane(harness: &str, profile: &str) {
    if !std::io::stdout().is_terminal() {
        return;
    }
    let Ok(pane) = std::env::var("HERDR_PANE_ID") else {
        return;
    };
    let _ = report_command(&pane, harness, profile).status();
}

fn report_command(pane: &str, harness: &str, profile: &str) -> Command {
    let mut command = Command::new("herdr");
    command
        .args([
            "pane",
            "report-metadata",
            pane,
            "--source",
            "aix",
            "--display-agent",
        ])
        .arg(format!("{harness} · {profile}"))
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
            ]
        );
    }
}
