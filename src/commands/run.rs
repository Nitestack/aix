use crate::commands::launch;
use crate::commands::run_lease;
use crate::commands::ProfileSelection;
use crate::error::AixError;
use crate::gateway::LiteLlmAdminClient;
use crate::run_history::{
    LeaseCleanupStatus, RunLeaseRecord, RunRecord, RunStatus, RunStore, RUN_SCHEMA_VERSION,
};
use color_eyre::Result;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;
use uuid::Uuid;

pub(crate) struct RunMetadata {
    pub name: Option<String>,
    pub workflow: Option<String>,
    pub task_id: Option<String>,
    pub tags: Vec<String>,
}

pub(crate) struct RunOptions {
    pub selection: ProfileSelection,
    pub timeout: std::time::Duration,
    pub config_path: Option<std::path::PathBuf>,
    pub metadata: RunMetadata,
    pub lease: bool,
    pub budget: Option<f64>,
    pub duration: Option<String>,
    pub requested_models: Vec<String>,
    pub dry_run: bool,
    pub args: Vec<String>,
}

pub async fn run(options: RunOptions) -> Result<()> {
    let RunOptions {
        selection,
        timeout,
        config_path,
        metadata,
        lease,
        budget,
        duration,
        requested_models,
        dry_run,
        args,
    } = options;
    let (budget, duration) =
        run_lease::validate_options(lease, budget, duration, &requested_models, dry_run)?;
    let (requested_program, command_args) = args.split_first().ok_or(AixError::RunNoCommand)?;
    let mut resolved = launch::resolve_run_launch(
        selection,
        config_path,
        requested_program,
        &requested_models,
        lease,
        dry_run,
    )?;
    if lease
        && run_lease::inputs_contain_parent_key(
            &resolved,
            duration.as_deref().expect("validated lease duration"),
            &metadata,
            &args,
        )
    {
        return Err(AixError::LeaseInputContainsCredential.into());
    }

    if dry_run {
        launch::validate_executable(&resolved.program)?;
        run_lease::print_dry_run(
            &resolved,
            budget.expect("validated lease budget"),
            duration.as_deref().expect("validated lease duration"),
            &metadata,
        );
        return Ok(());
    }
    let run_id = Uuid::new_v4();
    let started_at_unix_ms = RunRecord::now_unix_ms();
    let mut record = RunRecord {
        schema_version: RUN_SCHEMA_VERSION,
        run_id,
        name: metadata.name.clone(),
        workflow: metadata.workflow.clone(),
        task_id: metadata.task_id.clone(),
        tags: metadata.tags.clone(),
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
        let client = LiteLlmAdminClient::with_timeout(
            resolved.base_url.expose_secret(),
            resolved.parent_api_key.expose_secret(),
            timeout,
        );
        let mut metadata_tags = Vec::with_capacity(metadata.tags.len() + 1);
        metadata_tags.push(format!("aix:run:{run_id}"));
        metadata_tags.extend(metadata.tags.iter().cloned());

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
                finish_prelaunch_failure(&mut record);
                if let Err(write_error) = store.write(&mut handle, &record) {
                    eprintln!("{write_error:?}");
                }
                return Err(error.into());
            }
        };

        let parent_key = resolved.parent_api_key.expose_secret();
        let mut lease_record = RunLeaseRecord {
            key_alias,
            budget,
            duration: duration.to_string(),
            expires_at: generated
                .expires_at
                .filter(|expiry| !expiry.contains(parent_key)),
            allowed_models: resolved.allowed_models.clone(),
            spend: None,
            cleanup_status: LeaseCleanupStatus::ExpiredOrUnverified,
        };
        if !parent_key.is_empty() && generated.key.expose_secret().contains(parent_key) {
            lease_record.cleanup_status = LeaseCleanupStatus::RevokeFailed;
            run_lease::warn_unconfirmed_cleanup(
                &lease_record.key_alias,
                &lease_record.cleanup_status,
            );
            record.lease = Some(lease_record);
            finish_prelaunch_failure(&mut record);
            if let Err(write_error) = store.write(&mut handle, &record) {
                eprintln!("{write_error:?}");
            }
            return Err(AixError::LeaseKeyContainsParentCredential.into());
        }

        let active = run_lease::ActiveLease {
            client,
            key: generated.key,
            record: lease_record,
        };
        launch::apply_lease_credentials(
            &mut resolved.env,
            resolved.parent_api_key.expose_secret(),
            active.key.expose_secret(),
            resolved.base_url.expose_secret(),
        );
        record.lease = Some(active.record.clone());
        if let Err(error) = store.write(&mut handle, &record) {
            let status = run_lease::revoke_lease(&active.client, &active.key).await;
            if status != LeaseCleanupStatus::Revoked {
                run_lease::warn_unconfirmed_cleanup(&active.record.key_alias, &status);
            }
            return Err(error.into());
        }
        Some(active)
    } else {
        None
    };

    append_run_metadata(&mut resolved.env, run_id, metadata);

    let started = Instant::now();
    let child_result = launch::run_command_status_interruptible(
        &resolved.program,
        command_args,
        &resolved.env,
        &interrupt_requested,
        &terminated,
    );
    record.finished_at_unix_ms = Some(RunRecord::now_unix_ms());
    record.duration_ms = Some(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX));
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
        active.record.cleanup_status = run_lease::revoke_lease(&active.client, &active.key).await;
        cleanup_confirmed = active.record.cleanup_status == LeaseCleanupStatus::Revoked;
        record.lease = Some(active.record.clone());
        if !cleanup_confirmed {
            run_lease::warn_unconfirmed_cleanup(
                &active.record.key_alias,
                &active.record.cleanup_status,
            );
        }
    }

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

fn append_run_metadata(env: &mut launch::LaunchEnv, run_id: Uuid, metadata: RunMetadata) {
    env.vars
        .push(("AIX_RUN_ID".to_string(), run_id.to_string()));
    if let Some(value) = metadata.name {
        env.vars.push(("AIX_RUN_NAME".to_string(), value));
    } else {
        env.remove_vars.push("AIX_RUN_NAME".to_string());
    }
    if let Some(value) = metadata.workflow {
        env.vars.push(("AIX_WORKFLOW".to_string(), value));
    } else {
        env.remove_vars.push("AIX_WORKFLOW".to_string());
    }
    if let Some(value) = metadata.task_id {
        env.vars.push(("AIX_TASK_ID".to_string(), value));
    } else {
        env.remove_vars.push("AIX_TASK_ID".to_string());
    }
    if !metadata.tags.is_empty() {
        env.vars.push((
            "AIX_RUN_TAGS".to_string(),
            serde_json::to_string(&metadata.tags).expect("run tags are JSON strings"),
        ));
    } else {
        env.remove_vars.push("AIX_RUN_TAGS".to_string());
    }
}

fn finish_prelaunch_failure(record: &mut RunRecord) {
    record.finished_at_unix_ms = Some(RunRecord::now_unix_ms());
    record.duration_ms = Some(0);
    record.status = RunStatus::Failed;
}

fn executable_name(program: &str) -> String {
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
        .to_string()
}
