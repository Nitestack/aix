mod report;

use crate::commands::env::resolve_profile;
use crate::commands::gate::report::GateReport;
use crate::commands::ProfileSelection;
use crate::config::{self, Config};
use crate::error::{AixError, GateFailureCategory};
use crate::gateway::OpenAiClient;
use color_eyre::Result;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

pub(crate) struct GateOptions {
    pub(crate) selection: ProfileSelection,
    pub(crate) explicit_profile: Option<String>,
    pub(crate) timeout: Duration,
    pub(crate) config_path: Option<PathBuf>,
    pub(crate) policy: String,
    pub(crate) json: bool,
}

pub(crate) async fn run(options: GateOptions) -> Result<()> {
    let preflight = preflight(options).await;
    let failure_category = preflight.report.failure_category;
    report::print_report(&preflight.report, preflight.json)?;

    match failure_category {
        Some(category) => Err(AixError::GateChecksFailed { category }.into()),
        None => Ok(()),
    }
}

struct PreflightReport {
    report: GateReport,
    json: bool,
}

enum ModelDiscovery {
    Available(Vec<String>),
    Configured(Vec<String>),
    Invalid,
    Unavailable,
}

enum ModelRequirement {
    PolicyUnavailable,
    Unrestricted,
    ProfileUnavailable,
    Required(Vec<String>),
}

