use crate::auth::{AuthService, AuthStatus};
use crate::cli::AuthAction;
use crate::commands::{env::resolve_profile, ProfileSelection};
use crate::config::{self, ProfileAuth};
use crate::error::AixError;
use crate::output;
use color_eyre::Result;
use std::path::PathBuf;
use std::time::Duration;

pub async fn run(
    action: AuthAction,
    mut selection: ProfileSelection,
    config_path: Option<PathBuf>,
    json: bool,
    timeout: Duration,
) -> Result<()> {
    let (operation, positional_profile) = match action {
        AuthAction::Login { profile } => {
            if selection.non_interactive {
                return Err(AixError::AuthLoginRequiresInteractive.into());
            }
            (AuthOperation::Login, profile)
        }
        AuthAction::Status { profile } => (AuthOperation::Status, profile),
        AuthAction::Logout { profile } => (AuthOperation::Logout, profile),
    };
    selection.profile = positional_profile.or(selection.profile);

    let path = config::find_config_path(config_path.as_deref())?.ok_or(AixError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;
    config::load_env_files(&cfg)?;
    let profile_name = resolve_profile(selection, &cfg)?;
    let profile = cfg
        .profiles
        .get(&profile_name)
        .ok_or_else(|| AixError::ProfileNotFound {
            name: profile_name.clone(),
            available_hint: config::format_available_profiles(&cfg),
        })?;

    if matches!(operation, AuthOperation::Login | AuthOperation::Logout)
        && !matches!(&profile.auth, ProfileAuth::ChatGpt)
    {
        return Err(AixError::AuthProfileNotChatGpt.into());
    }

    let service = AuthService::new(timeout)?;
    match operation {
        AuthOperation::Login => {
            service.login(&profile_name).await?;
            let status = service.status(&profile_name, &profile.auth)?;
            print_status(&status, false)?;
        }
        AuthOperation::Status => {
            let status = service.status(&profile_name, &profile.auth)?;
            print_status(&status, json)?;
        }
        AuthOperation::Logout => {
            let outcome = service.logout(&profile_name).await?;
            if !outcome.was_connected {
                println!("Profile {profile_name:?} is already signed out.");
            } else if outcome.remote_revocation_confirmed {
                println!("Signed out ChatGPT profile {profile_name:?}.");
            } else {
                println!("Signed out ChatGPT profile {profile_name:?} locally.");
                eprintln!("Remote revocation was not confirmed; disconnect aix in ChatGPT Settings if needed.");
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum AuthOperation {
    Login,
    Status,
    Logout,
}

fn print_status(status: &AuthStatus, json: bool) -> Result<()> {
    if json {
        return output::print_json("auth status", status);
    }
    println!("Profile: {}", status.profile);
    println!("Authentication: {}", auth_label(&status.auth_type));
    println!("Status: {}", status.state);
    if let Some(enabled) = status.chatgpt_plan_usage_enabled {
        println!(
            "ChatGPT plan usage: {}",
            if enabled { "enabled" } else { "disabled" }
        );
    }
    if let Some(expires_at) = status.access_token_expires_at {
        println!("Access token expires: {}", format_timestamp(expires_at));
    }
    if let Some(email) = &status.email {
        println!("Account: {email}");
    }
    Ok(())
}

fn auth_label(auth_type: &str) -> &'static str {
    match auth_type {
        "chatgpt" => "ChatGPT",
        _ => "API key",
    }
}

fn format_timestamp(timestamp: u64) -> String {
    i64::try_from(timestamp)
        .ok()
        .and_then(|timestamp| chrono::DateTime::from_timestamp(timestamp, 0))
        .map(|date_time| date_time.to_rfc3339())
        .unwrap_or_else(|| timestamp.to_string())
}
