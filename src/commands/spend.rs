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
    // LiteLLM masks keys as "sk-...XXXX" — match by the last 4 characters.
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

    let (spend, budget) = if let Some(key) = find_matching_key(keys, api_key) {
        let spend = key.get("spend").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let budget = key.get("max_budget").and_then(|v| v.as_f64());
        (spend, budget)
    } else {
        let spend = data.get("spend").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let budget = data.get("max_budget").and_then(|v| v.as_f64());
        (spend, budget)
    };

    if let Some(b) = budget {
        let remaining = b - spend;
        let pct_used = (spend / b * 100.0).clamp(0.0, 100.0);
        let pct_remaining = 100.0 - pct_used;

        const BAR: usize = 40;
        let filled = (pct_used / 100.0 * BAR as f64).round() as usize;
        let bar = format!("[{}{}]", "█".repeat(filled), "░".repeat(BAR - filled));

        println!("${spend:.2} of ${b:.2}  ·  ${remaining:.2} remaining ({pct_remaining:.0}%)");
        println!("{bar}  {pct_used:.0}% used");
    } else {
        println!("${spend:.2} spent  (no budget set)");
    }
}
