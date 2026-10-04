use crate::local_gateway::LaunchContext;
use serde::{Deserialize, Serialize};
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

pub(crate) struct UsageEventRecorder {
    store: Option<crate::usage_store::UsageStore>,
    event: Option<LocalUsageEvent>,
}

impl UsageEventRecorder {
    pub(crate) fn new(
        store: Option<crate::usage_store::UsageStore>,
        context: &LaunchContext,
        protocol: &str,
    ) -> Self {
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
            if store.write_event(&event).is_err() {
                store.report_write_failure();
            }
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

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_totals_use_reported_total_or_complete_canonical_counters() {
        let calculated = TokenUsage::canonical(Some(9), Some(7), Some(2), None, Some(5), None);
        assert_eq!(calculated.total_tokens, Some(14));
        assert_eq!(calculated.completeness(), UsageCompleteness::Complete);

        let provider_total = TokenUsage::canonical(Some(9), None, None, None, Some(5), Some(17));
        assert_eq!(provider_total.total_tokens, Some(17));
    }
}
