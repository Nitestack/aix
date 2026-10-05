use crate::usage_event::{LocalUsageEvent, UsageOutcome, USAGE_EVENT_SCHEMA_VERSION};
use chrono::{DateTime, Utc};
use directories::BaseDirs;
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use uuid::Uuid;

static STORAGE_SETUP_WARNING_REPORTED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub(crate) struct UsageEventFilter {
    pub start_unix_ms: u64,
    pub end_unix_ms: u64,
    pub profile: Option<String>,
    pub logical_tool_name: Option<String>,
    pub model: Option<String>,
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct UsageSummary {
    pub request_count: u64,
    pub succeeded_count: u64,
    pub failed_count: u64,
    pub incomplete_count: u64,
    pub usage_complete_count: u64,
    pub usage_partial_count: u64,
    pub usage_unavailable_count: u64,
    pub input_tokens_total: Option<u64>,
    pub input_tokens_uncached: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub models: Vec<String>,
    pub protocols: Vec<String>,
}

#[derive(Default)]
struct EventRead {
    events: Vec<LocalUsageEvent>,
    read_incomplete: bool,
}

#[derive(Clone)]
pub(crate) struct UsageStore {
    events_dir: PathBuf,
    write_failure_reported: Arc<AtomicBool>,
}

impl UsageStore {
    pub(crate) fn from_environment() -> std::io::Result<Self> {
        let state_dir = match std::env::var_os("AIX_STATE_DIR") {
            Some(path) if !path.is_empty() => PathBuf::from(path),
            Some(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "AIX_STATE_DIR must not be empty",
                ))
            }
            None => default_state_dir().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no platform state directory is available; set AIX_STATE_DIR",
                )
            })?,
        };
        Ok(Self::new(state_dir))
    }

    pub(crate) fn from_environment_or_warn() -> Option<Self> {
        match Self::from_environment() {
            Ok(store) => Some(store),
            Err(_) => {
                report_storage_warning(&STORAGE_SETUP_WARNING_REPORTED);
                None
            }
        }
    }

    pub(crate) fn new(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            events_dir: state_dir.into().join("usage").join("events"),
            write_failure_reported: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn write_event(&self, event: &LocalUsageEvent) -> std::io::Result<()> {
        if event.schema_version != USAGE_EVENT_SCHEMA_VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unsupported local usage event schema version",
            ));
        }
        let date = event_date(event.started_at_unix_ms).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid event timestamp")
        })?;
        let directory = self.events_dir.join(date);
        create_private_directory(
            self.events_dir
                .parent()
                .expect("usage directory has a parent"),
        )?;
        create_private_directory(&self.events_dir)?;
        create_private_directory(&directory)?;
        let path = directory.join(format!("{}.json", event.event_id));
        let bytes = serde_json::to_vec(event).map_err(std::io::Error::other)?;
        write_new_event(&directory, &path, &bytes)
    }

    pub(crate) fn report_write_failure(&self) {
        report_storage_warning(&self.write_failure_reported);
    }

    /// Returns valid version-1 events whose start time is within the inclusive range.
    /// Individual unreadable, malformed, or unsupported event files are ignored.
    #[allow(dead_code)]
    pub(crate) fn events(
        &self,
        filter: &UsageEventFilter,
    ) -> std::io::Result<Vec<LocalUsageEvent>> {
        Ok(self.read_events(filter)?.events)
    }

    fn read_events(&self, filter: &UsageEventFilter) -> std::io::Result<EventRead> {
        if filter.start_unix_ms > filter.end_unix_ms || !self.events_dir.exists() {
            return Ok(EventRead::default());
        }
        let entries = fs::read_dir(&self.events_dir)?;
        let start_date = event_date(filter.start_unix_ms);
        let end_date = event_date(filter.end_unix_ms);
        let mut result = EventRead::default();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    result.read_incomplete = true;
                    continue;
                }
            };
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => {
                    result.read_incomplete = true;
                    continue;
                }
            };
            if !file_type.is_dir() {
                continue;
            }
            let directory_name = entry.file_name().to_string_lossy().into_owned();
            if let (Some(start), Some(end)) = (&start_date, &end_date) {
                if directory_name.as_str() < start.as_str()
                    || directory_name.as_str() > end.as_str()
                {
                    continue;
                }
            }
            let day_entries = match fs::read_dir(entry.path()) {
                Ok(entries) => entries,
                Err(_) => {
                    result.read_incomplete = true;
                    continue;
                }
            };
            for day_entry in day_entries {
                let day_entry = match day_entry {
                    Ok(entry) => entry,
                    Err(_) => {
                        result.read_incomplete = true;
                        continue;
                    }
                };
                let file_type = match day_entry.file_type() {
                    Ok(file_type) => file_type,
                    Err(_) => {
                        result.read_incomplete = true;
                        continue;
                    }
                };
                if !file_type.is_file()
                    || day_entry.path().extension().and_then(|ext| ext.to_str()) != Some("json")
                {
                    continue;
                }
                let bytes = match fs::read(day_entry.path()) {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        result.read_incomplete = true;
                        continue;
                    }
                };
                let event = match serde_json::from_slice::<LocalUsageEvent>(&bytes) {
                    Ok(event) => event,
                    Err(_) => {
                        result.read_incomplete = true;
                        continue;
                    }
                };
                if event.schema_version != USAGE_EVENT_SCHEMA_VERSION {
                    result.read_incomplete = true;
                    continue;
                }
                if event.started_at_unix_ms < filter.start_unix_ms
                    || event.started_at_unix_ms > filter.end_unix_ms
                    || filter
                        .profile
                        .as_ref()
                        .is_some_and(|value| &event.profile != value)
                    || filter
                        .logical_tool_name
                        .as_ref()
                        .is_some_and(|value| &event.logical_tool_name != value)
                    || filter
                        .model
                        .as_ref()
                        .is_some_and(|value| event.model.as_ref() != Some(value))
                    || filter
                        .run_id
                        .as_ref()
                        .is_some_and(|value| event.run_id.as_ref() != Some(value))
                {
                    continue;
                }
                result.events.push(event);
            }
        }
        result.events.sort_by(|left, right| {
            left.started_at_unix_ms
                .cmp(&right.started_at_unix_ms)
                .then_with(|| left.event_id.cmp(&right.event_id))
        });
        Ok(result)
    }

    #[allow(dead_code)]
    pub(crate) fn summarize(&self, filter: &UsageEventFilter) -> std::io::Result<UsageSummary> {
        let events = self.events(filter)?;
        Ok(summarize_events(events))
    }

    /// Summarize events linked to one run. No matching events are represented as
    /// `None`; token counters remain unknown if any matching event lacks a value.
    pub(crate) fn summarize_run(
        &self,
        run_id: &str,
        start_unix_ms: u64,
        end_unix_ms: u64,
    ) -> std::io::Result<Option<UsageSummary>> {
        let read = self.read_events(&UsageEventFilter {
            start_unix_ms,
            end_unix_ms,
            run_id: Some(run_id.to_string()),
            ..UsageEventFilter::default()
        })?;
        if read.read_incomplete {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "one or more local usage events could not be read",
            ));
        }
        let events = read.events;
        Ok((!events.is_empty()).then(|| summarize_events(events)))
    }
}

