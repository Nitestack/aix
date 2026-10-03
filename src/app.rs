use crate::cli::{Cli, Command};
use crate::commands::{GatewayRequestOptions, ProfileSelection};
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
            if !matches!(
                error.downcast_ref::<crate::error::AixError>(),
                Some(
                    crate::error::AixError::DoctorChecksFailed { .. }
                        | crate::error::AixError::GateChecksFailed { .. }
                )
            ) {
                eprintln!("{error:?}");
            }
            ExitCode::from(code as u8)
        }
    }
}

async fn dispatch(cli: Cli) -> color_eyre::Result<()> {
    if cli.json && !cli.command.supports_json() {
        return Err(crate::error::AixError::JsonUnsupportedCommand.into());
    }

    let json = cli.json;
    let explicit_global_profile = cli.profile;
    let global_profile = explicit_global_profile
        .clone()
        .or_else(|| std::env::var("AIX_PROFILE").ok());
    let non_interactive = cli.non_interactive;
    let timeout = cli.timeout;
    let config_path = cli.config;
    let selection_for = |profile: Option<String>, selection_is_non_interactive| ProfileSelection {
        profile: profile.or_else(|| global_profile.clone()),
        non_interactive: selection_is_non_interactive,
    };
    let request_options_for = |profile| GatewayRequestOptions {
        selection: selection_for(profile, non_interactive),
        timeout,
    };
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
        Command::Policies => crate::commands::policies::list(config_path, json),
        Command::Policy { action } => match action {
            crate::cli::PolicyAction::Show { name } => {
                crate::commands::policies::show(config_path, json, &name)
            }
        },
        Command::Env { profile, format } => {
            crate::commands::env::run(selection_for(profile, non_interactive), config_path, format)
        }
        Command::Shell {
            profile,
            dry_run,
            extra_args,
        } => {
            if !extra_args.is_empty() {
                return Err(crate::error::AixError::ShellExtraArgs.into());
            }
            crate::commands::shell::run(
                selection_for(profile, non_interactive),
                config_path,
                dry_run,
            )
        }
        Command::Exec {
            profile,
            dry_run,
            args,
        } => crate::commands::exec::run(
            selection_for(profile, non_interactive),
            config_path,
            dry_run,
            args,
        ),
        Command::Config { action } => crate::commands::config::run(action, config_path),
        Command::Auth { action } => {
            crate::commands::auth::run(
                action,
                selection_for(None, non_interactive),
                config_path,
                json,
                timeout,
            )
            .await
        }
        Command::Spend { profile, no_cache } => {
            crate::commands::spend::run(request_options_for(profile), config_path, json, no_cache)
                .await
        }
        Command::Status { profile, refresh } => {
            crate::commands::status::run(request_options_for(profile), config_path, json, refresh)
                .await
        }
        Command::Gate { policy } => {
            crate::commands::gate::run(crate::commands::gate::GateOptions {
                selection: selection_for(None, true),
                explicit_profile: explicit_global_profile,
                timeout,
                config_path,
                policy,
                json,
            })
            .await
        }
        Command::Doctor { profile } => {
            crate::commands::doctor::run(request_options_for(profile), config_path, json).await
        }
        Command::Models { profile, filter } => {
            crate::commands::models::run(request_options_for(profile), config_path, json, filter)
                .await
        }
        Command::Usage {
            profile,
            date_range,
            model,
        } => {
            crate::commands::usage::run(
                request_options_for(profile),
                config_path,
                json,
                date_range,
                model,
            )
            .await
        }
        Command::Ask {
            model,
            system,
            files,
            prompt,
        } => {
            crate::commands::ask::run(
                request_options_for(None),
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
                request_options_for(None),
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
            policy,
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
            crate::commands::run::run(crate::commands::run::RunOptions {
                selection: selection_for(None, non_interactive),
                timeout,
                explicit_profile: explicit_global_profile.clone(),
                config_path,
                metadata: crate::commands::run::RunMetadata {
                    name,
                    workflow,
                    task_id,
                    tags,
                },
                policy,
                lease,
                budget,
                duration,
                requested_models: allow_models,
                dry_run,
                args,
            })
            .await
        }
        Command::Lease { action } => match action {
            crate::cli::LeaseAction::Create {
                budget,
                duration,
                allow_models,
                tags,
                output,
            } => {
                crate::commands::leases::create(crate::commands::leases::CreateOptions {
                    selection: selection_for(None, non_interactive),
                    timeout,
                    config_path,
                    budget,
                    duration,
                    requested_models: allow_models,
                    tags,
                    output,
                })
                .await
            }
            crate::cli::LeaseAction::Show { lease_id } => {
                crate::commands::leases::show(&lease_id, json)
            }
            crate::cli::LeaseAction::Revoke { lease_id } => {
                crate::commands::leases::revoke(&lease_id, config_path, timeout).await
            }
        },
        Command::Leases => crate::commands::leases::list(json),
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
            let tool_non_interactive = tool_non_interactive(non_interactive, pre);

            crate::commands::launch::run_named_tool(
                &tool,
                selection_for(profile, tool_non_interactive),
                config_path,
                dry_run,
                tool_args,
                timeout,
            )
            .await
        }
    }
}

fn tool_non_interactive(global_flag: bool, pre_separator_args: &[String]) -> bool {
    global_flag
        || pre_separator_args
            .iter()
            .any(|arg| arg == "--non-interactive")
}

#[cfg(test)]
mod tests {
    use super::tool_non_interactive;

    #[test]
    fn named_tool_selection_honors_non_interactive_before_separator() {
        let rest = [
            "--non-interactive".to_string(),
            "work".to_string(),
            "--".to_string(),
            "--non-interactive".to_string(),
        ];
        let separator = rest.iter().position(|arg| arg == "--").unwrap();

        assert!(tool_non_interactive(false, &rest[..separator]));
        assert!(!tool_non_interactive(false, &[]));
        assert!(tool_non_interactive(true, &[]));
    }
}
