use crate::client::LiteLlmClient;
use crate::commands::env::resolve_profile;
use crate::config;
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

fn print_human(data: &serde_json::Value) {
    let spend = data.get("spend").and_then(|v| v.as_f64());
    let budget = data.get("max_budget").and_then(|v| v.as_f64());

    match (spend, budget) {
        (Some(s), Some(b)) => {
            let remaining = b - s;
            println!("Spend:     ${s:.4}  /  ${b:.2} budget  (${remaining:.4} remaining)");
        }
        (Some(s), None) => println!("Spend:     ${s:.4}"),
        _ => println!("Spend:     (unavailable)"),
    }

    if let Some(keys) = data.get("keys").and_then(|v| v.as_array()) {
        let keys_with_spend: Vec<_> = keys
            .iter()
            .filter(|k| k.get("spend").and_then(|v| v.as_f64()).is_some())
            .collect();
        if !keys_with_spend.is_empty() {
            println!();
            println!("Keys:");
            for key in keys_with_spend {
                let name = key
                    .get("key_alias")
                    .and_then(|v| v.as_str())
                    .or_else(|| key.get("key_name").and_then(|v| v.as_str()))
                    .unwrap_or("(unnamed)");
                let s = key.get("spend").and_then(|v| v.as_f64()).unwrap_or(0.0);
                println!("  {name:<30}  ${s:.4}");
            }
        }
    }
}
