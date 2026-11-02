mod local;
mod range;

use crate::cli::UsageRangeArgs;
use crate::commands::env::resolve_profile;
use crate::commands::GatewayRequestOptions;
use crate::config;
use crate::error::AixError;
use crate::output;
use crate::usage_store::{UsageEventFilter, UsageStore};
use chrono::Local;
use color_eyre::Result;
use std::path::PathBuf;

pub struct UsageOptions {
    pub request: GatewayRequestOptions,
    pub config_path: Option<PathBuf>,
    pub json: bool,
    pub date_range: UsageRangeArgs,
    pub model_filter: Option<String>,
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
        protocol_filter,
        tool_filter,
        run_filter,
    } = command;
    let GatewayRequestOptions { selection, .. } = request;
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
    let local_report = local::LocalUsageReport::from_events(
        events,
        local::LocalUsageContext {
            start_date: range.start,
            end_date: range.end,
            profile: profile_name,
            is_chatgpt: profile.auth.local_usage_cost_is_not_applicable(),
            storage_unavailable,
            scan_incomplete,
            model_filter,
        },
    );

    if json {
        output::print_json("usage", local_report)?;
    } else {
        local_report.print_human();
    }
    Ok(())
}
