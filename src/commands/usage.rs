mod local;
mod range;
mod report;

use crate::cli::{UsageRangeArgs, UsageSource};
use crate::commands::env::resolve_profile;
use crate::commands::GatewayRequestOptions;
use crate::config::{self, Config, Profile};
use crate::error::AixError;
use crate::gateway::LiteLlmAdminClient;
use crate::output;
use crate::usage_store::{UsageEventFilter, UsageStore};
use chrono::{Local, NaiveDate};
use color_eyre::Result;
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

pub struct UsageOptions {
    pub request: GatewayRequestOptions,
    pub config_path: Option<PathBuf>,
    pub json: bool,
    pub date_range: UsageRangeArgs,
    pub model_filter: Option<String>,
    pub source: UsageSource,
    pub protocol_filter: Option<String>,
    pub tool_filter: Option<String>,
    pub run_filter: Option<String>,
}

pub async fn run(command: UsageOptions) -> Result<()> {
    let UsageOptions {
        request,
        config_path,
        json,
        date_range,
        model_filter,
        source,
        protocol_filter,
        tool_filter,
        run_filter,
    } = command;
    let GatewayRequestOptions { selection, timeout } = request;
    let range = range::UsageRange::from_args(&date_range, Local::now().date_naive())?;
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
    let litellm_available = profile.auth.can_query_litellm_usage() && gateway_is_litellm(&cfg);
    let has_local_only_filter =
        protocol_filter.is_some() || tool_filter.is_some() || run_filter.is_some();

    let selected_source = match source {
        UsageSource::Auto if has_local_only_filter || !litellm_available => UsageSource::Local,
        UsageSource::Auto => UsageSource::LiteLlm,
        other => other,
    };

    if selected_source == UsageSource::LiteLlm {
        if has_local_only_filter {
            return Err(AixError::UsageLocalFilterWithLiteLlm.into());
        }
        let report = fetch_litellm_report(
            profile,
            &cfg,
            &profile_name,
            timeout,
            range.start,
            range.end,
            model_filter,
        )
        .await?;
        if json {
            output::print_json("usage", report)?;
        } else {
            report::print_human(&report, range.is_default);
        }
        return Ok(());
    }

    let (start_unix_ms, end_unix_ms) = range.local_unix_millis_bounds()?;
    let filter = UsageEventFilter {
        start_unix_ms,
        end_unix_ms,
        profile: Some(profile_name.clone()),
        logical_tool_name: tool_filter,
        model: model_filter.clone(),
        protocol: protocol_filter,
        run_id: run_filter,
    };
    let (events, storage_unavailable, scan_incomplete) =
        match UsageStore::from_environment().and_then(|store| store.scan_events(&filter)) {
            Ok(scan) => (scan.events, false, scan.incomplete),
            Err(_) => (Vec::new(), true, false),
        };
    let fallback_message = (profile.auth.can_query_litellm_usage() && !gateway_is_litellm(&cfg))
        .then(|| "This gateway does not provide LiteLLM historical activity.".to_string());
    let local_report = local::LocalUsageReport::from_events(
        events,
        local::LocalUsageContext {
            start_date: range.start,
            end_date: range.end,
            profile: profile_name.clone(),
            is_chatgpt: profile.auth.local_usage_cost_is_not_applicable(),
            storage_unavailable,
            scan_incomplete,
            model_filter: model_filter.clone(),
            message: fallback_message,
        },
    );

    match selected_source {
        UsageSource::Local => {
            if json {
                output::print_json("usage", local_report)?;
            } else {
                local_report.print_human();
            }
        }
        UsageSource::All => {
            let litellm = if litellm_available {
                match fetch_litellm_report(
                    profile,
                    &cfg,
                    &profile_name,
                    timeout,
                    range.start,
                    range.end,
                    model_filter,
                )
                .await
                {
                    Ok(report) => LiteLlmSourceReport::Available(Box::new(report)),
                    Err(error) => {
                        LiteLlmSourceReport::Unavailable(unavailable_litellm_source(&error))
                    }
                }
            } else {
                LiteLlmSourceReport::Unavailable(UnsupportedSourceReport {
                    source: "litellm",
                    status: "unsupported",
                    message: if !profile.auth.can_query_litellm_usage() {
                        "LiteLLM historical usage requires an API-key profile."
                    } else {
                        "The configured gateway does not provide LiteLLM historical activity."
                    },
                })
            };
            let report = CombinedUsageReport {
                source: "all",
                start_date: range.start.format("%Y-%m-%d").to_string(),
                end_date: range.end.format("%Y-%m-%d").to_string(),
                profile: profile_name,
                sources: CombinedSources {
                    local_gateway: local_report,
                    litellm,
                },
            };
            if json {
                output::print_json("usage", report)?;
            } else {
                report.sources.local_gateway.print_human();
                println!("\nBilling / upstream aggregate");
                match &report.sources.litellm {
                    LiteLlmSourceReport::Available(litellm) => {
                        report::print_human(litellm, range.is_default);
                    }
                    LiteLlmSourceReport::Unavailable(unavailable) => {
                        println!("  {}", unavailable.message);
                    }
                }
            }
        }
        UsageSource::Auto | UsageSource::LiteLlm => unreachable!("source was resolved above"),
    }
    Ok(())
}

