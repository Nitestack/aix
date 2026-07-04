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
    limit: u32,
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
    let data = client.spend_logs(limit).await?;

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
    let entries = match data.as_array() {
        Some(arr) => arr,
        None => {
            println!("(no spend data)");
            return;
        }
    };
    if entries.is_empty() {
        println!("(no spend logs)");
        return;
    }
    println!("{:<30}  {:<25}  {:>12}", "Time", "Model", "Cost ($)");
    println!("{}", "-".repeat(72));
    for entry in entries {
        let time = entry
            .get("startTime")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let model = entry.get("model").and_then(|v| v.as_str()).unwrap_or("-");
        let cost = entry.get("spend").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let req_id = entry
            .get("request_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let short_id = if req_id.len() > 8 {
            &req_id[..8]
        } else {
            req_id
        };
        println!("{time:<30}  {model:<25}  {cost:>12.6}  [{short_id}]");
    }
}
