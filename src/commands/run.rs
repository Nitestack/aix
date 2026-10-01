use crate::commands::launch;
use crate::error::AixError;
use crate::run_history::{RunRecord, RunStatus, RunStore, RUN_SCHEMA_VERSION};
use color_eyre::Result;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;
use uuid::Uuid;

pub fn run(
    profile: Option<String>,
    config_path: Option<std::path::PathBuf>,
    name: Option<String>,
    workflow: Option<String>,
    task_id: Option<String>,
    tags: Vec<String>,
    args: Vec<String>,
) -> Result<()> {
    let (requested_program, command_args) = args.split_first().ok_or(AixError::RunNoCommand)?;
    let resolved = launch::resolve_run_launch(profile, config_path, requested_program)?;
    let executable_name = executable_name(&resolved.program);
    let run_id = Uuid::new_v4();
    let started_at_unix_ms = RunRecord::now_unix_ms();
    let mut record = RunRecord {
        schema_version: RUN_SCHEMA_VERSION,
        run_id,
        name: name.clone(),
        workflow: workflow.clone(),
        task_id: task_id.clone(),
        tags: tags.clone(),
        profile: resolved.env.profile_name.clone(),
        logical_tool_name: resolved.logical_tool_name,
        executable_name,
        started_at_unix_ms,
        finished_at_unix_ms: None,
        duration_ms: None,
        process_exit_code: None,
        status: RunStatus::Running,
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

    let mut env = resolved.env;
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
        env.vars
            .push(("AIX_RUN_TAGS".to_string(), serde_json::to_string(&tags)?));
    } else {
        env.remove_vars.push("AIX_RUN_TAGS".to_string());
    }

    let started = Instant::now();
    let child_result = launch::run_command_status_interruptible(
        &resolved.program,
        command_args,
        &env,
        &interrupt_requested,
        &terminated,
    );
    let finished_at_unix_ms = RunRecord::now_unix_ms();
    record.finished_at_unix_ms = Some(finished_at_unix_ms);
    record.duration_ms = Some(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX));

    let child_status = match child_result {
        Ok(status) => status,
        Err(error) => {
            record.status = RunStatus::Failed;
            store.write(&mut handle, &record)?;
            return Err(error.into());
        }
    };

    record.process_exit_code = child_status.code();
    let interruption = launch::interruption_reason(&interrupt_requested, &terminated);
    record.status = if interruption.is_some() || child_status.code().is_none() {
        RunStatus::Interrupted
    } else if child_status.success() {
        RunStatus::Succeeded
    } else {
        RunStatus::Failed
    };
    if let Err(error) = store.write(&mut handle, &record) {
        eprintln!("{error:?}");
        if let Some(interruption) = interruption {
            std::process::exit(interruption.exit_code());
        }
        if let Some(exit_code) = record.process_exit_code.filter(|code| *code != 0) {
            std::process::exit(exit_code);
        }
        return Err(error.into());
    }

    if let Some(interruption) = interruption {
        std::process::exit(interruption.exit_code());
    }
    if let Some(exit_code) = record.process_exit_code {
        if exit_code != 0 {
            std::process::exit(exit_code);
        }
    } else {
        std::process::exit(1);
    }

    Ok(())
}

fn executable_name(program: &str) -> String {
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
        .to_string()
}
