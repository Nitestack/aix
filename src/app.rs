use crate::cli::{Cli, Command};
use clap::Parser;
use std::process::ExitCode;

pub async fn run() -> ExitCode {
    if let Err(error) = color_eyre::config::HookBuilder::default()
        .display_env_section(false)
        .install()
    {
        eprintln!("{error:?}");
        return ExitCode::from(1);
    }

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return ExitCode::from(code as u8);
        }
    };

    match dispatch(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let code = crate::error::exit_code(&error);
            eprintln!("{error:?}");
            ExitCode::from(code as u8)
        }
    }
}

async fn dispatch(cli: Cli) -> color_eyre::Result<()> {
    if cli.json && !cli.command.supports_json() {
        return Err(crate::error::AixError::JsonUnsupportedCommand.into());
    }

    let json = cli.json;
    let global_profile = cli.profile;
    let config_path = cli.config;
    match cli.command {
        Command::Init { shell } => crate::commands::init::run(shell),
        Command::Use {
            profile,
            clear,
            shell,
            format,
        } => crate::commands::use_profile::run(profile, clear, shell, format, config_path),
        Command::Current { format } => crate::commands::current::run(config_path, json, format),
        Command::Profiles => crate::commands::profiles::run(config_path, json),
        Command::Env { profile, format } => {
            let effective_profile = profile.or(global_profile);
            crate::commands::env::run(effective_profile, config_path, format)
        }
        Command::Shell {
            profile,
            dry_run,
            extra_args,
        } => {
            if !extra_args.is_empty() {
                return Err(crate::error::AixError::ShellExtraArgs.into());
            }
            let effective_profile = profile.or(global_profile);
            crate::commands::shell::run(effective_profile, config_path, dry_run)
        }
        Command::Exec {
            profile,
            dry_run,
            args,
        } => {
            let effective_profile = profile.or(global_profile);
            crate::commands::exec::run(effective_profile, config_path, dry_run, args)
        }
        Command::Config { action } => crate::commands::config::run(action, config_path),
        Command::Spend { profile, no_cache } => {
            let effective_profile = profile.or(global_profile);
            crate::commands::spend::run(effective_profile, config_path, json, no_cache).await
        }
        Command::Status { profile, refresh } => {
            let effective_profile = profile.or(global_profile);
            crate::commands::status::run(effective_profile, config_path, json, refresh).await
        }
        Command::Doctor { profile } => {
            let effective_profile = profile.or(global_profile);
            crate::commands::doctor::run(effective_profile, config_path, json).await
        }
        Command::Models { profile, filter } => {
            let effective_profile = profile.or(global_profile);
            crate::commands::models::run(effective_profile, config_path, json, filter).await
        }
        Command::Usage {
            profile,
            date_range,
            model,
        } => {
            let effective_profile = profile.or(global_profile);
            crate::commands::usage::run(effective_profile, config_path, json, date_range, model)
                .await
        }
        Command::Ask {
            model,
            system,
            files,
            prompt,
        } => {
            crate::commands::ask::run(
                global_profile,
                config_path,
                json,
                model,
                system,
                files,
                prompt,
            )
            .await
        }
        Command::Prompt {
            name,
            list,
            model,
            files,
        } => {
            crate::commands::prompt::run(
                global_profile,
                config_path,
                json,
                name,
                list,
                model,
                files,
            )
            .await
        }
        Command::Run {
            name,
            workflow,
            task_id,
            tags,
            lease,
            budget,
            duration,
            allow_models,
            dry_run,
            args,
        } => {
            let effective_profile = global_profile;
            crate::commands::run::run(crate::commands::run::RunOptions {
                profile: effective_profile,
                config_path,
                metadata: crate::commands::run::RunMetadata {
                    name,
                    workflow,
                    task_id,
                    tags,
                },
                lease,
                budget,
                duration,
                requested_models: allow_models,
                dry_run,
                args,
            })
            .await
        }
        Command::Runs { limit, action } => crate::commands::runs::run(action, limit, json),
        Command::Cache { action } => crate::commands::cache::run(action, config_path),
        Command::Tool(raw) => {
            let tool = raw[0].clone();
            let rest = &raw[1..];

            let sep = rest.iter().position(|arg| arg == "--");
            let (pre, tool_args) = match sep {
                Some(index) => (&rest[..index], rest[index + 1..].to_vec()),
                None => (rest, vec![]),
            };

            let dry_run = pre.iter().any(|arg| arg == "--dry-run");
            let profile = pre.iter().find(|arg| !arg.starts_with('-')).cloned();

            crate::commands::launch::run_named_tool(
                &tool,
                profile.or(global_profile),
                config_path,
                dry_run,
                tool_args,
            )
        }
    }
}