async fn preflight(options: GateOptions) -> PreflightReport {
    let GateOptions {
        selection,
        explicit_profile,
        timeout,
        config_path,
        policy: policy_name,
        json: json_output,
    } = options;
    let mut report = GateReport::new();

    let config_path = match config::find_config_path(config_path.as_deref()) {
        Ok(Some(path)) => path,
        Ok(None) => {
            report.fail(
                "config",
                "no config file could be resolved",
                None,
                GateFailureCategory::Config,
            );
            skip_after_config(&mut report);
            return PreflightReport {
                report,
                json: json_output,
            };
        }
        Err(_) => {
            report.fail(
                "config",
                "config path resolution failed",
                None,
                GateFailureCategory::Config,
            );
            skip_after_config(&mut report);
            return PreflightReport {
                report,
                json: json_output,
            };
        }
    };

    let cfg = match config::load(&config_path) {
        Ok(cfg) => cfg,
        Err(_) => {
            report.fail(
                "config",
                "config file could not be loaded",
                None,
                GateFailureCategory::Config,
            );
            skip_after_config(&mut report);
            return PreflightReport {
                report,
                json: json_output,
            };
        }
    };
    if config::validate(&cfg).is_err() {
        report.fail(
            "config",
            "config failed structural validation",
            None,
            GateFailureCategory::Config,
        );
        skip_after_config(&mut report);
        return PreflightReport {
            report,
            json: json_output,
        };
    }
    if config::load_env_files(&cfg).is_err() {
        report.fail(
            "config",
            "a configured environment file could not be loaded",
            None,
            GateFailureCategory::Config,
        );
        skip_after_config(&mut report);
        return PreflightReport {
            report,
            json: json_output,
        };
    }
    report.pass(
        "config",
        "config loaded, validated, and env files loaded",
        None,
    );

    let policy = cfg.run_policies.get(&policy_name);
    if policy.is_some() {
        report.pass("policy", "named run policy exists", None);
    } else {
        report.fail(
            "policy",
            "named run policy is not defined in the config",
            Some(json!({ "name": policy_name })),
            GateFailureCategory::Config,
        );
    }

    let policy_profile = policy.and_then(|policy| policy.profile.clone());
    let profile_conflict = policy_profile
        .as_deref()
        .zip(explicit_profile.as_deref())
        .is_some_and(|(policy_profile, selected)| policy_profile != selected);
    let selected_profile = policy_profile.or(selection.profile);
    let resolved_profile = resolve_profile(
        ProfileSelection {
            profile: selected_profile,
            non_interactive: true,
        },
        &cfg,
    )
    .ok()
    .and_then(|name| cfg.profiles.get(&name).map(|profile| (name, profile)));

    if let Some((profile_name, _)) = &resolved_profile {
        if profile_conflict {
            report.fail(
                "profile",
                "selected profile conflicts with the policy's fixed profile",
                Some(json!({ "effective_profile": profile_name })),
                GateFailureCategory::Config,
            );
        } else {
            report.pass(
                "profile",
                "effective profile resolved without prompting",
                Some(json!({ "name": profile_name })),
            );
        }
    } else {
        report.fail(
            "profile",
            "effective profile could not be resolved without prompting",
            None,
            GateFailureCategory::Config,
        );
    }

    let chatgpt_profile = resolved_profile
        .as_ref()
        .is_some_and(|(_, profile)| profile.auth.is_chatgpt());
    let (api_key, base_url) = match &resolved_profile {
        Some((profile_name, profile)) if profile.auth.is_chatgpt() => {
            let auth_status = crate::auth::AuthService::new(timeout)
                .and_then(|service| service.status(profile_name, &profile.auth));
            match auth_status {
                Ok(status)
                    if status.connected && status.chatgpt_plan_usage_enabled == Some(true) =>
                {
                    report.pass(
                        "credentials",
                        "ChatGPT profile is connected with plan usage enabled",
                        Some(json!({
                            "auth_type": "chatgpt",
                            "state": status.state,
                            "connected": status.connected,
                            "plan_usage_enabled": status.chatgpt_plan_usage_enabled,
                        })),
                    );
                }
                Ok(status) => report.fail(
                    "credentials",
                    "ChatGPT profile is signed out or does not have plan usage enabled",
                    Some(json!({
                        "auth_type": "chatgpt",
                        "state": status.state,
                        "connected": status.connected,
                        "plan_usage_enabled": status.chatgpt_plan_usage_enabled,
                    })),
                    GateFailureCategory::Authentication,
                ),
                Err(_) => report.fail(
                    "credentials",
                    "ChatGPT authentication status could not be read",
                    Some(json!({ "auth_type": "chatgpt" })),
                    GateFailureCategory::Secret,
                ),
            }
            (None, None)
        }
        Some((_, profile)) => {
            let api_key = profile
                .resolve_api_key()
                .ok()
                .filter(|value| !value.expose_secret().trim().is_empty());
            let base_url = config::resolve_base_url(profile, &cfg.endpoint)
                .ok()
                .filter(|value| !value.expose_secret().trim().is_empty());
            let details = json!({
                "api_key": if api_key.is_some() { "resolved" } else { "unavailable" },
                "base_url": if base_url.is_some() { "resolved" } else { "unavailable" }
            });
            if api_key.is_some() && base_url.is_some() {
                report.pass(
                    "credentials",
                    "profile API key and base URL resolved",
                    Some(details),
                );
            } else {
                report.fail(
                    "credentials",
                    "profile API key or base URL could not be resolved",
                    Some(details),
                    GateFailureCategory::Secret,
                );
            }
            (api_key, base_url)
        }
        None => {
            report.skipped("credentials", "effective profile did not resolve");
            (None, None)
        }
    };

    let model_requirement = match (policy, resolved_profile.as_ref()) {
        (None, _) => ModelRequirement::PolicyUnavailable,
        (Some(policy), _) if policy.allowed_models.is_none() => ModelRequirement::Unrestricted,
        (Some(_), None) => ModelRequirement::ProfileUnavailable,
        (Some(policy), Some((_, profile))) => {
            let models = policy
                .allowed_models
                .as_ref()
                .expect("unrestricted policies returned above");
            let mut resolved = Vec::with_capacity(models.len());
            for model in models {
                let model = config::resolve_model(Some(model), &cfg, profile)
                    .expect("validated model names always resolve");
                if !resolved.contains(&model) {
                    resolved.push(model);
                }
            }
            ModelRequirement::Required(resolved)
        }
    };

    let local_model_enforcement_required =
        policy.is_some_and(|policy| policy.allowed_models.is_some());
    let chatgpt_model_enforcement_required =
        policy.is_some_and(|policy| policy.allowed_models.is_some());
    let discovered_models = if chatgpt_profile {
        if has_chatgpt_local_responses_adapter(&cfg) {
            report.pass(
                "gateway",
                "configured ChatGPT local Responses adapter is available for local policy enforcement",
                None,
            );
        } else if chatgpt_model_enforcement_required {
            report.fail(
                "gateway",
                "ChatGPT run policies require a configured local Responses adapter",
                None,
                GateFailureCategory::Config,
            );
        } else {
            report.not_applicable(
                "gateway",
                "no local model restriction requires a specific ChatGPT tool path",
            );
        }
        ModelDiscovery::Available(
            crate::commands::launch::chatgpt_model_ids()
                .iter()
                .map(|model| (*model).to_string())
                .collect(),
        )
    } else if local_model_enforcement_required {
        if api_key.is_some() && base_url.is_some() {
            report.pass(
                "gateway",
                "profile credentials resolve for local request-time model enforcement",
                None,
            );
            match &model_requirement {
                ModelRequirement::Required(required) => {
                    ModelDiscovery::Configured(required.clone())
                }
                _ => ModelDiscovery::Unavailable,
            }
        } else {
            report.skipped("gateway", "profile credentials did not resolve");
            ModelDiscovery::Unavailable
        }
    } else {
        match (&api_key, &base_url) {
            (Some(api_key), Some(base_url)) => {
                match OpenAiClient::with_timeout(
                    base_url.expose_secret(),
                    api_key.expose_secret(),
                    timeout,
                )
                .models()
                .await
                {
                    Ok(response) => {
                        report.pass(
                            "gateway",
                            "authenticated model-discovery request reached the gateway",
                            None,
                        );
                        match crate::commands::models::extract_models(response) {
                            Ok(models) => ModelDiscovery::Available(
                                models.into_iter().map(|model| model.id).collect(),
                            ),
                            Err(_) => ModelDiscovery::Invalid,
                        }
                    }
                    Err(AixError::GatewayProtocolError(_)) => {
                        report.pass(
                            "gateway",
                            "authenticated model-discovery request reached the gateway",
                            None,
                        );
                        ModelDiscovery::Invalid
                    }
                    Err(error) => {
                        let (category, message, details) = gateway_failure(&error);
                        report.fail("gateway", message, details, category);
                        ModelDiscovery::Unavailable
                    }
                }
            }
            _ => {
                report.skipped("gateway", "profile credentials did not resolve");
                ModelDiscovery::Unavailable
            }
        }
    };

    match &model_requirement {
        ModelRequirement::PolicyUnavailable => {
            report.skipped("models", "named run policy did not resolve")
        }
        ModelRequirement::Unrestricted => {
            report.not_applicable("models", "policy does not restrict the allowed model set")
        }
        ModelRequirement::ProfileUnavailable => {
            report.skipped("models", "effective profile did not resolve")
        }
        ModelRequirement::Required(required) => match &discovered_models {
            ModelDiscovery::Unavailable => {
                report.skipped("models", "gateway request could not be completed")
            }
            ModelDiscovery::Invalid => report.fail(
                "models",
                "gateway returned an invalid model-discovery response",
                Some(json!({ "required": required })),
                GateFailureCategory::Network,
            ),
            ModelDiscovery::Available(available) | ModelDiscovery::Configured(available) => {
                let missing: Vec<_> = required
                    .iter()
                    .filter(|model| !available.contains(model))
                    .cloned()
                    .collect();
                let details = json!({ "required": required, "missing": missing });
                if missing.is_empty() {
                    match &discovered_models {
                        ModelDiscovery::Configured(_) => report.pass(
                            "models",
                            format!(
                                "{} configured model ID(s) resolve for local request-time enforcement",
                                required.len()
                            ),
                            Some(json!({ "required": required })),
                        ),
                        _ => report.pass(
                            "models",
                            format!("{} required model(s) are available", required.len()),
                            Some(details),
                        ),
                    }
                } else {
                    report.fail(
                        "models",
                        format!("{} required model(s) are unavailable", missing.len()),
                        Some(details),
                        GateFailureCategory::Budget,
                    );
                }
            }
        },
    }

    match policy {
        None => report.skipped("duration", "named run policy did not resolve"),
        Some(policy) => {
            let can_set_deadline = crate::duration::to_std_duration(&policy.max_duration)
                .is_some_and(|duration| std::time::Instant::now().checked_add(duration).is_some());
            if can_set_deadline {
                report.pass(
                    "duration",
                    "managed run process and local gateway can enforce the maximum duration",
                    Some(json!({ "max_duration": policy.max_duration })),
                );
            } else {
                report.fail(
                    "duration",
                    "maximum duration cannot be enforced by the managed run process",
                    None,
                    GateFailureCategory::Config,
                );
            }
        }
    }

    if !local_model_enforcement_required {
        report.not_applicable(
            "local_gateway",
            "policy does not require local allowed-model enforcement",
        );
    } else if chatgpt_profile {
        if has_chatgpt_local_responses_adapter(&cfg) {
            report.pass(
                "local_gateway",
                "configured ChatGPT local Responses adapter can enforce allowed models",
                None,
            );
        } else {
            report.fail(
                "local_gateway",
                "no configured ChatGPT local Responses adapter can enforce allowed models",
                None,
                GateFailureCategory::Config,
            );
        }
    } else if has_api_key_local_gateway_tool(&cfg) {
        report.pass(
            "local_gateway",
            "a configured local-gateway tool can enforce allowed models at request time",
            None,
        );
    } else {
        report.fail(
            "local_gateway",
            "allowed-model policy requires a configured local-gateway tool",
            None,
            GateFailureCategory::Config,
        );
    }

    PreflightReport {
        report,
        json: json_output,
    }
}

