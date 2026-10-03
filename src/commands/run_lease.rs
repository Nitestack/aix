use crate::commands::launch::ResolvedRunLaunch;
use crate::commands::run::RunMetadata;
use crate::error::AixError;
use crate::gateway::LiteLlmAdminClient;
use crate::run_history::{LeaseCleanupStatus, RunLeaseRecord};
use crate::secrets::SecretString;
use color_eyre::Result;
use std::collections::BTreeSet;
use uuid::Uuid;

pub(crate) struct ActiveLease {
    pub(crate) client: LiteLlmAdminClient,
    pub(crate) key: SecretString,
    pub(crate) record: RunLeaseRecord,
}

pub(crate) fn validate_options(
    lease: bool,
    budget: Option<f64>,
    duration: Option<String>,
    requested_models: &[String],
    dry_run: bool,
) -> Result<(Option<f64>, Option<String>), AixError> {
    if !lease && (budget.is_some() || duration.is_some() || !requested_models.is_empty() || dry_run)
    {
        return Err(AixError::LeaseOptionsRequireLease);
    }
    if !lease {
        return Ok((None, None));
    }

    let budget = budget.ok_or(AixError::LeaseBudgetRequired)?;
    if !budget.is_finite() || budget <= 0.0 {
        return Err(AixError::InvalidLeaseBudget);
    }
    if requested_models.iter().any(|model| model.trim().is_empty()) {
        return Err(AixError::EmptyLeaseModel);
    }

    let duration = duration.unwrap_or_else(|| "2h".to_string());
    if !is_positive_litellm_duration(&duration) {
        return Err(AixError::InvalidLeaseDuration);
    }
    Ok((Some(budget), Some(duration)))
}

fn is_positive_litellm_duration(value: &str) -> bool {
    crate::duration::parse_litellm_duration(value).is_some()
}

pub(crate) fn inputs_contain_parent_key(
    resolved: &ResolvedRunLaunch,
    duration: &str,
    metadata: &RunMetadata,
    args: &[String],
) -> bool {
    let Some(parent_gateway) = resolved.parent_gateway.as_ref() else {
        return false;
    };
    let parent_key = parent_gateway.api_key.expose_secret();
    if parent_key.is_empty() {
        return false;
    }
    let contains_key = |value: &str| value.contains(parent_key);
    contains_key(duration)
        || metadata.name.as_deref().is_some_and(contains_key)
        || metadata.workflow.as_deref().is_some_and(contains_key)
        || metadata.task_id.as_deref().is_some_and(contains_key)
        || metadata.tags.iter().any(|tag| contains_key(tag))
        || args.iter().any(|arg| contains_key(arg))
        || contains_key(&resolved.program)
        || contains_key(&resolved.env.profile_name)
        || resolved
            .logical_tool_name
            .as_deref()
            .is_some_and(contains_key)
        || resolved
            .parent_gateway
            .as_ref()
            .is_some_and(|parent_gateway| contains_key(parent_gateway.base_url.expose_secret()))
        || resolved
            .allowed_models
            .iter()
            .any(|model| contains_key(model))
        || resolved.policy.as_ref().is_some_and(|policy| {
            contains_key(&policy.name)
                || contains_key(&policy.max_duration)
                || policy.tags.iter().any(|tag| contains_key(tag))
        })
}

