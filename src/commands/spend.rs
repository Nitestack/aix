use crate::cache::Cache;
use crate::commands::env::resolve_profile;
use crate::commands::GatewayRequestOptions;
use crate::config;
use crate::error::AixError;
use crate::gateway::{BudgetClient, BudgetInfo};
use crate::output;
use color_eyre::Result;
use serde::Serialize;
use std::path::PathBuf;

pub async fn run(
    options: GatewayRequestOptions,
    config_path: Option<PathBuf>,
    json: bool,
    no_cache: bool,
) -> Result<()> {
    let GatewayRequestOptions { selection, timeout } = options;
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

    let base_url = config::resolve_base_url(profile, &cfg.endpoint)?;
    let api_key = profile.resolve_api_key()?;
    let base_url = base_url.expose_secret();
    let api_key = api_key.expose_secret();
    let cache = Cache::from_config(&cfg.cache);

    let hit = if no_cache {
        None
    } else {
        cache.get(base_url, api_key).and_then(|(data, fetched_at)| {
            BudgetInfo::from_cache_value(&data).map(|budget| (budget, fetched_at))
        })
    };

    let (budget, cached_at) = match hit {
        Some((budget, fetched_at)) => (budget, Some(fetched_at)),
        None => {
            let client = BudgetClient::with_timeout(base_url, api_key, timeout);
            let fresh = client.key_info().await?;
            cache.put(base_url, api_key, &fresh.to_cache_value());
            (fresh, None)
        }
    };

    let summary = SpendSummary::from_budget(&budget)?;
    if json {
        output::print_json("spend", summary)?;
    } else {
        print_human(&summary, cached_at);
    }
    Ok(())
}

#[derive(Serialize)]
struct SpendSummary {
    spend: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_budget: Option<f64>,
}

impl SpendSummary {
    fn from_budget(budget: &BudgetInfo) -> Result<Self, AixError> {
        if budget.remaining_usd.is_none() {
            return Ok(Self {
                spend: budget.key_spent_usd,
                max_budget: None,
            });
        }

        // Keep one coherent spent/limit pair. Never combine key spend with a
        // shared layer's remaining balance to invent a maximum budget.
        let layer = budget
            .layers
            .iter()
            .find(|layer| {
                Some(layer.scope.as_str()) == budget.limited_by.as_deref()
                    && layer.budget_usd.is_some()
            })
            .ok_or(AixError::GatewayProtocolError(
                "gateway did not report the limiting budget layer",
            ))?;
        Ok(Self {
            spend: layer.spent_usd,
            max_budget: layer.budget_usd,
        })
    }
}

fn print_human(summary: &SpendSummary, cached_at: Option<u64>) {
    let cache_suffix = cached_at.map_or(String::new(), |time| {
        let age = crate::cache::now_secs().saturating_sub(time);
        format!("  ·  cached {}", format_age(age))
    });

    for line in display::spend_lines(summary.spend, summary.max_budget, &cache_suffix) {
        println!("{line}");
    }
}

pub(crate) fn format_age(age_secs: u64) -> String {
    match age_secs {
        0..=59 => format!("{age_secs}s ago"),
        60..=3599 => format!("{}m ago", age_secs / 60),
        3600..=86399 => format!("{}h ago", age_secs / 3600),
        _ => format!("{}d ago", age_secs / 86400),
    }
}
pub(crate) mod display;
