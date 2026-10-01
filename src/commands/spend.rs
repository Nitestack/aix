use crate::cache::Cache;
use crate::commands::env::resolve_profile;
use crate::config;
use crate::error::AixError;
use crate::gateway::LiteLlmAdminClient;
use crate::output;
use color_eyre::Result;
use owo_colors::{OwoColorize, Rgb, Stream::Stdout};
use serde::Serialize;
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

    let base_url = config::resolve_base_url(profile, &cfg.endpoint)?;
    let api_key = profile.resolve_api_key()?;
    let cache = Cache::from_config(&cfg.cache);

    let hit = if !no_cache {
        cache.get(base_url.expose_secret(), api_key.expose_secret())
    } else {
        None
    };

    let (data, cached_at) = match hit {
        Some((data, fetched_at)) => (data, Some(fetched_at)),
        None => {
            let client = LiteLlmAdminClient::new(base_url.expose_secret(), api_key.expose_secret());
            match client.user_info().await {
                Ok(fresh) => {
                    cache.put(base_url.expose_secret(), api_key.expose_secret(), &fresh);
                    (fresh, None)
                }
                Err(AixError::BudgetExceeded { spend, max_budget }) => {
                    let fresh = serde_json::json!({
                        "spend": spend,
                        "max_budget": max_budget,
                        "_aix_budget_exceeded": true
                    });
                    cache.put(base_url.expose_secret(), api_key.expose_secret(), &fresh);
                    (fresh, None)
                }
                Err(e) => return Err(e.into()),
            }
        }
    };

    if json {
        if let Some(error) = budget_error(&data) {
            return Err(error.into());
        }
        output::print_json("spend", spend_summary(&data, api_key.expose_secret()))?;
    } else {
        print_human(&data, api_key.expose_secret(), cached_at);
        if let Some(error) = budget_error(&data) {
            return Err(error.into());
        }
    }
    Ok(())
}

fn find_matching_key<'a>(
    keys: &'a [serde_json::Value],
    api_key: &str,
) -> Option<&'a serde_json::Value> {
    keys.iter().find(|k| {
        k.get("key_name")
            .and_then(|v| v.as_str())
            .is_some_and(|kn| crate::cache::key_identity_matches_name(api_key, kn))
    })
}

pub(crate) fn has_spend_data(data: &serde_json::Value, api_key: &str) -> bool {
    let keys: &[serde_json::Value] = data
        .get("keys")
        .and_then(|value| value.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default();
    let source = find_matching_key(keys, api_key).unwrap_or(data);

    ["spend", "max_budget"].iter().any(|field| {
        source
            .get(field)
            .and_then(serde_json::Value::as_f64)
            .is_some()
    })
}

pub(crate) fn format_age(age_secs: u64) -> String {
    match age_secs {
        0..=59 => format!("{age_secs}s ago"),
        60..=3599 => format!("{}m ago", age_secs / 60),
        3600..=86399 => format!("{}h ago", age_secs / 3600),
        _ => format!("{}d ago", age_secs / 86400),
    }
}

fn budget_error(data: &serde_json::Value) -> Option<AixError> {
    data.get("_aix_budget_exceeded")
        .and_then(serde_json::Value::as_bool)
        .filter(|exceeded| *exceeded)
        .map(|_| AixError::BudgetExceeded {
            spend: data
                .get("spend")
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0),
            max_budget: data
                .get("max_budget")
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0),
        })
}

#[derive(Serialize)]
pub(crate) struct SpendSummary {
    pub(crate) spend: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_budget: Option<f64>,
}

pub(crate) fn spend_summary(data: &serde_json::Value, api_key: &str) -> SpendSummary {
    let keys: &[serde_json::Value] = data
        .get("keys")
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default();

    let (spend, max_budget) = if let Some(key) = find_matching_key(keys, api_key) {
        (
            key.get("spend").and_then(|v| v.as_f64()).unwrap_or(0.0),
            key.get("max_budget").and_then(|v| v.as_f64()),
        )
    } else {
        (
            data.get("spend").and_then(|v| v.as_f64()).unwrap_or(0.0),
            data.get("max_budget").and_then(|v| v.as_f64()),
        )
    };

    SpendSummary { spend, max_budget }
}

const GRADIENT_STOPS: [(f64, Rgb); 4] = [
    (0.0, Rgb(0, 200, 0)),
    (50.0, Rgb(220, 200, 0)),
    (75.0, Rgb(255, 165, 0)),
    (100.0, Rgb(220, 0, 0)),
];

fn lerp_channel(a: u8, b: u8, t: f64) -> u8 {
    (a as f64 + (b as f64 - a as f64) * t).round() as u8
}

