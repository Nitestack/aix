use crate::commands::launch;
use crate::commands::ProfileSelection;
use color_eyre::Result;
use std::path::PathBuf;

pub fn run(selection: ProfileSelection, config_path: Option<PathBuf>, dry_run: bool) -> Result<()> {
    let env = launch::resolve_launch_env(selection, config_path, None)?;
    let shell = launch::detect_shell();
    launch::run_command(&shell, &[], &env, dry_run)
}