fn summarize_events(events: Vec<LocalUsageEvent>) -> UsageSummary {
    if events.is_empty() {
        return UsageSummary::default();
    }
    let mut summary = UsageSummary {
        input_tokens_total: Some(0),
        input_tokens_uncached: Some(0),
        cache_read_input_tokens: Some(0),
        cache_write_input_tokens: Some(0),
        output_tokens: Some(0),
        total_tokens: Some(0),
        ..UsageSummary::default()
    };
    let mut models = BTreeSet::new();
    let mut protocols = BTreeSet::new();
    for event in events {
        summary.request_count = summary.request_count.saturating_add(event.request_count);
        match event.outcome {
            UsageOutcome::Succeeded => {
                summary.succeeded_count =
                    summary.succeeded_count.saturating_add(event.request_count)
            }
            UsageOutcome::Failed => {
                summary.failed_count = summary.failed_count.saturating_add(event.request_count)
            }
            UsageOutcome::Incomplete => {
                summary.incomplete_count =
                    summary.incomplete_count.saturating_add(event.request_count)
            }
        }
        match event.usage_completeness {
            crate::usage_event::UsageCompleteness::Complete => summary.usage_complete_count += 1,
            crate::usage_event::UsageCompleteness::Partial => summary.usage_partial_count += 1,
            crate::usage_event::UsageCompleteness::Unavailable => {
                summary.usage_unavailable_count += 1
            }
        }
        if let Some(model) = event.model.filter(|model| !model.is_empty()) {
            models.insert(model);
        }
        if !event.protocol.is_empty() {
            protocols.insert(event.protocol);
        }
        sum_complete(&mut summary.input_tokens_total, event.input_tokens_total);
        sum_complete(
            &mut summary.input_tokens_uncached,
            event.input_tokens_uncached,
        );
        sum_complete(
            &mut summary.cache_read_input_tokens,
            event.cache_read_input_tokens,
        );
        sum_complete(
            &mut summary.cache_write_input_tokens,
            event.cache_write_input_tokens,
        );
        sum_complete(&mut summary.output_tokens, event.output_tokens);
        sum_complete(&mut summary.total_tokens, event.total_tokens);
    }
    summary.models = models.into_iter().collect();
    summary.protocols = protocols.into_iter().collect();
    summary
}

