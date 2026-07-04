use crate::commands::env::{collect_vars, resolve_profile};
use crate::config;
use crate::error::AixError;
use color_eyre::Result;
use std::path::{Path, PathBuf};

pub struct LaunchEnv {
    pub vars: Vec<(&'static str, String)>,
}

pub fn resolve_launch_env(
    profile: Option<String>,
    config_path: Option<PathBuf>,
    format_override: Option<config::ApiFormat>,
) -> Result<LaunchEnv> {
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

    let api_key = profile_entry.api_key.resolve()?;
    let base_url = cfg.endpoint.base_url.resolve()?;
    let api_format = format_override.as_ref().unwrap_or(&cfg.endpoint.api_format);

    let vars = collect_vars(
        &profile_name,
        api_key.expose_secret(),
        base_url.expose_secret(),
        api_format,
    );
    Ok(LaunchEnv { vars })
}

pub fn run_named_tool(
    name: &str,
    format: config::ApiFormat,
    profile: Option<String>,
    config_path: Option<PathBuf>,
    dry_run: bool,
    args: Vec<String>,
) -> Result<()> {
    let env = resolve_launch_env(profile, config_path, Some(format))?;
    run_command(name, &args, &env, dry_run)
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
        return Ok(());
    }

    let resolved = find_executable(program)?;

    let mut cmd = std::process::Command::new(&resolved);
    cmd.args(args);
    for (k, v) in &env.vars {
        cmd.env(k, v);
    }

    let status = cmd.status().map_err(|source| AixError::ProcessSpawn {
        program: program.to_string(),
        source,
    })?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
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
        use crate::config::ApiFormat;
        let vars = collect_vars(
            "myprofile",
            "sk-test",
            "https://example.com",
            &ApiFormat::Anthropic,
        );
        let names: Vec<&str> = vars.iter().map(|(k, _)| *k).collect();
        assert!(names.contains(&"AIX_PROFILE"));
        assert!(!names.contains(&"AIX_API_KEY"));
        assert!(!names.contains(&"AIX_BASE_URL"));
    }
}
