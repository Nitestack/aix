use crate::cache::Cache;
use crate::commands::env::resolve_profile;
use crate::commands::spend::format_age;
use crate::commands::GatewayRequestOptions;
use crate::config::{self, Gateway, KnownGateway, KnownProvider, Provider};
use crate::error::AixError;
use crate::gateway::{openai::OpenAiClient, BudgetClient, BudgetInfo, BudgetLayer};
use crate::output;
use color_eyre::Result;
use serde::Serialize;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub async fn run(
    options: GatewayRequestOptions,
    config_path: Option<PathBuf>,
    json_output: bool,
    refresh: bool,
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

    let started_at = Instant::now();
    OpenAiClient::with_timeout(base_url, api_key, timeout)
        .models()
        .await?;
    let latency_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);

    let spend = load_spend(&cfg, base_url, api_key, refresh, timeout).await?;
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
    timeout: Duration,
) -> std::result::Result<SpendData, AixError> {
    let cache = Cache::from_config(&cfg.cache);
    if !refresh {
        if let Some((data, fetched_at)) = cache.get(base_url, api_key) {
            if let Some(budget) = BudgetInfo::from_cache_value(&data) {
                return Ok(SpendData::available(
                    &budget,
                    SpendSource::Cache,
                    Some(fetched_at),
                ));
            }
        }
    }

    match BudgetClient::with_timeout(base_url, api_key, timeout)
        .key_info()
        .await
    {
        Ok(budget) => {
            cache.put(base_url, api_key, &budget.to_cache_value());
            Ok(SpendData::available(&budget, SpendSource::Live, None))
        }
        Err(AixError::GatewayError {
            status: 404 | 405 | 501,
            ..
        }) => Ok(SpendData::unavailable(SpendAvailability::Unsupported)),
        Err(AixError::HttpError(error)) if error.is_timeout() => Err(AixError::HttpError(error)),
        Err(AixError::GatewayProtocolError(message)) => {
            Err(AixError::GatewayProtocolError(message))
        }
        Err(_) => Ok(SpendData::unavailable(SpendAvailability::Unavailable)),
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
    key_spent_usd: Option<f64>,
    remaining_usd: Option<f64>,
    limited_by: Option<String>,
    layers: Option<Vec<BudgetLayer>>,
    expires_at: Option<String>,
    cache: SpendCacheData,
}

impl SpendData {
    fn available(budget: &BudgetInfo, source: SpendSource, fetched_at: Option<u64>) -> Self {
        let age_seconds = fetched_at.map(|time| crate::cache::now_secs().saturating_sub(time));
        Self {
            status: SpendAvailability::Available,
            key_spent_usd: Some(budget.key_spent_usd),
            remaining_usd: budget.remaining_usd,
            limited_by: budget.limited_by.clone(),
            layers: Some(budget.layers.clone()),
            expires_at: budget.expires_at.clone(),
            cache: SpendCacheData {
                source,
                age_seconds,
            },
        }
    }

    fn unavailable(status: SpendAvailability) -> Self {
        Self {
            status,
            key_spent_usd: None,
            remaining_usd: None,
            limited_by: None,
            layers: None,
            expires_at: None,
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

    let Some(key_spent_usd) = spend.key_spent_usd else {
        println!("Spend       unavailable");
        return;
    };
    println!();
    for line in crate::commands::spend::display::budget_lines(
        key_spent_usd,
        spend.remaining_usd,
        spend.limited_by.as_deref(),
        spend.layers.as_deref().unwrap_or_default(),
        spend.expires_at.as_deref(),
        &cache_suffix,
    ) {
        println!("{line}");
    }
}
