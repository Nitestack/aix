use crate::commands::launch::{self, ResolvedRunLaunch};
use crate::error::AixError;
use crate::gateway::LiteLlmAdminClient;
use crate::run_history::{
    LeaseCleanupStatus, RunLeaseRecord, RunRecord, RunStatus, RunStore, RUN_SCHEMA_VERSION,
};
use crate::secrets::SecretString;
use color_eyre::Result;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use std::time::Instant;
use uuid::Uuid;

struct ActiveLease {
    client: LiteLlmAdminClient,
    key: SecretString,
    record: RunLeaseRecord,
}

pub struct RunOptions {
    pub profile: Option<String>,
    pub config_path: Option<std::path::PathBuf>,
    pub name: Option<String>,
    pub workflow: Option<String>,
    pub task_id: Option<String>,
    pub tags: Vec<String>,
    pub lease: bool,
    pub budget: Option<f64>,
    pub duration: Option<String>,
    pub requested_models: Vec<String>,
    pub dry_run: bool,
    pub args: Vec<String>,
}

pub async fn run(options: RunOptions) -> Result<()> {
    let RunOptions {
        profile,
        config_path,
        name,
        workflow,
        task_id,
        tags,
        lease,
        budget,
        duration,
        requested_models,
        dry_run,
        args,
    } = options;
    let (budget, duration) =
        validate_lease_options(lease, budget, duration, &requested_models, dry_run)?;
    let (requested_program, command_args) = args.split_first().ok_or(AixError::RunNoCommand)?;
    let mut resolved = launch::resolve_run_launch(
        profile,
        config_path,
        requested_program,
        &requested_models,
        lease,
        dry_run,
    )?;
    if lease
        && lease_inputs_contain_parent_key(
            &resolved,
            duration.as_deref().expect("validated lease duration"),
            &name,
            &workflow,
            &task_id,
            &tags,
            &args,
        )
    {
        return Err(AixError::LeaseInputContainsCredential.into());
    }

    if dry_run {
        print_lease_dry_run(
            &resolved,
            budget.expect("validated lease budget"),
            duration.as_deref().expect("validated lease duration"),
            &tags,
            name.as_deref(),
            workflow.as_deref(),
            task_id.as_deref(),
        );
        return Ok(());
    }

    let run_id = Uuid::new_v4();
    let run_started = Instant::now();
    let started_at_unix_ms = RunRecord::now_unix_ms();
    let mut record = RunRecord {
        schema_version: RUN_SCHEMA_VERSION,
        run_id,
        name: name.clone(),
        workflow: workflow.clone(),
        task_id: task_id.clone(),
        tags: tags.clone(),
        profile: resolved.env.profile_name.clone(),
        logical_tool_name: resolved.logical_tool_name.clone(),
        executable_name: executable_name(&resolved.program),
        started_at_unix_ms,
        finished_at_unix_ms: None,
        duration_ms: None,
        process_exit_code: None,
        status: RunStatus::Running,
        lease: None,
    };

    let interrupt_requested = Arc::new(AtomicBool::new(false));
    let ctrlc_flag = Arc::clone(&interrupt_requested);
    ctrlc::set_handler(move || ctrlc_flag.store(true, Ordering::SeqCst))
        .map_err(|error| AixError::RunInterruptHandler(std::io::Error::other(error.to_string())))?;
    #[cfg(unix)]
    let terminated = {
        let flag = Arc::new(AtomicBool::new(false));
        signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&flag))
            .map_err(AixError::RunInterruptHandler)?;
        flag
    };
    #[cfg(not(unix))]
    let terminated = Arc::new(AtomicBool::new(false));

    let store = RunStore::from_environment()?;
    let mut handle = store.begin(&record)?;

    let mut active_lease = if lease {
        let budget = budget.expect("validated lease budget");
        let duration = duration.as_deref().expect("validated lease duration");
        let key_alias = format!("aix-run-{run_id}");
        let client = LiteLlmAdminClient::new(
            resolved.base_url.expose_secret(),
            resolved.parent_api_key.expose_secret(),
        );
        let mut metadata_tags = Vec::with_capacity(tags.len() + 1);
        metadata_tags.push(format!("aix:run:{run_id}"));
        metadata_tags.extend(tags.iter().cloned());

        let generated = client
            .generate_virtual_key(
                budget,
                duration,
                &key_alias,
                &resolved.allowed_models,
                &metadata_tags,
            )
            .await;
        let generated = match generated {
            Ok(generated) => generated,
            Err(error) => {
                finish_prelaunch_failure(&mut record, run_started.elapsed());
                if let Err(write_error) = store.write(&mut handle, &record) {
                    eprintln!("{write_error:?}");
                }
                return Err(error.into());
            }
        };

        let active = ActiveLease {
            client,
            key: generated.key,
            record: RunLeaseRecord {
                key_alias,
                budget,
                duration: duration.to_string(),
                expires_at: generated
                    .expires_at
                    .filter(|expiry| !expiry.contains(resolved.parent_api_key.expose_secret())),
                allowed_models: resolved.allowed_models.clone(),
                spend: None,
                cleanup_status: LeaseCleanupStatus::ExpiredOrUnverified,
            },
        };
        launch::apply_lease_credentials(
            &mut resolved.env,
            resolved.parent_api_key.expose_secret(),
            active.key.expose_secret(),
            resolved.base_url.expose_secret(),
        );
        record.lease = Some(active.record.clone());
        if let Err(error) = store.write(&mut handle, &record) {
            let status = revoke_lease(&active.client, &active.key).await;
            if status != LeaseCleanupStatus::Revoked {
                warn_unconfirmed_cleanup(&active.record.key_alias, &status);
            }
            return Err(error.into());
        }
        Some(active)
    } else {
        None
    };

    append_run_metadata(&mut resolved.env, run_id, name, workflow, task_id, tags);

    let child_result = launch::run_command_status_interruptible(
        &resolved.program,
        command_args,
        &resolved.env,
        &interrupt_requested,
        &terminated,
    );
    let (child_status, child_error) = match child_result {
        Ok(status) => (Some(status), None),
        Err(error) => {
            record.status = RunStatus::Failed;
            (None, Some(error))
        }
    };

    let interruption = child_status
        .as_ref()
        .and_then(|_| launch::interruption_reason(&interrupt_requested, &terminated));
    if let Some(status) = child_status.as_ref() {
        record.process_exit_code = status.code();
        record.status = if interruption.is_some() || status.code().is_none() {
            RunStatus::Interrupted
        } else if status.success() {
            RunStatus::Succeeded
        } else {
            RunStatus::Failed
        };
    }

    let mut cleanup_confirmed = true;
    if let Some(active) = active_lease.as_mut() {
        active.record.spend = active
            .client
            .virtual_key_spend(active.key.expose_secret())
            .await
            .ok()
            .flatten();
        active.record.cleanup_status = revoke_lease(&active.client, &active.key).await;
        cleanup_confirmed = active.record.cleanup_status == LeaseCleanupStatus::Revoked;
        record.lease = Some(active.record.clone());
        if !cleanup_confirmed {
            warn_unconfirmed_cleanup(&active.record.key_alias, &active.record.cleanup_status);
        }
    }

    record.finished_at_unix_ms = Some(RunRecord::now_unix_ms());
    record.duration_ms = Some(
        run_started
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
    );

    if let Err(error) = store.write(&mut handle, &record) {
        eprintln!("{error:?}");
        if let Some(interruption) = interruption {
            std::process::exit(interruption.exit_code());
        }
        if let Some(exit_code) = child_status
            .as_ref()
            .and_then(std::process::ExitStatus::code)
            .filter(|code| *code != 0)
        {
            std::process::exit(exit_code);
        }
        if let Some(error) = child_error {
            return Err(error.into());
        }
        return Err(error.into());
    }

    if let Some(interruption) = interruption {
        std::process::exit(interruption.exit_code());
    }
    if let Some(status) = child_status {
        if let Some(exit_code) = status.code() {
            if exit_code != 0 {
                std::process::exit(exit_code);
            }
        } else {
            std::process::exit(1);
        }
    } else if let Some(error) = child_error {
        return Err(error.into());
    }

    if !cleanup_confirmed {
        return Err(AixError::LeaseCleanupFailed.into());
    }

    Ok(())
}

