use crate::local_gateway::LaunchContext;
use chrono::{DateTime, Utc};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub(crate) const USAGE_EVENT_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UsageOutcome {
    Succeeded,
    Failed,
    Incomplete,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UsageCompleteness {
    Complete,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TokenUsage {
    pub input_tokens_total: Option<u64>,
    pub input_tokens_uncached: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

impl TokenUsage {
    pub(crate) fn canonical(
        input_tokens_total: Option<u64>,
        input_tokens_uncached: Option<u64>,
        cache_read_input_tokens: Option<u64>,
        cache_write_input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        reported_total_tokens: Option<u64>,
    ) -> Self {
        let total_tokens = reported_total_tokens.or_else(|| {
            input_tokens_total
                .zip(output_tokens)
                .and_then(|(input, output)| input.checked_add(output))
        });
        Self {
            input_tokens_total,
            input_tokens_uncached,
            cache_read_input_tokens,
            cache_write_input_tokens,
            output_tokens,
            total_tokens,
        }
    }

    pub(crate) fn completeness(&self) -> UsageCompleteness {
        if self.input_tokens_total.is_some() && self.output_tokens.is_some() {
            UsageCompleteness::Complete
        } else if self.input_tokens_total.is_some()
            || self.input_tokens_uncached.is_some()
            || self.cache_read_input_tokens.is_some()
            || self.cache_write_input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.total_tokens.is_some()
        {
            UsageCompleteness::Partial
        } else {
            UsageCompleteness::Unavailable
        }
    }

    pub(crate) fn merge(&mut self, update: &Self) {
        if update.input_tokens_total.is_some() {
            self.input_tokens_total = update.input_tokens_total;
        }
        if update.input_tokens_uncached.is_some() {
            self.input_tokens_uncached = update.input_tokens_uncached;
        }
        if update.cache_read_input_tokens.is_some() {
            self.cache_read_input_tokens = update.cache_read_input_tokens;
        }
        if update.cache_write_input_tokens.is_some() {
            self.cache_write_input_tokens = update.cache_write_input_tokens;
        }
        if update.output_tokens.is_some() {
            self.output_tokens = update.output_tokens;
        }
        self.total_tokens = update.total_tokens.or_else(|| {
            self.input_tokens_total
                .zip(self.output_tokens)
                .and_then(|(input, output)| input.checked_add(output))
        });
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct LocalUsageEvent {
    pub schema_version: u8,
    pub event_id: Uuid,
    pub started_at_unix_ms: u64,
    pub finished_at_unix_ms: u64,
    pub duration_ms: u64,
    pub profile: String,
    pub logical_tool_name: String,
    pub run_id: Option<String>,
    pub run_policy: Option<String>,
    pub protocol: String,
    pub model: Option<String>,
    pub outcome: UsageOutcome,
    pub http_status: Option<u16>,
    pub error_category: Option<String>,
    pub input_tokens_total: Option<u64>,
    pub input_tokens_uncached: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub request_count: u64,
    pub usage_completeness: UsageCompleteness,
    pub actual_cost_usd: Option<f64>,
    pub cost_source: Option<String>,
}

impl LocalUsageEvent {
    pub(crate) fn new(context: &LaunchContext, protocol: &str, model: Option<String>) -> Self {
        Self {
            schema_version: USAGE_EVENT_SCHEMA_VERSION,
            event_id: Uuid::new_v4(),
            started_at_unix_ms: now_unix_ms(),
            finished_at_unix_ms: 0,
            duration_ms: 0,
            profile: context.profile.clone(),
            logical_tool_name: context.logical_tool_name.clone(),
            run_id: context.run_id.clone(),
            run_policy: context.run_policy.clone(),
            protocol: protocol.to_owned(),
            model,
            outcome: UsageOutcome::Failed,
            http_status: None,
            error_category: None,
            input_tokens_total: None,
            input_tokens_uncached: None,
            cache_read_input_tokens: None,
            cache_write_input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            request_count: 1,
            usage_completeness: UsageCompleteness::Unavailable,
            actual_cost_usd: None,
            cost_source: None,
        }
    }

    pub(crate) fn set_usage(&mut self, usage: &TokenUsage) {
        self.input_tokens_total = usage.input_tokens_total;
        self.input_tokens_uncached = usage.input_tokens_uncached;
        self.cache_read_input_tokens = usage.cache_read_input_tokens;
        self.cache_write_input_tokens = usage.cache_write_input_tokens;
        self.output_tokens = usage.output_tokens;
        self.total_tokens = usage.total_tokens;
        self.usage_completeness = usage.completeness();
    }

    fn finish(&mut self) {
        if self.finished_at_unix_ms == 0 {
            self.finished_at_unix_ms = now_unix_ms();
            self.duration_ms = self
                .finished_at_unix_ms
                .saturating_sub(self.started_at_unix_ms);
        }
    }
}

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
    pub input_tokens_total: u64,
    pub input_tokens_uncached: u64,
    pub cache_read_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Clone)]
pub(crate) struct UsageStore {
    events_dir: PathBuf,
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

    pub(crate) fn new(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            events_dir: state_dir.into().join("usage").join("events"),
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

    /// Returns valid version-1 events whose start time is within the inclusive range.
    /// Individual unreadable, malformed, or unsupported event files are ignored.
    #[allow(dead_code)]
    pub(crate) fn events(
        &self,
        filter: &UsageEventFilter,
    ) -> std::io::Result<Vec<LocalUsageEvent>> {
        if filter.start_unix_ms > filter.end_unix_ms || !self.events_dir.exists() {
            return Ok(Vec::new());
        }
        let entries = fs::read_dir(&self.events_dir)?;
        let mut events = Vec::new();
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
                continue;
            }
            let Ok(day_entries) = fs::read_dir(entry.path()) else {
                continue;
            };
            for day_entry in day_entries.flatten() {
                if day_entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                let Ok(bytes) = fs::read(day_entry.path()) else {
                    continue;
                };
                let Ok(event) = serde_json::from_slice::<LocalUsageEvent>(&bytes) else {
                    continue;
                };
                if event.schema_version != USAGE_EVENT_SCHEMA_VERSION
                    || event.started_at_unix_ms < filter.start_unix_ms
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
                events.push(event);
            }
        }
        events.sort_by(|left, right| {
            left.started_at_unix_ms
                .cmp(&right.started_at_unix_ms)
                .then_with(|| left.event_id.cmp(&right.event_id))
        });
        Ok(events)
    }

    #[allow(dead_code)]
    pub(crate) fn summarize(&self, filter: &UsageEventFilter) -> std::io::Result<UsageSummary> {
        let events = self.events(filter)?;
        let mut summary = UsageSummary::default();
        for event in events {
            summary.request_count = summary.request_count.saturating_add(event.request_count);
            match event.outcome {
                UsageOutcome::Succeeded => summary.succeeded_count += 1,
                UsageOutcome::Failed => summary.failed_count += 1,
                UsageOutcome::Incomplete => summary.incomplete_count += 1,
            }
            summary.input_tokens_total =
                add_known(summary.input_tokens_total, event.input_tokens_total);
            summary.input_tokens_uncached =
                add_known(summary.input_tokens_uncached, event.input_tokens_uncached);
            summary.cache_read_input_tokens = add_known(
                summary.cache_read_input_tokens,
                event.cache_read_input_tokens,
            );
            summary.cache_write_input_tokens = add_known(
                summary.cache_write_input_tokens,
                event.cache_write_input_tokens,
            );
            summary.output_tokens = add_known(summary.output_tokens, event.output_tokens);
            summary.total_tokens = add_known(summary.total_tokens, event.total_tokens);
        }
        Ok(summary)
    }
}

pub(crate) struct UsageEventRecorder {
    store: Option<UsageStore>,
    event: Option<LocalUsageEvent>,
}

impl UsageEventRecorder {
    pub(crate) fn new(store: Option<UsageStore>, context: &LaunchContext, protocol: &str) -> Self {
        Self {
            store,
            event: Some(LocalUsageEvent::new(context, protocol, None)),
        }
    }

    pub(crate) fn set_model(&mut self, model: Option<&str>) {
        if let Some(event) = &mut self.event {
            event.model = model.map(str::to_owned);
        }
    }

    pub(crate) fn set_usage(&mut self, usage: &TokenUsage) {
        if let Some(event) = &mut self.event {
            event.set_usage(usage);
        }
    }

    pub(crate) fn finish(
        &mut self,
        outcome: UsageOutcome,
        http_status: Option<u16>,
        error_category: Option<&str>,
    ) {
        let Some(mut event) = self.event.take() else {
            return;
        };
        event.outcome = outcome;
        event.http_status = http_status;
        event.error_category = error_category.map(str::to_owned);
        event.finish();
        if let Some(store) = &self.store {
            let _ = store.write_event(&event);
        }
    }
}

impl Drop for UsageEventRecorder {
    fn drop(&mut self) {
        if self.event.is_some() {
            self.finish(UsageOutcome::Failed, None, Some("request_terminated"));
        }
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

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn event_date(unix_ms: u64) -> Option<String> {
    let timestamp = i64::try_from(unix_ms).ok()?;
    DateTime::<Utc>::from_timestamp_millis(timestamp)
        .map(|date| date.format("%Y-%m-%d").to_string())
}

#[allow(dead_code)]
fn add_known(total: u64, value: Option<u64>) -> u64 {
    total.saturating_add(value.unwrap_or_default())
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
                    let store = UsageStore::new(state);
                    store
                        .write_event(&event(&format!("model-{index}")))
                        .unwrap();
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }

        let all = UsageStore::new(temp.path())
            .events(&UsageEventFilter {
                start_unix_ms: 0,
                end_unix_ms: u64::MAX,
                ..UsageEventFilter::default()
            })
            .unwrap();
        assert_eq!(all.len(), 24);
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
        let directory = temp.path().join("usage/events").join(date);
        let path = directory.join(format!("{}.json", event.event_id));

        assert_eq!(
            fs::metadata(&usage_directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&events_directory)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn filters_are_inclusive_and_summaries_do_not_expose_file_paths() {
        let temp = assert_fs::TempDir::new().unwrap();
        let store = UsageStore::new(temp.path());
        let mut first = event("model-a");
        first.input_tokens_total = Some(10);
        first.output_tokens = Some(4);
        first.total_tokens = Some(14);
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
        assert_eq!(summary.input_tokens_total, 10);
        assert_eq!(summary.output_tokens, 4);
    }

    #[test]
    fn token_totals_use_reported_total_or_complete_canonical_counters() {
        let calculated = TokenUsage::canonical(Some(9), Some(7), Some(2), None, Some(5), None);
        assert_eq!(calculated.total_tokens, Some(14));
        assert_eq!(calculated.completeness(), UsageCompleteness::Complete);

        let provider_total = TokenUsage::canonical(Some(9), None, None, None, Some(5), Some(17));
        assert_eq!(provider_total.total_tokens, Some(17));
    }
}
