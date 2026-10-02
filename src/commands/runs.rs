use crate::cli::RunsAction;
use crate::output;
use crate::run_history::{RunPolicyRecord, RunRecord, RunStore};
use color_eyre::Result;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

pub fn run(action: Option<RunsAction>, limit: usize, json: bool) -> Result<()> {
    let store = RunStore::from_environment()?;
    match action {
        Some(RunsAction::Show { run_id }) => {
            let record = store.get(&run_id)?;
            if json {
                output::print_json("runs show", record)?;
            } else {
                print_record(&record);
            }
        }
        None => {
            let records = store.list(limit)?;
            if json {
                output::print_json("runs", records)?;
            } else {
                print_list(&records);
            }
        }
    }
    Ok(())
}

fn print_list(records: &[RunRecord]) {
    if records.is_empty() {
        println!("(no runs)");
        return;
    }

    println!("RUN ID  STARTED  PROFILE  EXECUTABLE  WORKFLOW / TASK  STATUS  DURATION");
    for record in records {
        let executable = record
            .logical_tool_name
            .as_ref()
            .map(|tool| format!("{:?} ({tool:?})", record.executable_name))
            .unwrap_or_else(|| format!("{:?}", record.executable_name));
        let metadata = match (&record.workflow, &record.task_id) {
            (Some(workflow), Some(task_id)) => format!("{workflow:?} / {task_id:?}"),
            (Some(workflow), None) => format!("{workflow:?}"),
            (None, Some(task_id)) => format!("{task_id:?}"),
            (None, None) => "-".to_string(),
        };
        let duration = record
            .duration_ms
            .map(|duration| format!("{duration}ms"))
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{}  {}  {:?}  {}  {}  {}  {}",
            record.run_id,
            format_timestamp(record.started_at_unix_ms),
            record.profile,
            executable,
            metadata,
            status_name(record.status),
            duration,
        );
    }
}

fn print_record(record: &RunRecord) {
    // Use debug formatting for caller-supplied labels so embedded newlines cannot
    // masquerade as extra record fields in terminal output.
    println!("Schema version: {}", record.schema_version);
    println!("Run ID: {}", record.run_id);
    println!("Name: {}", format_option(record.name.as_ref()));
    println!("Workflow: {}", format_option(record.workflow.as_ref()));
    println!("Task ID: {}", format_option(record.task_id.as_ref()));
    println!("Tags: {:?}", record.tags);
    println!("Profile: {:?}", record.profile);
    println!(
        "Logical tool name: {}",
        format_option(record.logical_tool_name.as_ref())
    );
    println!("Executable name: {:?}", record.executable_name);
    println!(
        "Started at: {}",
        format_timestamp(record.started_at_unix_ms)
    );
    println!(
        "Finished at: {}",
        record
            .finished_at_unix_ms
            .map(format_timestamp)
            .unwrap_or_else(|| "(still running)".to_string())
    );
    println!(
        "Duration: {}",
        record
            .duration_ms
            .map(|duration| format!("{duration}ms"))
            .unwrap_or_else(|| "(still running)".to_string())
    );
    println!(
        "Process exit code: {}",
        record
            .process_exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "(unavailable)".to_string())
    );
    println!("Status: {}", status_name(record.status));
    if let Some(policy) = &record.policy {
        print_policy(policy);
    }
    if let Some(lease) = &record.lease {
        println!("Lease key alias: {:?}", lease.key_alias);
        println!("Lease budget: ${:.2}", lease.budget);
        println!("Lease duration: {:?}", lease.duration);
        println!(
            "Lease expiry: {}",
            lease.expires_at.as_deref().map_or_else(
                || "(not returned)".to_string(),
                |expiry| format!("{expiry:?}")
            )
        );
        println!("Lease allowed models: {:?}", lease.allowed_models);
        println!(
            "Lease final spend: {}",
            lease.spend.map_or_else(
                || "(unavailable)".to_string(),
                |spend| format!("${spend:.2}")
            )
        );
        println!("Lease cleanup: {:?}", lease.cleanup_status);
    }
}

fn print_policy(policy: &RunPolicyRecord) {
    println!("Policy: {:?}", policy.name);
    println!("Effective budget: ${:.2}", policy.effective_budget);
    println!("Effective duration: {:?}", policy.effective_duration);
    println!(
        "Effective allowed models: {:?}",
        policy.effective_allowed_models
    );
    println!("Effective tags: {:?}", policy.effective_tags);
}

fn format_option(value: Option<&String>) -> String {
    value.map_or_else(|| "(none)".to_string(), |value| format!("{value:?}"))
}

fn status_name(status: crate::run_history::RunStatus) -> &'static str {
    match status {
        crate::run_history::RunStatus::Running => "running",
        crate::run_history::RunStatus::Succeeded => "succeeded",
        crate::run_history::RunStatus::Failed => "failed",
        crate::run_history::RunStatus::Interrupted => "interrupted",
    }
}

fn format_timestamp(unix_ms: u64) -> String {
    let timestamp = i64::try_from(unix_ms / 1000)
        .ok()
        .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok())
        .and_then(|time| time.format(&Rfc3339).ok());
    timestamp.unwrap_or_else(|| format!("{unix_ms}ms since Unix epoch"))
}