fn report_storage_warning(reported: &AtomicBool) {
    if !reported.swap(true, Ordering::Relaxed) {
        let stderr = std::io::stderr();
        let _ = writeln!(
            stderr.lock(),
            "aix: usage_storage_unavailable: local usage events may be missing"
        );
    }
}

fn default_state_dir() -> Option<PathBuf> {
    let base_dirs = BaseDirs::new()?;
    Some(
        base_dirs
            .state_dir()
            .unwrap_or_else(|| base_dirs.data_local_dir())
            .join("aix"),
    )
}

fn event_date(unix_ms: u64) -> Option<String> {
    let timestamp = i64::try_from(unix_ms).ok()?;
    DateTime::<Utc>::from_timestamp_millis(timestamp)
        .map(|date| date.format("%Y-%m-%d").to_string())
}

fn sum_complete(total: &mut Option<u64>, value: Option<u64>) {
    *total = match (*total, value) {
        (Some(total), Some(value)) => Some(total.saturating_add(value)),
        _ => None,
    };
}

fn create_private_directory(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_new_event(directory: &Path, destination: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = directory.join(format!(".{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        fs::hard_link(&temporary, destination)?;
        fs::remove_file(&temporary)?;
        #[cfg(unix)]
        File::open(directory)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_gateway::LaunchContext;
    use crate::usage_event::UsageCompleteness;
    use std::thread;

    fn context() -> LaunchContext {
        LaunchContext::new(
            "work".to_string(),
            "opencode".to_string(),
            Some("run-123".to_string()),
            Some("bounded".to_string()),
        )
    }

    fn event(model: &str) -> LocalUsageEvent {
        let mut event = LocalUsageEvent::new(&context(), "openai_responses", Some(model.into()));
        event.finished_at_unix_ms = event.started_at_unix_ms;
        event.outcome = UsageOutcome::Succeeded;
        event
    }

    fn all_events(store: &UsageStore) -> Vec<LocalUsageEvent> {
        store
            .events(&UsageEventFilter {
                start_unix_ms: 0,
                end_unix_ms: u64::MAX,
                ..UsageEventFilter::default()
            })
            .unwrap()
    }

    #[test]
    fn writes_immutable_events_and_skips_malformed_neighbors() {
        let temp = assert_fs::TempDir::new().unwrap();
        let store = UsageStore::new(temp.path());
        let valid = event("model-a");
        store.write_event(&valid).unwrap();
        let date = event_date(valid.started_at_unix_ms).unwrap();
        let directory = temp.path().join("usage/events").join(date);
        fs::write(directory.join("broken.json"), b"not-json").unwrap();

        let found = store
            .events(&UsageEventFilter {
                start_unix_ms: valid.started_at_unix_ms,
                end_unix_ms: valid.started_at_unix_ms,
                ..UsageEventFilter::default()
            })
            .unwrap();
        assert_eq!(found, [valid]);
    }

    #[test]
    fn concurrent_event_writes_do_not_clobber_each_other() {
        let temp = assert_fs::TempDir::new().unwrap();
        let state = temp.path().to_path_buf();
        let workers: Vec<_> = (0..24)
            .map(|index| {
                let state = state.clone();
                thread::spawn(move || {
                    UsageStore::new(state)
                        .write_event(&event(&format!("model-{index}")))
                        .unwrap();
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(all_events(&UsageStore::new(temp.path())).len(), 24);
    }

    #[cfg(unix)]
    #[test]
    fn event_directories_and_files_are_owner_private() {
        use std::os::unix::fs::PermissionsExt;

        let temp = assert_fs::TempDir::new().unwrap();
        let store = UsageStore::new(temp.path());
        let event = event("private-model");
        store.write_event(&event).unwrap();
        let date = event_date(event.started_at_unix_ms).unwrap();
        let usage_directory = temp.path().join("usage");
        let events_directory = usage_directory.join("events");
        let directory = events_directory.join(date);
        let path = directory.join(format!("{}.json", event.event_id));

        for path in [&usage_directory, &events_directory, &directory] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn filters_are_inclusive_and_summaries_do_not_expose_file_paths() {
        let temp = assert_fs::TempDir::new().unwrap();
        let store = UsageStore::new(temp.path());
        let mut first = event("model-a");
        first.input_tokens_total = Some(10);
        first.input_tokens_uncached = Some(10);
        first.output_tokens = Some(4);
        first.total_tokens = Some(14);
        first.usage_completeness = UsageCompleteness::Complete;
        store.write_event(&first).unwrap();
        let mut second = event("model-b");
        second.profile = "other".to_string();
        store.write_event(&second).unwrap();

        let filter = UsageEventFilter {
            start_unix_ms: first.started_at_unix_ms,
            end_unix_ms: first.started_at_unix_ms,
            profile: Some("work".into()),
            logical_tool_name: Some("opencode".into()),
            model: Some("model-a".into()),
            run_id: Some("run-123".into()),
        };
        let summary = store.summarize(&filter).unwrap();
        assert_eq!(summary.request_count, 1);
        assert_eq!(summary.input_tokens_total, Some(10));
        assert_eq!(summary.output_tokens, Some(4));
        assert_eq!(summary.usage_complete_count, 1);
    }

    #[test]
    fn summaries_preserve_unknown_token_counts() {
        let temp = assert_fs::TempDir::new().unwrap();
        let store = UsageStore::new(temp.path());
        let mut known = event("model-a");
        known.input_tokens_total = Some(10);
        known.output_tokens = Some(4);
        known.total_tokens = Some(14);
        known.usage_completeness = UsageCompleteness::Complete;
        store.write_event(&known).unwrap();
        store.write_event(&event("model-a")).unwrap();

        let summary = store
            .summarize(&UsageEventFilter {
                start_unix_ms: 0,
                end_unix_ms: u64::MAX,
                ..UsageEventFilter::default()
            })
            .unwrap();

        assert_eq!(summary.request_count, 2);
        assert_eq!(summary.input_tokens_total, None);
        assert_eq!(summary.output_tokens, None);
        assert_eq!(summary.total_tokens, None);
        assert_eq!(summary.usage_complete_count, 1);
        assert_eq!(summary.usage_unavailable_count, 1);
    }

    #[test]
    fn run_summaries_aggregate_mixed_protocols_without_double_counting_cache_tokens() {
        let temp = assert_fs::TempDir::new().unwrap();
        let store = UsageStore::new(temp.path());

        let mut openai = event("gpt-test");
        openai.protocol = "openai_chat_completions".to_string();
        openai.input_tokens_total = Some(100);
        openai.input_tokens_uncached = Some(70);
        openai.cache_read_input_tokens = Some(30);
        openai.cache_write_input_tokens = Some(0);
        openai.output_tokens = Some(10);
        openai.total_tokens = Some(110);
        openai.usage_completeness = UsageCompleteness::Complete;
        store.write_event(&openai).unwrap();

        let mut duplicate_model = openai.clone();
        duplicate_model.event_id = Uuid::new_v4();
        duplicate_model.input_tokens_total = Some(0);
        duplicate_model.input_tokens_uncached = Some(0);
        duplicate_model.cache_read_input_tokens = Some(0);
        duplicate_model.cache_write_input_tokens = Some(0);
        duplicate_model.output_tokens = Some(0);
        duplicate_model.total_tokens = Some(0);
        store.write_event(&duplicate_model).unwrap();

        let mut anthropic = event("claude-test");
        anthropic.protocol = "anthropic_messages".to_string();
        anthropic.input_tokens_total = Some(50);
        anthropic.input_tokens_uncached = Some(15);
        anthropic.cache_read_input_tokens = Some(30);
        anthropic.cache_write_input_tokens = Some(5);
        anthropic.output_tokens = Some(20);
        anthropic.total_tokens = Some(70);
        anthropic.usage_completeness = UsageCompleteness::Complete;
        store.write_event(&anthropic).unwrap();

        let summary = store
            .summarize_run("run-123", 0, u64::MAX)
            .unwrap()
            .unwrap();
        assert_eq!(summary.request_count, 3);
        assert_eq!(summary.succeeded_count, 3);
        assert_eq!(summary.failed_count, 0);
        assert_eq!(summary.input_tokens_total, Some(150));
        assert_eq!(summary.input_tokens_uncached, Some(85));
        assert_eq!(summary.cache_read_input_tokens, Some(60));
        assert_eq!(summary.cache_write_input_tokens, Some(5));
        assert_eq!(summary.output_tokens, Some(30));
        assert_eq!(summary.total_tokens, Some(180));
        assert_eq!(summary.models, ["claude-test", "gpt-test"]);
        assert_eq!(
            summary.protocols,
            ["anthropic_messages", "openai_chat_completions"]
        );
    }

    #[test]
    fn run_summaries_count_failed_and_incomplete_requests_and_keep_unknown_tokens_unknown() {
        let temp = assert_fs::TempDir::new().unwrap();
        let store = UsageStore::new(temp.path());
        let mut failed = event("gpt-failed");
        failed.run_id = Some("failed-run".to_string());
        failed.outcome = UsageOutcome::Failed;
        store.write_event(&failed).unwrap();
        let mut incomplete = event("claude-interrupted");
        incomplete.run_id = Some("failed-run".to_string());
        incomplete.protocol = "anthropic_messages".to_string();
        incomplete.outcome = UsageOutcome::Incomplete;
        store.write_event(&incomplete).unwrap();

        let summary = store
            .summarize_run("failed-run", 0, u64::MAX)
            .unwrap()
            .unwrap();
        assert_eq!(summary.request_count, 2);
        assert_eq!(summary.succeeded_count, 0);
        assert_eq!(summary.failed_count, 1);
        assert_eq!(summary.incomplete_count, 1);
        assert_eq!(summary.input_tokens_total, None);
        assert_eq!(summary.output_tokens, None);
        assert_eq!(summary.total_tokens, None);
        assert_eq!(summary.models, ["claude-interrupted", "gpt-failed"]);
        assert_eq!(
            summary.protocols,
            ["anthropic_messages", "openai_responses"]
        );
    }

    #[test]
    fn run_summaries_are_limited_to_run_id_and_time_range_and_absent_without_events() {
        let temp = assert_fs::TempDir::new().unwrap();
        let store = UsageStore::new(temp.path());
        let mut event = event("gpt-test");
        event.started_at_unix_ms = 100;
        event.finished_at_unix_ms = 110;
        store.write_event(&event).unwrap();

        assert!(store
            .summarize_run("another-run", 0, 200)
            .unwrap()
            .is_none());
        assert!(store.summarize_run("run-123", 0, 99).unwrap().is_none());
        assert!(store.summarize_run("run-123", 111, 200).unwrap().is_none());
        assert!(store
            .summarize_run("no-events", 0, u64::MAX)
            .unwrap()
            .is_none());
        assert_eq!(
            store
                .summarize_run("run-123", 100, 110)
                .unwrap()
                .unwrap()
                .request_count,
            1
        );
    }

    #[test]
    fn run_summary_fails_closed_on_malformed_events_but_general_reads_keep_valid_events() {
        let temp = assert_fs::TempDir::new().unwrap();
        let store = UsageStore::new(temp.path());
        let valid = event("gpt-test");
        store.write_event(&valid).unwrap();
        let date = event_date(valid.started_at_unix_ms).unwrap();
        let directory = temp.path().join("usage/events").join(date);
        fs::write(directory.join("broken.json"), b"not-json").unwrap();

        assert!(store.summarize_run("run-123", 0, u64::MAX).is_err());
        assert_eq!(all_events(&store), [valid]);
    }
}
