use crate::config::{self, Config, RunPolicy};
use crate::error::AixError;
use crate::output;
use color_eyre::Result;
use serde::Serialize;
use std::path::PathBuf;

pub fn list(config_path: Option<PathBuf>, json: bool) -> Result<()> {
    let cfg = load_config(config_path)?;
    let names = config::sorted_run_policy_names(&cfg);
    let summaries = names
        .iter()
        .map(|name| summary(&cfg, name, &cfg.run_policies[*name]))
        .collect::<Result<Vec<_>, _>>()?;

    if json {
        output::print_json("policies", summaries)?;
    } else if summaries.is_empty() {
        println!("(no run policies configured)");
    } else {
        for policy in &summaries {
            print_summary(policy);
        }
    }
    Ok(())
}

pub fn show(config_path: Option<PathBuf>, json: bool, name: &str) -> Result<()> {
    let cfg = load_config(config_path)?;
    let policy = cfg
        .run_policies
        .get(name)
        .ok_or_else(|| AixError::RunPolicyNotFound {
            name: name.to_string(),
            available_hint: available_policies(&cfg),
        })?;
    let summary = summary(&cfg, name, policy)?;

    if json {
        output::print_json("policy show", summary)?;
    } else {
        print_summary(&summary);
    }
    Ok(())
}

fn load_config(config_path: Option<PathBuf>) -> Result<Config, AixError> {
    let path = config::find_config_path(config_path.as_deref())?.ok_or(AixError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;
    Ok(cfg)
}

fn summary<'a>(
    cfg: &'a Config,
    name: &'a str,
    policy: &'a RunPolicy,
) -> Result<PolicySummary<'a>, AixError> {
    let selected_profile = policy.profile.as_deref().or(cfg.default_profile.as_deref());
    let resolved_models = policy
        .allowed_models
        .as_ref()
        .and_then(|models| selected_profile.map(|profile_name| (models, profile_name)))
        .map(|(models, profile_name)| {
            let profile = cfg
                .profiles
                .get(profile_name)
                .expect("config validation ensures fixed/default profiles exist");
            models
                .iter()
                .map(|model| config::resolve_model(Some(model), cfg, profile))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;

    Ok(PolicySummary {
        name,
        profile: policy.profile.as_deref(),
        max_budget: policy.max_budget,
        max_duration: &policy.max_duration,
        allowed_models: policy.allowed_models.as_deref(),
        resolved_models,
        tags: &policy.tags,
    })
}

#[derive(Serialize)]
struct PolicySummary<'a> {
    name: &'a str,
    profile: Option<&'a str>,
    max_budget: f64,
    max_duration: &'a str,
    allowed_models: Option<&'a [String]>,
    resolved_models: Option<Vec<String>>,
    tags: &'a [String],
}

fn print_summary(policy: &PolicySummary<'_>) {
    println!("{}", policy.name);
    println!(
        "  Profile: {}",
        policy
            .profile
            .map_or("normal resolution", |profile| profile)
    );
    println!("  Max budget: ${:.2}", policy.max_budget);
    println!("  Max duration: {}", policy.max_duration);
    match (&policy.allowed_models, &policy.resolved_models) {
        (Some(aliases), Some(resolved)) => {
            let models = aliases
                .iter()
                .zip(resolved)
                .map(|(alias, model)| {
                    if alias == model {
                        model.clone()
                    } else {
                        format!("{alias} -> {model}")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            println!("  Allowed models: {models}");
        }
        (Some(models), None) => println!("  Allowed models: {}", models.join(", ")),
        (None, _) => println!("  Allowed models: unrestricted by aix"),
    }
    println!("  Required tags: {:?}", policy.tags);
}

fn available_policies(cfg: &Config) -> String {
    let names = config::sorted_run_policy_names(cfg);
    if names.is_empty() {
        return "  (no run policies configured)".to_string();
    }
    names
        .into_iter()
        .map(|name| format!("  {name}"))
        .collect::<Vec<_>>()
        .join("\n")
}
