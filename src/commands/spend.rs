use crate::cache::Cache;
use crate::commands::env::resolve_profile;
use crate::commands::GatewayRequestOptions;
use crate::config;
use crate::error::AixError;
use crate::gateway::identity::{self, GatewayIdentity};
use crate::gateway::BudgetInfo;
use crate::output;
use color_eyre::Result;
use report::{lookup_budget, CachePolicy, SpendAvailability, SpendData};
use serde::Serialize;
use std::path::PathBuf;

pub(crate) mod report;

pub async fn run(
    options: GatewayRequestOptions,
    config_path: Option<PathBuf>,
    json: bool,
    policy: CachePolicy,
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
    let label = profile
        .label
        .as_ref()
        .map(|label| label.resolve())
        .transpose()?
        .unwrap_or_else(|| profile_name.clone());

    let base_url = config::resolve_base_url(profile, &cfg.endpoint)?;
    let api_key = profile.resolve_api_key()?;
    let base_url = base_url.expose_secret();
    let api_key = api_key.expose_secret();
    let cache = Cache::from_config(&cfg.cache);

    // A gateway without the native budget endpoint is a capability gap, not a
    // failure: report it instead of failing scripts that scan spend across
    // profiles. Other gateway errors keep their exit codes.
    let lookup = lookup_budget(&cache, base_url, api_key, policy, timeout).await?;

    let gateway = identity::identify(
        &cfg.endpoint,
        base_url,
        lookup
            .as_ref()
            .and_then(|lookup| lookup.budget.gateway.as_deref()),
    );
    let key_fingerprint = identity::key_fingerprint(base_url, api_key);

    let Some(lookup) = lookup else {
        if json {
            output::print_json(
                "spend",
                SpendJson {
                    summary: None,
                    details: SpendData::unavailable(SpendAvailability::Unsupported),
                    profile: ProfileData {
                        name: profile_name,
                        label,
                    },
                    key_fingerprint,
                    gateway,
                },
            )?;
        } else {
            println!("Spend unsupported by this gateway");
        }
        return Ok(());
    };

    let summary = SpendSummary::from_budget(&lookup.budget)?;
    if json {
        output::print_json(
            "spend",
            SpendJson {
                summary: Some(summary),
                details: SpendData::available(&lookup),
                profile: ProfileData {
                    name: profile_name,
                    label,
                },
                key_fingerprint,
                gateway,
            },
        )?;
    } else {
        let cached_at = (lookup.source == report::SpendSource::Cache).then_some(lookup.fetched_at);
        print_human(&summary, cached_at);
    }
    Ok(())
}

/// `spend.data`: the original `spend`/`max_budget` summary (present only when
/// a budget is available) plus the same budget object as `status.data.spend`,
/// and the non-secret profile, key, and gateway identity.
#[derive(Serialize)]
struct SpendJson {
    #[serde(flatten)]
    summary: Option<SpendSummary>,
    #[serde(flatten)]
    details: SpendData,
    profile: ProfileData,
    key_fingerprint: String,
    gateway: GatewayIdentity,
}

#[derive(Serialize)]
pub(crate) struct ProfileData {
    pub(crate) name: String,
    pub(crate) label: String,
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
