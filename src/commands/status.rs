use crate::cache::Cache;
use crate::commands::env::resolve_profile;
use crate::commands::spend::{format_age, has_spend_data, spend_summary};
use crate::config::{self, Gateway, KnownGateway, KnownProvider, Provider};
use crate::error::AixError;
use crate::gateway::{openai::OpenAiClient, LiteLlmAdminClient};
use crate::output;
use color_eyre::Result;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Instant;

pub async fn run(
    positional_profile: Option<String>,
    config_path: Option<PathBuf>,
    json_output: bool,
    refresh: bool,
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
    let label = profile
        .label
        .as_ref()
        .map(|label| label.resolve())
        .transpose()?
        .unwrap_or_else(|| profile_name.clone());
    let base_url = config::resolve_base_url(profile, &cfg.endpoint)?;
    let api_key = profile.api_key.resolve()?;
    let base_url = base_url.expose_secret();
    let api_key = api_key.expose_secret();

    let started_at = Instant::now();
    OpenAiClient::new(base_url, api_key).models().await?;
    let latency_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);

    let spend = load_spend(&cfg, base_url, api_key, refresh).await;
    let status = StatusData {
        profile: ProfileData {
            name: profile_name,
            label,
        },
        gateway: GatewayData {
            gateway: cfg
                .endpoint
                .gateway
                .as_ref()
                .map(gateway_name)
                .map(str::to_owned),
            provider: cfg
                .endpoint
                .provider
                .as_ref()
                .map(provider_name)
                .map(str::to_owned),
            status: "reachable",
            authentication_status: "authenticated",
            latency_ms,
        },
        key_suffix: key_suffix(api_key),
        spend,
    };

    if json_output {
        output::print_json("status", status)?;
    } else {
        print_human(&status);
    }
    Ok(())
}

async fn load_spend(
    cfg: &config::Config,
    base_url: &str,
    api_key: &str,
    refresh: bool,
) -> SpendData {
    if matches!(cfg.endpoint.gateway.as_ref(), Some(Gateway::Custom(_))) {
        return SpendData::unavailable(SpendAvailability::Unsupported);
    }

    let cache = Cache::from_config(&cfg.cache);
    if !refresh {
        if let Some((data, fetched_at)) = cache.get(base_url, api_key) {
            if has_spend_data(&data, api_key) {
                return SpendData::available(&data, api_key, SpendSource::Cache, Some(fetched_at));
            }
        }
    }

    match LiteLlmAdminClient::new(base_url, api_key).user_info().await {
        Ok(data) => {
            if !has_spend_data(&data, api_key) {
                return SpendData::unavailable(SpendAvailability::Unavailable);
            }
            cache.put(base_url, api_key, &data);
            SpendData::available(&data, api_key, SpendSource::Live, None)
        }
        Err(AixError::BudgetExceeded { spend, max_budget }) => {
            let data = json!({
                "spend": spend,
                "max_budget": max_budget,
                "_aix_budget_exceeded": true
            });
            cache.put(base_url, api_key, &data);
            SpendData::available(&data, api_key, SpendSource::Live, None)
        }
        Err(AixError::GatewayError {
            status: 404 | 405 | 501,
            ..
        }) => SpendData::unavailable(SpendAvailability::Unsupported),
        Err(_) => SpendData::unavailable(SpendAvailability::Unavailable),
    }
}

#[derive(Serialize)]
struct StatusData {
    profile: ProfileData,
    gateway: GatewayData,
    key_suffix: Option<String>,
    spend: SpendData,
}

#[derive(Serialize)]
struct ProfileData {
    name: String,
    label: String,
}

