use crate::commands::env::{collect_profile_vars, resolve_profile};
use crate::config;
use crate::error::AixError;
use color_eyre::Result;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub struct LaunchEnv {
    pub vars: Vec<(String, String)>,
    pub clear_vars: Vec<String>,
    pub remove_vars: Vec<String>,
    pub profile_name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interruption {
    CtrlC,
    Termination,
}

impl Interruption {
    pub fn exit_code(self) -> i32 {
        match self {
            Self::CtrlC => 130,
            Self::Termination => 143,
        }
    }
}

pub struct ResolvedRunLaunch {
    pub env: LaunchEnv,
    pub program: String,
    pub args: Vec<String>,
    pub logical_tool_name: Option<String>,
}

pub fn resolve_launch_env(
    profile: Option<String>,
    config_path: Option<PathBuf>,
    format_override: Option<config::ApiFormat>,
) -> Result<LaunchEnv> {
    let (env, _, _, _) = resolve_launch_env_inner(profile, config_path, format_override, None)?;
    Ok(env)
}

pub fn resolve_run_launch(
    profile: Option<String>,
    config_path: Option<PathBuf>,
    program: &str,
) -> Result<ResolvedRunLaunch> {
    resolve_tool_launch(profile, config_path, program, None)
}

fn resolve_tool_launch(
    profile: Option<String>,
    config_path: Option<PathBuf>,
    tool_name: &str,
    fallback_format: Option<config::ApiFormat>,
) -> Result<ResolvedRunLaunch> {
    let (env, program, logical_tool_name, args) =
        resolve_launch_env_inner(profile, config_path, fallback_format, Some(tool_name))?;
    Ok(ResolvedRunLaunch {
        env,
        program,
        logical_tool_name,
        args,
    })
}

fn resolve_launch_env_inner(
    profile: Option<String>,
    config_path: Option<PathBuf>,
    format_override: Option<config::ApiFormat>,
    configured_tool_name: Option<&str>,
) -> Result<(LaunchEnv, String, Option<String>, Vec<String>)> {
    let path = config::find_config_path(config_path.as_deref())?.ok_or(AixError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;
    config::load_env_files(&cfg)?;

    let profile_name = resolve_profile(profile, &cfg)?;
    let profile_entry =
        cfg.profiles
            .get(&profile_name)
            .ok_or_else(|| AixError::ProfileNotFound {
                name: profile_name.clone(),
                available_hint: config::format_available_profiles(&cfg),
            })?;

    let configured_tool = configured_tool_name.and_then(|name| {
        profile_entry
            .tools
            .get(name)
            .or_else(|| cfg.tools.get(name))
    });
    let api_format = configured_tool
        .and_then(|tool| tool.api_format)
        .or(format_override)
        .unwrap_or(config::ApiFormat::Both);
    let clear_vars = if profile_entry.auth == config::ProfileAuth::Native {
        native_clear_vars()
    } else if configured_tool.is_some() {
        match api_format {
            config::ApiFormat::Anthropic => {
                vec!["OPENAI_API_KEY".to_string(), "OPENAI_BASE_URL".to_string()]
            }
            config::ApiFormat::OpenAi => vec![
                "ANTHROPIC_API_KEY".to_string(),
                "ANTHROPIC_BASE_URL".to_string(),
            ],
            config::ApiFormat::Both => Vec::new(),
        }
    } else {
        Vec::new()
    };

    let mut vars = if profile_entry.auth == config::ProfileAuth::Native {
        let mut vars = vec![("AIX_PROFILE".to_string(), profile_name.clone())];
        crate::commands::env::append_custom_vars(&mut vars, &profile_entry.env)?;
        vars
    } else {
        let api_key = profile_entry.resolve_api_key()?;
        let base_url = config::resolve_base_url(profile_entry, &cfg.endpoint)?;
        collect_profile_vars(
            &profile_name,
            api_key.expose_secret(),
            base_url.expose_secret(),
            &api_format,
            profile_entry,
        )?
    };
    if let Some(tool) = configured_tool {
        let mut tool_vars: Vec<_> = tool.env.iter().collect();
        tool_vars.sort_unstable_by_key(|(key, _)| key.as_str());
        for (key, value) in tool_vars {
            vars.push((key.clone(), value.resolve()?.expose_secret().to_string()));
        }
    }

    let logical_tool_name = configured_tool.map(|_| configured_tool_name.unwrap().to_string());
    let effective_program = configured_tool
        .and_then(|tool| tool.command.as_deref())
        .or(configured_tool_name)
        .unwrap_or_default()
        .to_string();

    let args = configured_tool
        .map(|tool| resolve_tool_args(&tool.args, &cfg, profile_entry))
        .transpose()?
        .unwrap_or_default();

    Ok((
        LaunchEnv {
            vars,
            clear_vars,
            remove_vars: Vec::new(),
            profile_name,
        },
        effective_program,
        logical_tool_name,
        args,
    ))
}

/// Clear inherited gateway credentials before using the tool's own login.
pub(crate) fn native_clear_vars() -> Vec<String> {
    [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "LITELLM_API_KEY",
        "LITELLM_BASE_URL",
        "CODEX_API_KEY",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn resolve_tool_args(
    args: &[String],
    cfg: &config::Config,
    profile: &config::Profile,
) -> Result<Vec<String>, AixError> {
    let model = args
        .iter()
        .any(|arg| arg.contains("{model}"))
        .then(|| config::resolve_model(None, cfg, profile))
        .transpose()?;
    Ok(args
        .iter()
        .map(|arg| match &model {
            Some(model) => arg.replace("{model}", model),
            None => arg.clone(),
        })
        .collect())
}

pub fn run_named_tool(
    name: &str,
    profile: Option<String>,
    config_path: Option<PathBuf>,
    dry_run: bool,
    args: Vec<String>,
) -> Result<()> {
    let fallback_format = if name == "claude" {
        config::ApiFormat::Anthropic
    } else {
        config::ApiFormat::OpenAi
    };
    let resolved = resolve_tool_launch(profile, config_path, name, Some(fallback_format))?;
    let args = resolved.args.into_iter().chain(args).collect::<Vec<_>>();
    run_command(&resolved.program, &args, &resolved.env, dry_run)
}

pub fn detect_shell() -> String {
    detect_shell_impl()
}

pub fn run_command(program: &str, args: &[String], env: &LaunchEnv, dry_run: bool) -> Result<()> {
    if dry_run {
        let display_args: Vec<&str> = std::iter::once(program)
            .chain(args.iter().map(String::as_str))
            .collect();
        eprintln!("Would run: {}", display_args.join(" "));
        eprintln!("Would set:");
        for (k, _) in &env.vars {
            eprintln!("  {k}");
        }
        if !env.clear_vars.is_empty() {
            eprintln!("Would unset:");
            for key in &env.clear_vars {
                eprintln!("  {key}");
            }
        }
        return Ok(());
    }

    let status = run_command_status(program, args, env)?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

pub fn run_command_status(
    program: &str,
    args: &[String],
    env: &LaunchEnv,
) -> Result<std::process::ExitStatus, AixError> {
    let mut cmd = command_with_env(program, args, env)?;
    cmd.status().map_err(|source| AixError::ProcessSpawn {
        program: program.to_string(),
        source,
    })
}

pub fn run_command_status_interruptible(
    program: &str,
    args: &[String],
    env: &LaunchEnv,
    interrupt_requested: &AtomicBool,
    terminated: &AtomicBool,
) -> Result<std::process::ExitStatus, AixError> {
    let mut command = command_with_env(program, args, env)?;
    command
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    for key in &env.remove_vars {
        command.env_remove(key);
    }

    let mut child = command.spawn().map_err(|source| AixError::ProcessSpawn {
        program: program.to_string(),
        source,
    })?;
    loop {
        if let Some(status) = child.try_wait().map_err(|source| AixError::ProcessWait {
            program: program.to_string(),
            source,
        })? {
            return Ok(status);
        }

        if let Some(interruption) = interruption_reason(interrupt_requested, terminated) {
            return terminate_child(&mut child, program, interruption);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn terminate_child(
    child: &mut std::process::Child,
    program: &str,
    interruption: Interruption,
) -> Result<std::process::ExitStatus, AixError> {
    #[cfg(unix)]
    {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;

        let child_pid = Pid::from_raw(child.id() as i32);
        let signal = if interruption == Interruption::Termination {
            Signal::SIGTERM
        } else {
            Signal::SIGINT
        };
        let _ = kill(child_pid, signal);

        let grace_period = Instant::now() + Duration::from_millis(500);
        loop {
            if let Some(status) = child.try_wait().map_err(|source| AixError::ProcessWait {
                program: program.to_string(),
                source,
            })? {
                return Ok(status);
            }
            if Instant::now() >= grace_period {
                let _ = kill(child_pid, Signal::SIGKILL);
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = interruption;
        child.kill().map_err(|source| AixError::ProcessWait {
            program: program.to_string(),
            source,
        })?;
    }

    child.wait().map_err(|source| AixError::ProcessWait {
        program: program.to_string(),
        source,
    })
}

pub(crate) fn command_with_env(
    program: &str,
    args: &[String],
    env: &LaunchEnv,
) -> Result<std::process::Command, AixError> {
    let resolved = find_executable(program)?;
    let mut command = std::process::Command::new(resolved);
    command.args(args);
    for key in &env.clear_vars {
        command.env_remove(key);
    }
    for (key, value) in &env.vars {
        command.env(key, value);
    }
    Ok(command)
}

pub fn interruption_reason(
    interrupt_requested: &AtomicBool,
    terminated: &AtomicBool,
) -> Option<Interruption> {
    if terminated.load(Ordering::SeqCst) {
        Some(Interruption::Termination)
    } else if interrupt_requested.load(Ordering::SeqCst) {
        Some(Interruption::CtrlC)
    } else {
        None
    }
}

fn find_executable(program: &str) -> Result<PathBuf, AixError> {
    let path = Path::new(program);
    if path.is_absolute() {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        return Err(AixError::ExecutableNotFound {
            program: program.to_string(),
        });
    }
    which::which(program).map_err(|_| AixError::ExecutableNotFound {
        program: program.to_string(),
    })
}

#[cfg(unix)]
fn detect_shell_impl() -> String {
    if let Ok(shell) = std::env::var("SHELL") {
        if !shell.is_empty() {
            return shell;
        }
    }
    if std::env::var("NU_VERSION").is_ok() && which::which("nu").is_ok() {
        return "nu".to_string();
    }
    "sh".to_string()
}

#[cfg(windows)]
fn detect_shell_impl() -> String {
    if let Ok(shell) = std::env::var("SHELL") {
        if !shell.is_empty() {
            return shell;
        }
    }
    for candidate in &["pwsh", "powershell"] {
        if which::which(candidate).is_ok() {
            return candidate.to_string();
        }
    }
    "cmd".to_string()
}

#[cfg(not(any(unix, windows)))]
fn detect_shell_impl() -> String {
    "sh".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn detect_shell_returns_shell_env_when_set() {
        let prev = std::env::var("SHELL").ok();
        std::env::set_var("SHELL", "/usr/bin/fish");
        let result = detect_shell();
        match prev {
            Some(v) => std::env::set_var("SHELL", v),
            None => std::env::remove_var("SHELL"),
        }
        assert_eq!(result, "/usr/bin/fish");
    }

    #[test]
    #[cfg(unix)]
    fn detect_shell_falls_back_to_sh_when_shell_not_set() {
        let prev = std::env::var("SHELL").ok();
        let prev_nu = std::env::var("NU_VERSION").ok();
        std::env::remove_var("SHELL");
        std::env::remove_var("NU_VERSION");
        let result = detect_shell();
        match prev {
            Some(v) => std::env::set_var("SHELL", v),
            None => std::env::remove_var("SHELL"),
        }
        match prev_nu {
            Some(v) => std::env::set_var("NU_VERSION", v),
            None => std::env::remove_var("NU_VERSION"),
        }
        assert_eq!(result, "sh");
    }

    #[test]
    fn detect_shell_returns_a_non_empty_string() {
        let shell = detect_shell();
        assert!(!shell.is_empty());
    }

    #[test]
    fn launch_env_vars_include_required_keys() {
        use crate::commands::env::collect_vars;
        use crate::config::ApiFormat;
        let vars = collect_vars(
            "myprofile",
            "sk-test",
            "https://example.com",
            &ApiFormat::Anthropic,
        );
        let names: Vec<&str> = vars.iter().map(|(k, _)| *k).collect();
        assert!(names.contains(&"AIX_PROFILE"));
        assert!(names.contains(&"LITELLM_API_KEY"));
        assert!(!names.contains(&"AIX_API_KEY"));
        assert!(!names.contains(&"AIX_BASE_URL"));
    }
}
