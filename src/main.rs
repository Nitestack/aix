use clap::Parser;
use cli::{Cli, Command};
use config::ApiFormat;

mod cli;
mod client;
mod commands;
mod config;
mod error;
mod secrets;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    run(cli).await
}

async fn run(cli: Cli) -> color_eyre::Result<()> {
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
        Command::Config { action } => commands::config::run(action, config_path),
        Command::Spend { profile, json } => {
            let effective_profile = profile.or(global_profile);
            commands::spend::run(effective_profile, config_path, json).await
        }
        Command::Tool(raw) => {
            let tool = raw[0].clone();
            let rest = &raw[1..];

            let sep = rest.iter().position(|a| a == "--");
            let (pre, tool_args) = match sep {
                Some(i) => (&rest[..i], rest[i + 1..].to_vec()),
                None => (rest, vec![]),
            };

            let dry_run = pre.iter().any(|a| a == "--dry-run");
            let profile = pre.iter().find(|a| !a.starts_with('-')).cloned();

            let format = if tool == "claude" {
                ApiFormat::Anthropic
            } else {
                ApiFormat::OpenAi
            };

            commands::launch::run_named_tool(
                &tool,
                format,
                profile.or(global_profile),
                config_path,
                dry_run,
                tool_args,
            )
        }
    }
}