async fn fetch_litellm_report(
    profile: &Profile,
    cfg: &Config,
    profile_name: &str,
    timeout: Duration,
    start: NaiveDate,
    end: NaiveDate,
    model_filter: Option<String>,
) -> Result<report::UsageReport> {
    if !profile.auth.can_query_litellm_usage() {
        return Err(AixError::UsageLiteLlmRequiresApiKey.into());
    }
    if !gateway_is_litellm(cfg) {
        return Err(AixError::UsageNotLiteLlm.into());
    }
    let base_url = config::resolve_base_url(profile, &cfg.endpoint)?;
    let api_key = profile.resolve_api_key()?;
    let client = LiteLlmAdminClient::with_timeout(
        base_url.expose_secret(),
        api_key.expose_secret(),
        timeout,
    );
    let response = client
        .daily_activity(
            &start.format("%Y-%m-%d").to_string(),
            &end.format("%Y-%m-%d").to_string(),
        )
        .await?;

    Ok(report::UsageReport::from_response(
        response,
        start,
        end,
        model_filter,
        profile_name.to_string(),
    ))
}

fn gateway_is_litellm(cfg: &Config) -> bool {
    cfg.endpoint.gateway.as_ref().is_none_or(|gateway| {
        matches!(
            gateway,
            config::Gateway::Known(config::KnownGateway::Litellm)
        )
    })
}

fn unavailable_litellm_source(error: &color_eyre::Report) -> UnsupportedSourceReport {
    let (status, message) = match error.downcast_ref::<AixError>() {
        None => (
            "unavailable",
            "LiteLLM usage could not be accessed using the configured profile.",
        ),
        Some(AixError::UsageUnavailable) => (
            "unsupported",
            "LiteLLM historical activity is unsupported by this gateway/API version.",
        ),
        Some(
            AixError::GatewayError {
                status: 401 | 403, ..
            }
            | AixError::GatewayRequestFailed { status: 401 | 403 },
        ) => (
            "unavailable",
            "LiteLLM rejected the configured credentials.",
        ),
        Some(
            AixError::GatewayError { .. }
            | AixError::GatewayRequestFailed { .. }
            | AixError::GatewayProtocolError(_)
            | AixError::HttpError(_),
        ) => (
            "unavailable",
            "LiteLLM historical activity could not be fetched.",
        ),
        Some(_) => (
            "unavailable",
            "LiteLLM usage could not be accessed using the configured profile.",
        ),
    };
    UnsupportedSourceReport {
        source: "litellm",
        status,
        message,
    }
}

#[derive(Serialize)]
struct CombinedUsageReport {
    source: &'static str,
    start_date: String,
    end_date: String,
    profile: String,
    sources: CombinedSources,
}

#[derive(Serialize)]
struct CombinedSources {
    local_gateway: local::LocalUsageReport,
    litellm: LiteLlmSourceReport,
}

#[derive(Serialize)]
#[serde(untagged)]
enum LiteLlmSourceReport {
    Available(Box<report::UsageReport>),
    Unavailable(UnsupportedSourceReport),
}

#[derive(Serialize)]
struct UnsupportedSourceReport {
    source: &'static str,
    status: &'static str,
    message: &'static str,
}
