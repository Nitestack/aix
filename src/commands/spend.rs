use crate::cache::Cache;
use crate::client::LiteLlmClient;
use crate::commands::env::resolve_profile;
use crate::config;
use crate::error::AixError;
use color_eyre::Result;
use owo_colors::Rgb;
use std::path::PathBuf;

pub async fn run(
    positional_profile: Option<String>,
    config_path: Option<PathBuf>,
    json: bool,
    no_cache: bool,
) -> Result<()> {
    let path = config::find_config_path(config_path.as_deref())?.ok_or(AixError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;
    config::load_env_files(&cfg)?;

    if let Some(gateway) = &cfg.endpoint.gateway {
        if !matches!(
            gateway,
            config::Gateway::Known(config::KnownGateway::Litellm)
        ) {
            return Err(AixError::NotLiteLlm.into());
        }
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
    let cache = Cache::from_config(&cfg.cache);

    let hit = if !no_cache {
        cache.get(base_url.expose_secret(), api_key.expose_secret())
    } else {
        None
    };

    let (data, cached_at) = match hit {
        Some((data, fetched_at)) => (data, Some(fetched_at)),
        None => {
            let client = LiteLlmClient::new(base_url.expose_secret(), api_key.expose_secret());
            match client.user_info().await {
                Ok(fresh) => {
                    cache.put(base_url.expose_secret(), api_key.expose_secret(), &fresh);
                    (fresh, None)
                }
                Err(AixError::BudgetExceeded { spend, max_budget }) => {
                    let fresh = serde_json::json!({ "spend": spend, "max_budget": max_budget });
                    cache.put(base_url.expose_secret(), api_key.expose_secret(), &fresh);
                    (fresh, None)
                }
                Err(e) => return Err(e.into()),
            }
        }
    };

    if json {
        let mut out = serde_json::to_string_pretty(&data)?;
        out.push('\n');
        print!("{out}");
    } else {
        print_human(&data, api_key.expose_secret(), cached_at);
    }
    Ok(())
}

fn find_matching_key<'a>(
    keys: &'a [serde_json::Value],
    api_key: &str,
) -> Option<&'a serde_json::Value> {
    let suffix = crate::cache::key_suffix(api_key);
    keys.iter().find(|k| {
        k.get("key_name")
            .and_then(|v| v.as_str())
            .is_some_and(|kn| kn.ends_with(&suffix))
    })
}

fn format_age(age_secs: u64) -> String {
    match age_secs {
        0..=59 => format!("{age_secs}s ago"),
        60..=3599 => format!("{}m ago", age_secs / 60),
        3600..=86399 => format!("{}h ago", age_secs / 3600),
        _ => format!("{}d ago", age_secs / 86400),
    }
}

#[allow(dead_code, reason = "wired into print_human in the next commit")]
fn tier_color(pct_used: f64) -> Rgb {
    match pct_used {
        p if p < 50.0 => Rgb(0, 200, 0),
        p if p < 75.0 => Rgb(220, 200, 0),
        p if p < 100.0 => Rgb(255, 165, 0),
        _ => Rgb(220, 0, 0),
    }
}

fn print_human(data: &serde_json::Value, api_key: &str, cached_at: Option<u64>) {
    let keys: &[serde_json::Value] = data
        .get("keys")
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default();

    let (spend, budget) = if let Some(key) = find_matching_key(keys, api_key) {
        let spend = key.get("spend").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let budget = key.get("max_budget").and_then(|v| v.as_f64());
        (spend, budget)
    } else {
        let spend = data.get("spend").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let budget = data.get("max_budget").and_then(|v| v.as_f64());
        (spend, budget)
    };

    let cache_suffix = cached_at.map_or(String::new(), |t| {
        let age = crate::cache::now_secs().saturating_sub(t);
        format!("  ·  cached {}", format_age(age))
    });

    if let Some(b) = budget {
        let remaining = b - spend;
        let pct_used = if b > 0.0 {
            (spend / b * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
        let pct_remaining = 100.0 - pct_used;

        const BAR: usize = 40;
        let filled = (pct_used / 100.0 * BAR as f64).round() as usize;
        let bar = format!("[{}{}]", "█".repeat(filled), "░".repeat(BAR - filled));

        if remaining < 0.0 {
            println!(
                "${spend:.2} of ${b:.2}  ·  over budget by ${:.2}{cache_suffix}",
                -remaining
            );
        } else {
            println!(
                "${spend:.2} of ${b:.2}  ·  ${remaining:.2} available ({pct_remaining:.0}%){cache_suffix}"
            );
        }
        println!("{bar}  {pct_used:.0}% used");
    } else {
        println!("${spend:.2} spent  (no budget set){cache_suffix}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use owo_colors::Rgb;

    #[test]
    fn tier_color_boundaries() {
        assert_eq!(tier_color(0.0), Rgb(0, 200, 0));
        assert_eq!(tier_color(49.0), Rgb(0, 200, 0));
        assert_eq!(tier_color(50.0), Rgb(220, 200, 0));
        assert_eq!(tier_color(74.0), Rgb(220, 200, 0));
        assert_eq!(tier_color(75.0), Rgb(255, 165, 0));
        assert_eq!(tier_color(99.0), Rgb(255, 165, 0));
        assert_eq!(tier_color(100.0), Rgb(220, 0, 0));
        assert_eq!(tier_color(150.0), Rgb(220, 0, 0));
    }
}
