mod range;
mod report;

use crate::cli::UsageRangeArgs;
use crate::commands::env::resolve_profile;
use crate::commands::GatewayRequestOptions;
use crate::config;
use crate::error::AixError;
use crate::gateway::LiteLlmAdminClient;
use crate::output;
use chrono::Local;
use color_eyre::Result;
use std::path::PathBuf;

pub async fn run(
    options: GatewayRequestOptions,
    config_path: Option<PathBuf>,
    json: bool,
    date_range: UsageRangeArgs,
    model_filter: Option<String>,
) -> Result<()> {
    let GatewayRequestOptions { selection, timeout } = options;
    let range = range::UsageRange::from_args(&date_range, Local::now().date_naive())?;

    let path = config::find_config_path(config_path.as_deref())?.ok_or(AixError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;

    if let Some(gateway) = &cfg.endpoint.gateway {
        if !matches!(
            gateway,
            config::Gateway::Known(config::KnownGateway::Litellm)
        ) {
            return Err(AixError::UsageNotLiteLlm.into());
        }
    }
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
    let client = LiteLlmAdminClient::with_timeout(
        base_url.expose_secret(),
        api_key.expose_secret(),
        timeout,
    );
    let response = client
        .daily_activity(
            &range.start.format("%Y-%m-%d").to_string(),
            &range.end.format("%Y-%m-%d").to_string(),
        )
        .await?;

    let report = report::UsageReport::from_response(response, range.start, range.end, model_filter);
    if json {
        output::print_json("usage", report)?;
    } else {
        report::print_human(&report, range.is_default);
    }
    Ok(())
}
