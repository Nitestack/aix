use crate::commands::launch;
use color_eyre::Result;
use std::path::PathBuf;

pub fn run(profile: Option<String>, config_path: Option<PathBuf>, dry_run: bool) -> Result<()> {
    let env = launch::resolve_launch_env(profile, config_path, None)?;
    let shell = launch::detect_shell();
    launch::run_command(&shell, &[], &env, dry_run)
}