fn validate_lease_options(
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
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > 128 {
        return false;
    }

    let digit_end = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digit_end == 0 || digit_end == bytes.len() {
        return false;
    }
    let Ok(amount) = value[..digit_end].parse::<u64>() else {
        return false;
    };
    if amount == 0 {
        return false;
    }

    let unit = &value[digit_end..];
    let maximum_seconds = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        "w" => 7 * 24 * 60 * 60,
        // LiteLLM computes calendar months when it turns this into an expiry.
        // A 31-day upper bound keeps that amount finite for validation.
        "mo" => 31 * 24 * 60 * 60,
        _ => return false,
    };
    amount.checked_mul(maximum_seconds).is_some()
}

fn lease_inputs_contain_parent_key(
    resolved: &ResolvedRunLaunch,
    duration: &str,
    name: &Option<String>,
    workflow: &Option<String>,
    task_id: &Option<String>,
    tags: &[String],
    args: &[String],
) -> bool {
    let parent_key = resolved.parent_api_key.expose_secret();
    if parent_key.is_empty() {
        return false;
    }
    let contains_key = |value: &str| value.contains(parent_key);
    contains_key(duration)
        || name.as_deref().is_some_and(contains_key)
        || workflow.as_deref().is_some_and(contains_key)
        || task_id.as_deref().is_some_and(contains_key)
        || tags.iter().any(|tag| contains_key(tag))
        || args.iter().any(|arg| contains_key(arg))
        || contains_key(&resolved.program)
        || contains_key(&resolved.env.profile_name)
        || resolved
            .logical_tool_name
            .as_deref()
            .is_some_and(contains_key)
        || contains_key(resolved.base_url.expose_secret())
        || resolved
            .allowed_models
            .iter()
            .any(|model| contains_key(model))
}

