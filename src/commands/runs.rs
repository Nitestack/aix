use crate::cli::RunsAction;
use crate::output;
use crate::run_history::{RunPolicyRecord, RunRecord, RunStore, RunUsageRecord};
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

    let show_usage = records.iter().any(|record| record.usage.is_some());
    if show_usage {
        println!(
            "RUN ID  STARTED  PROFILE  EXECUTABLE  WORKFLOW / TASK  STATUS  DURATION  REQS  TOKENS"
        );
    } else {
        println!("RUN ID  STARTED  PROFILE  EXECUTABLE  WORKFLOW / TASK  STATUS  DURATION");
    }
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
        let row = format!(
            "{}  {}  {:?}  {}  {}  {}  {}",
            record.run_id,
            format_timestamp(record.started_at_unix_ms),
            record.profile,
            executable,
            metadata,
            status_name(record.status),
            duration,
        );
        if show_usage {
            let requests = record
                .usage
                .as_ref()
                .map(|usage| usage.request_count.to_string())
                .unwrap_or_else(|| "-".to_string());
            let tokens = record
                .usage
                .as_ref()
                .and_then(|usage| usage.total_tokens)
                .map(compact_count)
                .unwrap_or_else(|| "-".to_string());
            println!("{row}  {requests}  {tokens}");
        } else {
            println!("{row}");
        }
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
    if let Some(usage) = &record.usage {
        print_usage(usage);
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

fn print_usage(usage: &RunUsageRecord) {
    println!("Requests: {}", usage.request_count);
    println!("Successful requests: {}", usage.successful_requests);
    println!("Failed requests: {}", usage.failed_requests);
    println!(
        "Input tokens total: {}",
        format_token_count(usage.input_tokens_total)
    );
    println!(
        "Input tokens uncached: {}",
        format_token_count(usage.input_tokens_uncached)
    );
    println!(
        "Cache-read input tokens: {}",
        format_token_count(usage.cache_read_input_tokens)
    );
    println!(
        "Cache-write input tokens: {}",
        format_token_count(usage.cache_write_input_tokens)
    );
    println!("Output tokens: {}", format_token_count(usage.output_tokens));
    println!("Total tokens: {}", format_token_count(usage.total_tokens));
    println!("Models: {:?}", usage.models);
    println!("Protocols: {:?}", usage.protocols);
}

fn format_token_count(count: Option<u64>) -> String {
    count.map_or_else(|| "(unknown)".to_string(), |count| count.to_string())
}

fn compact_count(count: u64) -> String {
    let (scale, suffix) = if count >= 1_000_000_000 {
        (1_000_000_000, "b")
    } else if count >= 1_000_000 {
        (1_000_000, "m")
    } else if count >= 1_000 {
        (1_000, "k")
    } else {
        return count.to_string();
    };
    let whole = count / scale;
    let decimal = (count % scale) * 10 / scale;
    if decimal == 0 {
        format!("{whole}{suffix}")
    } else {
        format!("{whole}.{decimal}{suffix}")
    }
}

fn print_policy(policy: &RunPolicyRecord) {
    println!("Policy: {:?}", policy.name);
    match policy.effective_budget {
        Some(budget) => println!("Effective budget: ${budget:.2}"),
        None => println!("Effective budget: (none)"),
    }
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
        crate::run_history::RunStatus::TimedOut => "timed out",
    }
}

fn format_timestamp(unix_ms: u64) -> String {
    let timestamp = i64::try_from(unix_ms / 1000)
        .ok()
        .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok())
        .and_then(|time| time.format(&Rfc3339).ok());
    timestamp.unwrap_or_else(|| format!("{unix_ms}ms since Unix epoch"))
}
