use crate::cache::Cache;
use crate::commands::env::resolve_profile;
use crate::commands::spend::format_age;
use crate::commands::spend::report::{
    lookup_budget, CachePolicy, SpendAvailability, SpendData, SpendSource,
};
use crate::commands::spend::ProfileData;
use crate::commands::GatewayRequestOptions;
use crate::config;
use crate::error::AixError;
use crate::gateway::identity::{self, DetectedFrom};
use crate::gateway::openai::OpenAiClient;
use crate::output;
use color_eyre::Result;
use serde::Serialize;
use std::path::PathBuf;
use std::time::Instant;

pub async fn run(
    options: GatewayRequestOptions,
    config_path: Option<PathBuf>,
    json_output: bool,
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

    let started_at = Instant::now();
    OpenAiClient::with_timeout(base_url, api_key, timeout)
        .models()
        .await?;
    let latency_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);

    let cache = Cache::from_config(&cfg.cache);
    let (spend, reported_gateway) =
        match lookup_budget(&cache, base_url, api_key, policy, timeout).await {
            Ok(Some(lookup)) => (SpendData::available(&lookup), lookup.budget.gateway.clone()),
            Ok(None) => (SpendData::unavailable(SpendAvailability::Unsupported), None),
            Err(AixError::HttpError(error)) if error.is_timeout() => {
                return Err(AixError::HttpError(error).into())
            }
            Err(AixError::GatewayProtocolError(message)) => {
                return Err(AixError::GatewayProtocolError(message).into())
            }
            Err(_) => (SpendData::unavailable(SpendAvailability::Unavailable), None),
        };
    let gateway = identity::identify(&cfg.endpoint, base_url, reported_gateway.as_deref());
    let status = StatusData {
        profile: ProfileData {
            name: profile_name,
            label,
        },
        gateway: GatewayData {
            gateway: gateway.gateway,
            provider: gateway.provider,
            detected_from: gateway.detected_from,
            status: "reachable",
            authentication_status: "authenticated",
            latency_ms,
        },
        key_suffix: key_suffix(api_key),
        key_fingerprint: identity::key_fingerprint(base_url, api_key),
        spend,
    };

    if json_output {
        output::print_json("status", status)?;
    } else {
        print_human(&status);
    }
    Ok(())
}

#[derive(Serialize)]
struct StatusData {
    profile: ProfileData,
    gateway: GatewayData,
    key_suffix: Option<String>,
    key_fingerprint: String,
    spend: SpendData,
}

#[derive(Serialize)]
struct GatewayData {
    gateway: Option<String>,
    provider: Option<String>,
    detected_from: Option<DetectedFrom>,
    status: &'static str,
    authentication_status: &'static str,
    latency_ms: u64,
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
