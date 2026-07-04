use crate::client::LiteLlmClient;
use crate::commands::env::resolve_profile;
use crate::config::{self, Gateway, KnownGateway};
use crate::error::AixError;
use color_eyre::Result;
use std::path::PathBuf;

pub async fn run(
    positional_profile: Option<String>,
    config_path: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    let path = config::find_config_path(config_path.as_deref())?.ok_or(AixError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;
    config::load_env_files(&cfg)?;

    if !is_litellm(&cfg) {
        return Err(AixError::NotLiteLlm.into());
    }

    let profile_name = resolve_profile(positional_profile, &cfg)?;
    let profile = cfg
        .profiles
        .get(&profile_name)
        .ok_or_else(|| AixError::ProfileNotFound {
            name: profile_name.clone(),
            available_hint: config::format_available_profiles(&cfg),
        })?;

    let base_url = cfg.endpoint.base_url.resolve()?;
    let api_key = profile.api_key.resolve()?;

    let client = LiteLlmClient::new(base_url.expose_secret(), api_key.expose_secret());
    let data = client.user_info().await?;

    if json {
        let mut out = serde_json::to_string_pretty(&data)?;
        out.push('\n');
        print!("{out}");
    } else {
        print_human(&data);
    }
    Ok(())
}

fn is_litellm(cfg: &config::Config) -> bool {
    matches!(
        cfg.endpoint.gateway,
        Some(Gateway::Known(KnownGateway::Litellm))
    )
}

fn print_human(data: &serde_json::Value) {
    if let Some(id) = data.get("user_id").and_then(|v| v.as_str()) {
        println!("User ID:  {id}");
    }
    if let Some(teams) = data.get("teams").and_then(|v| v.as_array()) {
        let names: Vec<&str> = teams
            .iter()
            .filter_map(|t| t.get("team_alias").and_then(|v| v.as_str()))
            .collect();
        if !names.is_empty() {
            println!("Teams:    {}", names.join(", "));
        }
    }
    if let Some(spend) = data.get("spend").and_then(|v| v.as_f64()) {
        if let Some(budget) = data.get("max_budget").and_then(|v| v.as_f64()) {
            let remaining = budget - spend;
            println!("Spend:    ${spend:.4}  /  ${budget:.2} budget  (${remaining:.4} remaining)");
        } else {
            println!("Spend:    ${spend:.4}");
        }
    }
    if let Some(keys) = data.get("keys").and_then(|v| v.as_array()) {
        println!("Keys:     {}", keys.len());
    }
}