pub(crate) fn print_dry_run(
    resolved: &ResolvedRunLaunch,
    budget: f64,
    duration: &str,
    metadata: &RunMetadata,
) {
    let run_id = Uuid::new_v4();
    eprintln!("Would create LiteLLM virtual-key lease:");
    if let Some(policy) = &resolved.policy {
        eprintln!("  policy: {}", policy.name);
    }
    eprintln!("  profile: {}", resolved.env.profile_name);
    eprintln!("  key alias: aix-run-{run_id}");
    eprintln!("  budget: ${budget:.2}");
    eprintln!("  duration: {duration}");
    if resolved.allowed_models.is_empty() {
        eprintln!("  allowed models: unrestricted by the lease");
    } else {
        eprintln!("  allowed models: {}", resolved.allowed_models.join(", "));
    }
    eprintln!("  run tag values: {}", metadata.tags.len());
    if resolved.policy.is_some() {
        let run_tag = format!("aix:run:{run_id}");
        let mut effective_tags = metadata.tags.clone();
        if !effective_tags.contains(&run_tag) {
            effective_tags.push(run_tag);
        }
        eprintln!("  effective tags: {effective_tags:?}");
    }
    eprintln!("Would launch the configured child command.");
    eprintln!("Would set variable names:");
    let mut names = BTreeSet::new();
    names.extend(resolved.env.vars.iter().map(|(name, _)| name.as_str()));
    names.extend(resolved.env.auth_vars.iter().map(|(name, _)| name.as_str()));
    names.extend(resolved.env.display_only_vars.iter().map(String::as_str));
    names.insert("AIX_RUN_ID");
    if resolved.policy.is_some() {
        names.insert("AIX_RUN_POLICY");
    }
    if metadata.name.is_some() {
        names.insert("AIX_RUN_NAME");
    }
    if metadata.workflow.is_some() {
        names.insert("AIX_WORKFLOW");
    }
    if metadata.task_id.is_some() {
        names.insert("AIX_TASK_ID");
    }
    if !metadata.tags.is_empty() || resolved.policy.is_some() {
        names.insert("AIX_RUN_TAGS");
    }
    for name in names {
        eprintln!("  {name}");
    }
}

pub(crate) async fn revoke_lease(
    client: &LiteLlmAdminClient,
    key: &SecretString,
) -> LeaseCleanupStatus {
    match client.delete_virtual_key(key.expose_secret()).await {
        Ok(()) => LeaseCleanupStatus::Revoked,
        Err(AixError::GatewayError {
            status: 404 | 410, ..
        })
        | Err(AixError::GatewayRequestFailed { status: 404 | 410 }) => {
            LeaseCleanupStatus::ExpiredOrUnverified
        }
        Err(_) => LeaseCleanupStatus::RevokeFailed,
    }
}

pub(crate) fn warn_unconfirmed_cleanup(alias: &str, status: &LeaseCleanupStatus) {
    let reason = match status {
        LeaseCleanupStatus::Revoked => return,
        LeaseCleanupStatus::ExpiredOrUnverified => "expired or already absent",
        LeaseCleanupStatus::RevokeFailed => "revocation failed",
    };
    eprintln!(
        "warning: lease {alias} cleanup not confirmed ({reason}); expiry remains the safety bound"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_parser_accepts_positive_finite_litellm_durations() {
        for duration in ["2h", "30m", "1d", "15s", "2w", "1mo"] {
            assert!(
                is_positive_litellm_duration(duration),
                "expected {duration:?} to be accepted"
            );
        }
    }

    #[test]
    fn duration_parser_rejects_zero_malformed_and_overflowing_durations() {
        for duration in [
            "",
            "0s",
            "forever",
            "1h ",
            "1.5h",
            "1h30m",
            "18446744073709551615d",
        ] {
            assert!(
                !is_positive_litellm_duration(duration),
                "expected {duration:?} to be rejected"
            );
        }
    }

    #[test]
    fn lease_arguments_require_positive_budget_and_lease_opt_in() {
        assert!(matches!(
            validate_options(true, None, None, &[], false),
            Err(AixError::LeaseBudgetRequired)
        ));
        assert!(matches!(
            validate_options(true, Some(0.0), None, &[], false),
            Err(AixError::InvalidLeaseBudget)
        ));
        assert!(matches!(
            validate_options(false, Some(1.0), None, &[], false),
            Err(AixError::LeaseOptionsRequireLease)
        ));
    }
}
