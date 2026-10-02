mod failure;
mod report;

use self::failure::{admin_failure, gateway_failure, safe_load_failure, safe_validation_failure};
use self::report::{print_human, DoctorOutput, DoctorReport};
use crate::cache::Cache;
use crate::commands::GatewayRequestOptions;
use crate::config::{self, CacheConfig, Config, Gateway, KnownGateway};
use crate::error::{AixError, DoctorFailureCategory};
use crate::gateway::{LiteLlmAdminClient, OpenAiClient};
use crate::output;
use color_eyre::Result;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub async fn run(
    options: GatewayRequestOptions,
    config_path: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    let GatewayRequestOptions { selection, timeout } = options;
    let report = diagnose(selection.profile, config_path, timeout).await;
    let failed = report.failure_category.is_some();
    if json {
        let data = DoctorOutput {
            checks: &report.checks,
        };
        if failed {
            let mut stderr = std::io::stderr().lock();
            output::print_json_to("doctor", data, &mut stderr)?;
        } else {
            output::print_json("doctor", data)?;
        }
    } else {
        if failed {
            let mut stderr = std::io::stderr().lock();
            print_human(&report.checks, &mut stderr)?;
        } else {
            let mut stdout = std::io::stdout().lock();
            print_human(&report.checks, &mut stdout)?;
        }
    }

    match report.failure_category {
        Some(category) => Err(AixError::DoctorChecksFailed { category }.into()),
        None => Ok(()),
    }
}

