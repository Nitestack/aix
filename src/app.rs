use crate::cli::{Cli, Command};
use crate::commands::spend::report::CachePolicy;
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
            if code != 0 && json_errors_requested(std::env::args().skip(1)) {
                let message = error.render().to_string().trim_end().to_string();
                crate::output::print_json_usage_error(code, message);
            } else {
                let _ = error.print();
            }
            return ExitCode::from(code as u8);
        }
    };

    let json_error_command =
        (cli.error_format == crate::cli::ErrorFormat::Json).then(|| cli.command.json_name());
    match dispatch(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let code = crate::error::exit_code(&error);
            // Doctor and gate have already written their failed report to stderr.
            if matches!(
                error.downcast_ref::<crate::error::AixError>(),
                Some(
                    crate::error::AixError::DoctorChecksFailed { .. }
                        | crate::error::AixError::GateChecksFailed { .. }
                )
            ) {
            } else if let Some(command) = json_error_command {
                crate::output::print_json_error(command, &error);
            } else {
                eprintln!("{error:?}");
            }
            ExitCode::from(code as u8)
        }
    }
}

async fn dispatch(cli: Cli) -> color_eyre::Result<()> {
    if let Command::Tool(raw) = &cli.command {
        let command = match raw.first().map(String::as_str) {
            Some("lease") => Some("lease"),
            Some("leases") => Some("leases"),
            _ => None,
        };
        if let Some(command) = command {
            return Err(crate::error::AixError::RemovedCommand { command }.into());
        }
    }
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
        Command::Spend {
            profile,
            no_cache,
            max_age,
        } => {
            crate::commands::spend::run(
                request_options_for(profile),
                config_path,
                json,
                CachePolicy::from_flags(no_cache, max_age),
            )
            .await
        }
        Command::Status {
            profile,
            refresh,
            max_age,
        } => {
            crate::commands::status::run(
                request_options_for(profile),
                config_path,
                json,
                CachePolicy::from_flags(refresh, max_age),
            )
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
            protocol,
            tool,
            run_id,
        } => {
            crate::commands::usage::run(crate::commands::usage::UsageOptions {
                request: request_options_for(profile),
                config_path,
                json,
                date_range,
                model_filter: model,
                protocol_filter: protocol,
                tool_filter: tool,
                run_filter: run_id,
            })
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
                duration,
                requested_models: allow_models,
                dry_run,
                args,
            })
            .await
        }
        Command::Runs { limit, action } => crate::commands::runs::run(action, limit, json),
        Command::Cache { action } => crate::commands::cache::run(action, config_path),
        Command::Upgrade { check, force } => {
            crate::commands::upgrade::run(check, force, timeout).await
        }
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

/// Whether `--error-format json` appears before any `--` separator. Used only
/// when clap could not parse the command line.
fn json_errors_requested(args: impl IntoIterator<Item = String>) -> bool {
    let mut previous_was_flag = false;
    for arg in args {
        if arg == "--" {
            return false;
        }
        if arg == "--error-format=json" || (previous_was_flag && arg == "json") {
            return true;
        }
        previous_was_flag = arg == "--error-format";
    }
    false
}

fn tool_non_interactive(global_flag: bool, pre_separator_args: &[String]) -> bool {
    global_flag
        || pre_separator_args
            .iter()
            .any(|arg| arg == "--non-interactive")
}

#[cfg(test)]
mod tests {
    use super::{json_errors_requested, tool_non_interactive};

    #[test]
    fn json_error_format_is_detected_without_a_successful_parse() {
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        assert!(json_errors_requested(args(&[
            "--error-format",
            "json",
            "x"
        ])));
        assert!(json_errors_requested(args(&["x", "--error-format=json"])));
        assert!(!json_errors_requested(args(&["--error-format", "text"])));
        assert!(!json_errors_requested(args(&[
            "exec",
            "--",
            "--error-format=json"
        ])));
        assert!(!json_errors_requested(args(&["json"])));
    }

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
