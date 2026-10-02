use crate::commands::launch;
use crate::commands::ProfileSelection;
use crate::error::AixError;
use color_eyre::Result;
use std::path::PathBuf;

pub fn run(
    selection: ProfileSelection,
    config_path: Option<PathBuf>,
    dry_run: bool,
    args: Vec<String>,
) -> Result<()> {
    let (program, cmd_args) = args
        .split_first()
        .map(|(p, rest)| (p.clone(), rest.to_vec()))
        .ok_or(AixError::ExecNoCommand)?;

    let env = launch::resolve_launch_env(selection, config_path, None)?;
    launch::run_command(&program, &cmd_args, &env, dry_run)
}
