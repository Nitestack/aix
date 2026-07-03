use clap::Parser;
use cli::{Cli, Command};

mod cli;
mod commands;
mod config;
mod error;
mod secrets;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    run(cli)
}

fn run(cli: Cli) -> color_eyre::Result<()> {
    let global_profile = cli.profile;
    let config_path = cli.config;
    match cli.command {
        Command::Profiles { json } => commands::profiles::run(config_path, json),
        Command::Env { profile, format } => {
            let effective_profile = profile.or(global_profile);
            commands::env::run(effective_profile, config_path, format)
        }
        Command::Shell {
            profile,
            dry_run,
            extra_args,
        } => {
            if !extra_args.is_empty() {
                return Err(color_eyre::eyre::eyre!(
                    "`aix shell` does not accept commands after --; use `aix exec` to run a command directly"
                ));
            }
            let effective_profile = profile.or(global_profile);
            commands::shell::run(effective_profile, config_path, dry_run)
        }
        Command::Exec {
            profile,
            dry_run,
            args,
        } => {
            let effective_profile = profile.or(global_profile);
            commands::exec::run(effective_profile, config_path, dry_run, args)
        }
        Command::Claude {
            profile,
            dry_run,
            args,
        } => {
            let effective_profile = profile.or(global_profile);
            commands::claude::run(effective_profile, config_path, dry_run, args)
        }
        Command::Pi {
            profile,
            dry_run,
            args,
        } => {
            let effective_profile = profile.or(global_profile);
            commands::pi::run(effective_profile, config_path, dry_run, args)
        }
        Command::Config { action } => commands::config::run(action, config_path),
    }
}
