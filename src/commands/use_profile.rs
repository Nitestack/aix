use crate::cli::{Shell, UseFormat};
use crate::config;
use crate::error::AixError;
use color_eyre::Result;
use std::path::PathBuf;

pub fn run(
    profile: Option<String>,
    clear: bool,
    shell: Option<Shell>,
    format: Option<UseFormat>,
    config_path: Option<PathBuf>,
) -> Result<()> {
    let profile = if clear {
        None
    } else {
        let profile = profile.ok_or(AixError::NoProfile)?;
        let path =
            config::find_config_path(config_path.as_deref())?.ok_or(AixError::NoConfigFile)?;
        let cfg = config::load(&path)?;
        if !cfg.profiles.contains_key(&profile) {
            return Err(AixError::ProfileNotFound {
                name: profile,
                available_hint: profile_names_hint(&cfg),
            }
            .into());
        }
        Some(profile)
    };

    if matches!(format, Some(UseFormat::Json)) {
        println!("{}", serde_json::json!({ "AIX_PROFILE": profile }));
    } else {
        let shell = shell.unwrap_or(Shell::Sh);
        print!("{}", format_shell_assignment(profile.as_deref(), shell));
    }

    Ok(())
}

fn profile_names_hint(cfg: &config::Config) -> String {
    let mut names: Vec<_> = cfg.profiles.keys().map(String::as_str).collect();
    names.sort_unstable();
    if names.is_empty() {
        return "  (no profiles defined)".to_string();
    }
    names
        .into_iter()
        .map(|name| format!("  {name}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_shell_assignment(profile: Option<&str>, shell: Shell) -> String {
    match (profile, shell) {
        (Some(profile), Shell::Sh | Shell::Bash | Shell::Zsh) => {
            format!("export AIX_PROFILE={}\n", sh_quote(profile))
        }
        (Some(profile), Shell::Fish) => {
            format!("set -gx AIX_PROFILE {}\n", fish_quote(profile))
        }
        (Some(profile), Shell::Nu) => {
            format!("$env.AIX_PROFILE = {}\n", nu_quote(profile))
        }
        (Some(profile), Shell::Powershell) => {
            format!("$env:AIX_PROFILE = {}\n", powershell_quote(profile))
        }
        (None, Shell::Sh | Shell::Bash | Shell::Zsh) => "unset AIX_PROFILE\n".to_string(),
        (None, Shell::Fish) => "set --erase --global AIX_PROFILE\n".to_string(),
        (None, Shell::Nu) => "hide-env --ignore-errors AIX_PROFILE\n".to_string(),
        (None, Shell::Powershell) => {
            "if (Test-Path Env:AIX_PROFILE) { Remove-Item Env:AIX_PROFILE }\n".to_string()
        }
    }
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn fish_quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn nu_quote(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
