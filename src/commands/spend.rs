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
        Some((budget, fetched_at)) => (Some(budget), Some(fetched_at)),
        None => {
            let client = BudgetClient::with_timeout(base_url, api_key, timeout);
            match client.key_info().await {
                Ok(fresh) => {
                    cache.put(base_url, api_key, &fresh.to_cache_value());
                    (Some(fresh), None)
                }
                Err(AixError::GatewayError {
                    status: 404 | 405 | 501,
                    ..
                }) => (None, None),
                Err(error) => return Err(error.into()),
            }
        }
    };

    let summary = budget
        .as_ref()
        .map_or_else(SpendSummary::unsupported, |budget| {
            SpendSummary::available(budget)
        });
    if json {
        output::print_json("spend", summary)?;
    } else if let Some(budget) = budget.as_ref() {
        print_human(budget, cached_at);
    } else {
        println!("Budget info  unsupported");
    }
    Ok(())
}

#[derive(Serialize)]
struct SpendSummary {
    status: SpendStatus,
    key_spent_usd: Option<f64>,
    remaining_usd: Option<f64>,
    limited_by: Option<String>,
    layers: Option<Vec<SpendLayerSummary>>,
    expires_at: Option<String>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum SpendStatus {
    Available,
    Unsupported,
}

#[derive(Serialize)]
struct SpendLayerSummary {
    scope: String,
    budget_usd: Option<f64>,
    spent_usd: f64,
    remaining_usd: Option<f64>,
    period_start: String,
}

impl SpendSummary {
    fn available(budget: &BudgetInfo) -> Self {
        Self {
            status: SpendStatus::Available,
            key_spent_usd: Some(budget.key_spent_usd),
            remaining_usd: budget.remaining_usd,
            limited_by: budget.limited_by.clone(),
            layers: Some(
                budget
                    .layers
                    .iter()
                    .map(|layer| SpendLayerSummary {
                        scope: layer.scope.clone(),
                        budget_usd: layer.budget_usd,
                        spent_usd: layer.spent_usd,
                        remaining_usd: layer.remaining_usd,
                        period_start: layer.period_start.clone(),
                    })
                    .collect(),
            ),
            expires_at: budget.expires_at.clone(),
        }
    }

    fn unsupported() -> Self {
        Self {
            status: SpendStatus::Unsupported,
            key_spent_usd: None,
            remaining_usd: None,
            limited_by: None,
            layers: None,
            expires_at: None,
        }
    }
}

fn print_human(budget: &BudgetInfo, cached_at: Option<u64>) {
    let cache_suffix = cached_at.map_or(" · live".to_string(), |time| {
        let age = crate::cache::now_secs().saturating_sub(time);
        format!(" · cached {}", format_age(age))
    });

    for line in display::budget_lines(
        budget.key_spent_usd,
        budget.remaining_usd,
        budget.limited_by.as_deref(),
        &budget.layers,
        budget.expires_at.as_deref(),
        &cache_suffix,
    ) {
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