fn print_lease_dry_run(
    resolved: &ResolvedRunLaunch,
    budget: f64,
    duration: &str,
    tags: &[String],
    name: Option<&str>,
    workflow: Option<&str>,
    task_id: Option<&str>,
) {
    let run_id = Uuid::new_v4();
    eprintln!("Would create LiteLLM virtual-key lease:");
    eprintln!("  key alias: aix-run-{run_id}");
    eprintln!("  budget: ${budget:.2}");
    eprintln!("  duration: {duration}");
    if resolved.allowed_models.is_empty() {
        eprintln!("  allowed models: unrestricted by the lease");
    } else {
        eprintln!("  allowed models: {}", resolved.allowed_models.join(", "));
    }
    eprintln!("  user tags: configured ({} total)", tags.len());
    eprintln!("Would launch the configured child command.");
    eprintln!("Would set variable names:");
    let mut names = BTreeSet::new();
    names.extend(resolved.env.vars.iter().map(|(name, _)| name.as_str()));
    names.extend(resolved.env.display_only_vars.iter().map(String::as_str));
    names.insert("AIX_RUN_ID");
    if name.is_some() {
        names.insert("AIX_RUN_NAME");
    }
    if workflow.is_some() {
        names.insert("AIX_WORKFLOW");
    }
    if task_id.is_some() {
        names.insert("AIX_TASK_ID");
    }
    if !tags.is_empty() {
        names.insert("AIX_RUN_TAGS");
    }
    for name in names {
        eprintln!("  {name}");
    }
}

fn append_run_metadata(
    env: &mut launch::LaunchEnv,
    run_id: Uuid,
    name: Option<String>,
    workflow: Option<String>,
    task_id: Option<String>,
    tags: Vec<String>,
) {
    env.vars
        .push(("AIX_RUN_ID".to_string(), run_id.to_string()));
    if let Some(value) = name {
        env.vars.push(("AIX_RUN_NAME".to_string(), value));
    } else {
        env.remove_vars.push("AIX_RUN_NAME".to_string());
    }
    if let Some(value) = workflow {
        env.vars.push(("AIX_WORKFLOW".to_string(), value));
    } else {
        env.remove_vars.push("AIX_WORKFLOW".to_string());
    }
    if let Some(value) = task_id {
        env.vars.push(("AIX_TASK_ID".to_string(), value));
    } else {
        env.remove_vars.push("AIX_TASK_ID".to_string());
    }
    if !tags.is_empty() {
        env.vars.push((
            "AIX_RUN_TAGS".to_string(),
            serde_json::to_string(&tags).expect("run tags are JSON strings"),
        ));
    } else {
        env.remove_vars.push("AIX_RUN_TAGS".to_string());
    }
}

fn finish_prelaunch_failure(record: &mut RunRecord, duration: Duration) {
    record.finished_at_unix_ms = Some(RunRecord::now_unix_ms());
    record.duration_ms = Some(duration.as_millis().try_into().unwrap_or(u64::MAX));
    record.status = RunStatus::Failed;
}

async fn revoke_lease(client: &LiteLlmAdminClient, key: &SecretString) -> LeaseCleanupStatus {
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

fn warn_unconfirmed_cleanup(alias: &str, status: &LeaseCleanupStatus) {
    let reason = match status {
        LeaseCleanupStatus::Revoked => return,
        LeaseCleanupStatus::ExpiredOrUnverified => "expired or already absent",
        LeaseCleanupStatus::RevokeFailed => "revocation failed",
    };
    eprintln!(
        "warning: lease {alias} cleanup not confirmed ({reason}); expiry remains the safety bound"
    );
}

fn executable_name(program: &str) -> String {
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
        .to_string()
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
            validate_lease_options(true, None, None, &[], false),
            Err(AixError::LeaseBudgetRequired)
        ));
        assert!(matches!(
            validate_lease_options(true, Some(0.0), None, &[], false),
            Err(AixError::InvalidLeaseBudget)
        ));
        assert!(matches!(
            validate_lease_options(false, Some(1.0), None, &[], false),
            Err(AixError::LeaseOptionsRequireLease)
        ));
    }
}