fn skip_after_config(report: &mut GateReport) {
    report.skipped("policy", "config did not pass");
    report.skipped("profile", "config did not pass");
    report.skipped("credentials", "config did not pass");
    report.skipped("gateway", "config did not pass");
    report.skipped("models", "config did not pass");
    report.skipped("duration", "config did not pass");
    report.skipped("local_gateway", "config did not pass");
}

fn has_chatgpt_local_responses_adapter(config: &Config) -> bool {
    config
        .tools
        .iter()
        .any(|(name, tool)| tool.chatgpt_responses_adapter(name).is_some())
}

fn has_api_key_local_gateway_tool(config: &Config) -> bool {
    config.tools.values().any(|tool| {
        tool.local_gateway
            && (tool.api_format.supports_openai() || tool.api_format.supports_anthropic())
    })
}

fn gateway_failure(error: &AixError) -> (GateFailureCategory, String, Option<Value>) {
    match error {
        AixError::GatewayError { status, .. } | AixError::GatewayRequestFailed { status } => {
            if matches!(status, 401 | 403) {
                (
                    GateFailureCategory::Authentication,
                    format!("gateway rejected authentication (HTTP {status})"),
                    Some(json!({ "http_status": status })),
                )
            } else {
                (
                    if *status == 402 {
                        GateFailureCategory::Budget
                    } else {
                        GateFailureCategory::Network
                    },
                    format!("gateway returned HTTP {status}"),
                    Some(json!({ "http_status": status })),
                )
            }
        }
        AixError::HttpError(error) if error.is_timeout() => (
            GateFailureCategory::Network,
            "gateway model-discovery request timed out".to_string(),
            None,
        ),
        _ => (
            GateFailureCategory::Network,
            "gateway model-discovery request failed".to_string(),
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_rejection_from_gateway_retains_budget_exit_category() {
        let (category, message, _) =
            gateway_failure(&AixError::GatewayRequestFailed { status: 402 });
        assert!(matches!(category, GateFailureCategory::Budget));
        assert!(message.contains("402"));
    }
}