#[derive(Serialize)]
struct GatewayData {
    gateway: Option<String>,
    provider: Option<String>,
    status: &'static str,
    authentication_status: &'static str,
    latency_ms: u64,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum SpendAvailability {
    Available,
    Unsupported,
    Unavailable,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum SpendSource {
    Cache,
    Live,
    None,
}

#[derive(Serialize)]
struct SpendCacheData {
    source: SpendSource,
    age_seconds: Option<u64>,
}

#[derive(Serialize)]
struct SpendData {
    status: SpendAvailability,
    spend: Option<f64>,
    max_budget: Option<f64>,
    remaining_budget: Option<f64>,
    percent_used: Option<f64>,
    cache: SpendCacheData,
}

impl SpendData {
    fn available(
        data: &Value,
        api_key: &str,
        source: SpendSource,
        fetched_at: Option<u64>,
    ) -> Self {
        let summary = spend_summary(data, api_key);
        let remaining_budget = summary.max_budget.map(|budget| budget - summary.spend);
        let percent_used = summary
            .max_budget
            .filter(|budget| *budget > 0.0)
            .map(|budget| summary.spend / budget * 100.0);
        let age_seconds = fetched_at.map(|time| crate::cache::now_secs().saturating_sub(time));

        Self {
            status: SpendAvailability::Available,
            spend: Some(summary.spend),
            max_budget: summary.max_budget,
            remaining_budget,
            percent_used,
            cache: SpendCacheData {
                source,
                age_seconds,
            },
        }
    }

    fn unavailable(status: SpendAvailability) -> Self {
        Self {
            status,
            spend: None,
            max_budget: None,
            remaining_budget: None,
            percent_used: None,
            cache: SpendCacheData {
                source: SpendSource::None,
                age_seconds: None,
            },
        }
    }
}

fn gateway_name(gateway: &Gateway) -> &str {
    match gateway {
        Gateway::Known(KnownGateway::Litellm) => "litellm",
        Gateway::Custom(name) => name,
    }
}

fn provider_name(provider: &Provider) -> &str {
    match provider {
        Provider::Known(KnownProvider::LiteLlm) => "litellm",
        Provider::Custom(name) => name,
    }
}

fn key_suffix(api_key: &str) -> Option<String> {
    let mut characters = api_key.chars().rev();
    let suffix: String = characters
        .by_ref()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    (characters.next().is_some()).then_some(suffix)
}

fn print_human(status: &StatusData) {
    let profile = if status.profile.name == status.profile.label {
        status.profile.name.clone()
    } else {
        format!("{} · {}", status.profile.name, status.profile.label)
    };
    println!("Profile     {profile}");

    let mut metadata = Vec::new();
    if let Some(gateway) = &status.gateway.gateway {
        metadata.push(gateway.clone());
    }
    if let Some(provider) = &status.gateway.provider {
        metadata.push(format!("provider={provider}"));
    }
    let metadata = if metadata.is_empty() {
        String::new()
    } else {
        format!("{} · ", metadata.join(" · "))
    };
    println!(
        "Gateway     {metadata}{} · {} · {}ms",
        status.gateway.status, status.gateway.authentication_status, status.gateway.latency_ms
    );

    match &status.key_suffix {
        Some(suffix) => println!("Key         sk-...{suffix}"),
        None => println!("Key         redacted"),
    }

    match status.spend.status {
        SpendAvailability::Available => print_spend_human(&status.spend),
        SpendAvailability::Unsupported => println!("Spend       unsupported"),
        SpendAvailability::Unavailable => println!("Spend       unavailable"),
    }
}

fn print_spend_human(spend: &SpendData) {
    let cache_suffix = match spend.cache.source {
        SpendSource::Cache => format!(
            " · cached {}",
            format_age(spend.cache.age_seconds.unwrap_or_default())
        ),
        SpendSource::Live => " · live".to_string(),
        SpendSource::None => String::new(),
    };

    match (spend.spend, spend.max_budget) {
        (Some(amount), Some(budget)) => {
            let percent = spend
                .percent_used
                .map(|percent| format!(" · {percent:.0}% used"))
                .unwrap_or_default();
            if let Some(remaining) = spend.remaining_budget.filter(|value| *value < 0.0) {
                println!(
                    "Spend       ${amount:.2} / ${budget:.2} · over budget by ${:.2}{percent}{cache_suffix}",
                    -remaining
                );
            } else {
                let remaining = spend.remaining_budget.unwrap_or_default();
                println!(
                    "Spend       ${amount:.2} / ${budget:.2} · ${remaining:.2} remaining{percent}{cache_suffix}"
                );
            }
        }
        (Some(amount), None) => {
            println!("Spend       ${amount:.2} · no budget set{cache_suffix}");
        }
        _ => println!("Spend       unavailable"),
    }
}
