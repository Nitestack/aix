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
        print_human(&data, api_key.expose_secret());
    }
    Ok(())
}

fn find_matching_key<'a>(
    keys: &'a [serde_json::Value],
    api_key: &str,
) -> Option<&'a serde_json::Value> {
    // LiteLLM masks keys as "sk-...H6Og" — match by the last 4 characters.
    let suffix = &api_key[api_key.len().saturating_sub(4)..];
    keys.iter().find(|k| {
        k.get("key_name")
            .and_then(|v| v.as_str())
            .is_some_and(|kn| kn.ends_with(suffix))
    })
}

fn print_human(data: &serde_json::Value, api_key: &str) {
    let empty = vec![];
    let keys = data
        .get("keys")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);

    if let Some(key) = find_matching_key(keys, api_key) {
        let label = key
            .get("metadata")
            .and_then(|m| m.get("key_name"))
            .and_then(|v| v.as_str())
            .unwrap_or("(unnamed)");
        let key_name = key.get("key_name").and_then(|v| v.as_str()).unwrap_or("?");
        let spend = key.get("spend").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let budget = key.get("max_budget").and_then(|v| v.as_f64());

        println!("Key:       {label}  ({key_name})");
        if let Some(b) = budget {
            let remaining = b - spend;
            println!("Spend:     ${spend:.2}  /  ${b:.2}  (${remaining:.2} remaining)");
        } else {
            println!("Spend:     ${spend:.2}");
        }
    } else {
        // Fallback: no key match found, show user-level totals.
        let spend = data.get("spend").and_then(|v| v.as_f64());
        let budget = data.get("max_budget").and_then(|v| v.as_f64());
        match (spend, budget) {
            (Some(s), Some(b)) => {
                let remaining = b - s;
                println!("Spend:     ${s:.2}  /  ${b:.2}  (${remaining:.2} remaining)");
            }
            (Some(s), None) => println!("Spend:     ${s:.2}"),
            _ => println!("(no spend data available)"),
        }
    }
}
