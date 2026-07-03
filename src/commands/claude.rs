use crate::commands::launch;
use color_eyre::Result;
use std::path::PathBuf;

pub fn run(
    profile: Option<String>,
    config_path: Option<PathBuf>,
    dry_run: bool,
    args: Vec<String>,
) -> Result<()> {
    let env = launch::resolve_launch_env(profile, config_path)?;
    launch::run_command("claude", &args, &env, dry_run)
}