fn gradient_color(pct_used: f64) -> Rgb {
    let pct = pct_used.clamp(0.0, 100.0);
    let segment = GRADIENT_STOPS
        .windows(2)
        .find(|w| pct <= w[1].0)
        .unwrap_or(&GRADIENT_STOPS[GRADIENT_STOPS.len() - 2..]);
    let (p0, c0) = segment[0];
    let (p1, c1) = segment[1];
    let t = if p1 > p0 { (pct - p0) / (p1 - p0) } else { 0.0 };

    Rgb(
        lerp_channel(c0.0, c1.0, t),
        lerp_channel(c0.1, c1.1, t),
        lerp_channel(c0.2, c1.2, t),
    )
}

fn position_color(index: usize, bar_len: usize) -> Rgb {
    let pct = index as f64 / (bar_len - 1) as f64 * 100.0;
    gradient_color(pct)
}

fn print_human(data: &serde_json::Value, api_key: &str, cached_at: Option<u64>) {
    let SpendSummary { spend, max_budget } = spend_summary(data, api_key);

    let cache_suffix = cached_at.map_or(String::new(), |t| {
        let age = crate::cache::now_secs().saturating_sub(t);
        format!("  ·  cached {}", format_age(age))
    });

    if let Some(b) = max_budget {
        let remaining = b - spend;
        let pct_used = if b > 0.0 {
            (spend / b * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
        let pct_remaining = 100.0 - pct_used;

        const BAR: usize = 40;
        let filled = (pct_used / 100.0 * BAR as f64).round() as usize;
        let bar_empty = "░".repeat(BAR - filled);
        let bar_filled: String = (0..filled)
            .map(|i| {
                "█"
                    .if_supports_color(Stdout, |t| t.color(position_color(i, BAR)))
                    .to_string()
            })
            .collect();
        let label = format!("{pct_used:.0}% used")
            .if_supports_color(Stdout, |t| t.color(gradient_color(pct_used)))
            .to_string();

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
        println!("[{bar_filled}{bar_empty}]  {label}");
    } else {
        println!("${spend:.2} spent  (no budget set){cache_suffix}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use owo_colors::Rgb;

    #[test]
    fn gradient_color_at_stops() {
        assert_eq!(gradient_color(0.0), Rgb(0, 200, 0));
        assert_eq!(gradient_color(50.0), Rgb(220, 200, 0));
        assert_eq!(gradient_color(75.0), Rgb(255, 165, 0));
        assert_eq!(gradient_color(100.0), Rgb(220, 0, 0));
    }

    #[test]
    fn gradient_color_interpolates_between_stops() {
        // Halfway between green (0%) and yellow (50%).
        assert_eq!(gradient_color(25.0), Rgb(110, 200, 0));
        // Halfway between yellow (50%) and orange (75%).
        assert_eq!(gradient_color(62.5), Rgb(238, 183, 0));
        // Halfway between orange (75%) and red (100%).
        assert_eq!(gradient_color(87.5), Rgb(238, 83, 0));
    }

    #[test]
    fn gradient_color_clamps_out_of_range_input() {
        assert_eq!(gradient_color(-10.0), Rgb(0, 200, 0));
        assert_eq!(gradient_color(150.0), Rgb(220, 0, 0));
    }

    #[test]
    fn position_color_sweeps_the_full_spectrum_across_a_bar() {
        assert_eq!(position_color(0, 40), Rgb(0, 200, 0));
        assert_eq!(position_color(39, 40), Rgb(220, 0, 0));
        // index 20 of 40 -> 20/39*100 = 51.28...%, between the yellow (50%) and
        // orange (75%) stops.
        assert_eq!(position_color(20, 40), Rgb(222, 198, 0));
    }

    #[test]
    fn find_matching_key_matches_short_credentials_without_exposing_them() {
        let api_key = "abcd";
        let key_name = crate::cache::sanitized_key_name(api_key);
        let keys = [serde_json::json!({ "key_name": key_name, "spend": 2.0 })];

        let matched = find_matching_key(&keys, api_key).unwrap();
        assert_eq!(matched["spend"], 2.0);
        assert!(!matched.to_string().contains(api_key));
    }

    #[test]
    fn find_matching_key_matches_masked_short_key_suffix() {
        let api_key = "abcd";
        let key_name = crate::cache::sanitized_key_name("sk-...abcd");
        let keys = [serde_json::json!({ "key_name": key_name, "spend": 2.0 })];

        let matched = find_matching_key(&keys, api_key).unwrap();
        assert_eq!(matched["spend"], 2.0);
    }
}