async fn diagnose(
    profile_arg: Option<String>,
    config_path: Option<PathBuf>,
    timeout: Duration,
) -> DoctorReport {
    let mut report = DoctorReport::new();

    let path = match config::find_config_path(config_path.as_deref()) {
        Ok(Some(path)) => {
            report.pass("config_discovery", "config file found", None);
            Some(path)
        }
        Ok(None) => {
            report.fail(
                "config_discovery",
                "no config file could be resolved",
                "pass --config <PATH> or set AIX_CONFIG",
                None,
                DoctorFailureCategory::Config,
            );
            None
        }
        Err(_) => {
            report.fail(
                "config_discovery",
                "config path resolution failed",
                "check --config and AIX_CONFIG",
                None,
                DoctorFailureCategory::Config,
            );
            None
        }
    };

    let mut config = None;
    let mut config_is_valid = false;
    let mut env_files_loaded = false;
    if let Some(path) = path {
        match config::load(&path) {
            Ok(loaded) => match config::validate(&loaded) {
                Ok(()) => {
                    config_is_valid = true;
                    config = Some(loaded);
                    if config::load_env_files(config.as_ref().expect("config was just set")).is_ok()
                    {
                        env_files_loaded = true;
                        report.pass(
                            "config_validation",
                            "config parsed, validated, and configured env files loaded",
                            None,
                        );
                    } else {
                        report.fail(
                            "config_validation",
                            "a configured environment file could not be loaded",
                            "check that each configured env_files path exists and is readable",
                            None,
                            DoctorFailureCategory::Config,
                        );
                    }
                }
                Err(error) => {
                    report.fail_with("config_validation", safe_validation_failure(&error), None)
                }
            },
            Err(error) => report.fail_with("config_validation", safe_load_failure(&error), None),
        }
    } else {
        report.skipped("config_validation", "no config file was discovered");
    }

    let selected_profile = if config_is_valid {
        let cfg = config.as_ref().expect("validated config is available");
        match resolve_profile_without_picker(profile_arg, cfg) {
            Some(name) => match cfg.profiles.get(&name) {
                Some(profile) => {
                    report.pass(
                        "profile_selection",
                        "profile resolved without an interactive picker",
                        None,
                    );
                    Some(profile)
                }
                None => {
                    report.fail(
                        "profile_selection",
                        "selected profile is not defined in the config",
                        "choose a name under [profiles] or set a valid default_profile",
                        None,
                        DoctorFailureCategory::Config,
                    );
                    None
                }
            },
            None => {
                report.fail(
                    "profile_selection",
                    "no profile can be resolved without an interactive picker",
                    "pass PROFILE or --profile, set AIX_PROFILE, or configure default_profile",
                    None,
                    DoctorFailureCategory::Config,
                );
                None
            }
        }
    } else {
        report.skipped(
            "profile_selection",
            "config parsing or structural validation did not pass",
        );
        None
    };

    let (api_key, base_url) = match (selected_profile, config.as_ref(), env_files_loaded) {
        (Some(profile), Some(cfg), true) => {
            let api_key = match profile.api_key.resolve() {
                Ok(value) if !value.expose_secret().trim().is_empty() => {
                    report.pass(
                        "api_key",
                        "API key source resolved to a non-empty value",
                        None,
                    );
                    Some(value)
                }
                Ok(_) => {
                    report.fail(
                        "api_key",
                        "API key source resolved to an empty value",
                        "check the selected profile's api_key source",
                        None,
                        DoctorFailureCategory::Secret,
                    );
                    None
                }
                Err(_) => {
                    report.fail(
                        "api_key",
                        "API key source could not be resolved",
                        "check the selected profile's api_key source",
                        None,
                        DoctorFailureCategory::Secret,
                    );
                    None
                }
            };

            let base_url = match config::resolve_base_url(profile, &cfg.endpoint) {
                Ok(value) if !value.expose_secret().trim().is_empty() => {
                    report.pass(
                        "base_url",
                        "base URL source resolved to a non-empty value",
                        None,
                    );
                    Some(value)
                }
                Ok(_) => {
                    report.fail(
                        "base_url",
                        "base URL source resolved to an empty value",
                        "check the profile base_url or endpoint.base_url source",
                        None,
                        DoctorFailureCategory::Secret,
                    );
                    None
                }
                Err(_) => {
                    report.fail(
                        "base_url",
                        "base URL source could not be resolved",
                        "check the profile base_url or endpoint.base_url source",
                        None,
                        DoctorFailureCategory::Secret,
                    );
                    None
                }
            };
            (api_key, base_url)
        }
        (Some(_), Some(_), false) => {
            report.skipped(
                "api_key",
                "configured env files did not load; secret sources may depend on them",
            );
            report.skipped(
                "base_url",
                "configured env files did not load; secret sources may depend on them",
            );
            (None, None)
        }
        _ => {
            report.skipped("api_key", "profile selection did not pass");
            report.skipped("base_url", "profile selection did not pass");
            (None, None)
        }
    };

    match (&api_key, &base_url) {
        (Some(api_key), Some(base_url)) => {
            let client = OpenAiClient::with_timeout(
                base_url.expose_secret(),
                api_key.expose_secret(),
                timeout,
            );
            let started = Instant::now();
            let response = client.models().await;
            let duration_ms = elapsed_ms(started.elapsed());
            match response {
                Ok(response) => {
                    report.pass(
                        "gateway_auth",
                        "authenticated OpenAI-compatible request succeeded",
                        Some(duration_ms),
                    );
                    match crate::commands::models::validate_response(response) {
                        Ok(()) => report.pass(
                            "model_discovery",
                            "GET /v1/models returned a parseable model list",
                            Some(duration_ms),
                        ),
                        Err(_) => report.fail(
                            "model_discovery",
                            "GET /v1/models did not return a valid model list",
                            "ensure the gateway returns a data array with a non-empty string id for each model",
                            Some(duration_ms),
                            DoctorFailureCategory::Network,
                        ),
                    }
                }
                Err(AixError::GatewayProtocolError(_)) => {
                    // The transport only reports this after receiving a successful HTTP status.
                    report.pass(
                        "gateway_auth",
                        "gateway accepted the authenticated request",
                        Some(duration_ms),
                    );
                    report.fail(
                        "model_discovery",
                        "GET /v1/models returned malformed JSON",
                        "ensure the gateway returns a valid OpenAI-compatible models response",
                        Some(duration_ms),
                        DoctorFailureCategory::Network,
                    );
                }
                Err(error) => {
                    report.fail_with("gateway_auth", gateway_failure(&error), Some(duration_ms));
                    report.skipped(
                        "model_discovery",
                        "the authenticated /v1/models request did not succeed",
                    );
                }
            }
        }
        _ => {
            report.skipped(
                "gateway_auth",
                "a non-empty API key and base URL are required",
            );
            report.skipped(
                "model_discovery",
                "the authenticated /v1/models request was skipped",
            );
        }
    }

    match (config.as_ref(), &api_key, &base_url) {
        (Some(cfg), Some(api_key), Some(base_url)) if config_is_valid && env_files_loaded => {
            if is_litellm_or_unset(cfg) {
                let started = Instant::now();
                let result = LiteLlmAdminClient::with_timeout(
                    base_url.expose_secret(),
                    api_key.expose_secret(),
                    timeout,
                )
                .user_info()
                .await;
                let duration_ms = elapsed_ms(started.elapsed());
                match result {
                    Ok(_) => report.pass(
                        "litellm_admin",
                        "LiteLLM management capability is reachable",
                        Some(duration_ms),
                    ),
                    Err(AixError::BudgetExceeded { .. }) => report.fail(
                        "litellm_admin",
                        "LiteLLM rejected the management request because the budget is exceeded",
                        "review the configured key's LiteLLM budget policy",
                        Some(duration_ms),
                        DoctorFailureCategory::Budget,
                    ),
                    Err(error) => {
                        report.fail_with("litellm_admin", admin_failure(&error), Some(duration_ms));
                    }
                }
            } else {
                report.not_applicable(
                    "litellm_admin",
                    "endpoint.gateway explicitly identifies a non-LiteLLM gateway",
                );
            }
        }
        (Some(cfg), _, _) if config_is_valid && !is_litellm_or_unset(cfg) => report.not_applicable(
            "litellm_admin",
            "endpoint.gateway explicitly identifies a non-LiteLLM gateway",
        ),
        _ => report.skipped(
            "litellm_admin",
            "a valid config, API key, and base URL are required",
        ),
    }

    let default_cache_config = CacheConfig::default();
    let cache_config = config
        .as_ref()
        .map(|cfg| &cfg.cache)
        .unwrap_or(&default_cache_config);
    if Cache::from_config(cache_config).check_writable().is_ok() {
        report.pass(
            "cache_directory",
            "cache directory is accessible and writable",
            None,
        );
    } else {
        report.fail(
            "cache_directory",
            "cache directory could not be created or written",
            "check permissions for AIX_CACHE_DIR or the platform cache directory",
            None,
            DoctorFailureCategory::Internal,
        );
    }

    report
}

fn resolve_profile_without_picker(profile: Option<String>, config: &Config) -> Option<String> {
    profile.or_else(|| config.default_profile.clone())
}

fn is_litellm_or_unset(config: &Config) -> bool {
    match &config.endpoint.gateway {
        None | Some(Gateway::Known(KnownGateway::Litellm)) => true,
        Some(Gateway::Custom(_)) => false,
    }
}

fn elapsed_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_resolution_does_not_pick_interactively() {
        let config: Config = toml::from_str(
            r#"
[endpoint]
base_url = "https://gateway.example"
[profiles.work]
api_key = "test-key"
"#,
        )
        .unwrap();

        assert_eq!(resolve_profile_without_picker(None, &config), None);
        assert_eq!(
            resolve_profile_without_picker(Some("other".to_string()), &config),
            Some("other".to_string())
        );
    }

    #[test]
    fn profile_resolution_uses_default_without_interactive_picker() {
        let config: Config = toml::from_str(
            r#"
default_profile = "work"
[endpoint]
base_url = "https://gateway.example"
[profiles.work]
api_key = "test-key"
"#,
        )
        .unwrap();

        assert_eq!(
            resolve_profile_without_picker(None, &config),
            Some("work".to_string())
        );
    }
}
