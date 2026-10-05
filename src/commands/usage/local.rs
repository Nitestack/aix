use crate::usage_event::{LocalUsageEvent, UsageCompleteness, UsageOutcome};
use chrono::{DateTime, Local, NaiveDate, Utc};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize)]
pub(super) struct LocalUsageReport {
    source: &'static str,
    status: LocalReportStatus,
    start_date: String,
    end_date: String,
    profile: String,
    metrics: LocalUsageMetrics,
    daily: Vec<DailyUsage>,
    models: Vec<ModelUsage>,
    tools: Vec<ToolUsage>,
    protocols: Vec<ProtocolUsage>,
    runs: Vec<RunUsage>,
    billing: LocalBilling,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_filter: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum LocalReportStatus {
    Available,
    Empty,
    Partial,
    Unavailable,
}

pub(super) struct LocalUsageContext {
    pub start_date: NaiveDate,
    pub end_date: NaiveDate,
    pub profile: String,
    pub is_chatgpt: bool,
    pub storage_unavailable: bool,
    pub scan_incomplete: bool,
    pub model_filter: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Copy, Default, Serialize)]
struct LocalUsageMetrics {
    request_count: u64,
    successful_requests: u64,
    failed_requests: u64,
    incomplete_requests: u64,
    input_tokens_total: Option<u64>,
    input_tokens_uncached: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_write_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
    usage_completeness: UsageCompletenessCounts,
    #[serde(skip)]
    initialized: bool,
}

impl LocalUsageMetrics {
    fn add_event(&mut self, event: &LocalUsageEvent) {
        if !self.initialized {
            self.input_tokens_total = Some(0);
            self.input_tokens_uncached = Some(0);
            self.cache_read_input_tokens = Some(0);
            self.cache_write_input_tokens = Some(0);
            self.output_tokens = Some(0);
            self.total_tokens = Some(0);
            self.initialized = true;
        }

        self.request_count = self.request_count.saturating_add(event.request_count);
        match event.outcome {
            UsageOutcome::Succeeded => {
                self.successful_requests =
                    self.successful_requests.saturating_add(event.request_count)
            }
            UsageOutcome::Failed => {
                self.failed_requests = self.failed_requests.saturating_add(event.request_count)
            }
            UsageOutcome::Incomplete => {
                self.incomplete_requests =
                    self.incomplete_requests.saturating_add(event.request_count)
            }
        }
        match event.usage_completeness {
            UsageCompleteness::Complete => {
                self.usage_completeness.complete_requests = self
                    .usage_completeness
                    .complete_requests
                    .saturating_add(event.request_count)
            }
            UsageCompleteness::Partial => {
                self.usage_completeness.partial_requests = self
                    .usage_completeness
                    .partial_requests
                    .saturating_add(event.request_count)
            }
            UsageCompleteness::Unavailable => {
                self.usage_completeness.unavailable_requests = self
                    .usage_completeness
                    .unavailable_requests
                    .saturating_add(event.request_count)
            }
        }
        add_known(&mut self.input_tokens_total, event.input_tokens_total);
        add_known(&mut self.input_tokens_uncached, event.input_tokens_uncached);
        add_known(
            &mut self.cache_read_input_tokens,
            event.cache_read_input_tokens,
        );
        add_known(
            &mut self.cache_write_input_tokens,
            event.cache_write_input_tokens,
        );
        add_known(&mut self.output_tokens, event.output_tokens);
        add_known(&mut self.total_tokens, event.total_tokens);
    }
}

#[derive(Clone, Copy, Default, Serialize)]
struct UsageCompletenessCounts {
    complete_requests: u64,
    partial_requests: u64,
    unavailable_requests: u64,
}

#[derive(Serialize)]
struct LocalBilling {
    status: LocalBillingStatus,
    actual_spend: Option<f64>,
    source: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum LocalBillingStatus {
    Available,
    Unavailable,
    NotApplicable,
}

#[derive(Serialize)]
struct DailyUsage {
    date: String,
    metrics: LocalUsageMetrics,
}

#[derive(Serialize)]
struct ModelUsage {
    model: Option<String>,
    metrics: LocalUsageMetrics,
}

#[derive(Serialize)]
struct ToolUsage {
    tool: String,
    metrics: LocalUsageMetrics,
}

#[derive(Serialize)]
struct ProtocolUsage {
    protocol: String,
    metrics: LocalUsageMetrics,
}

#[derive(Serialize)]
struct RunUsage {
    run_id: Option<String>,
    metrics: LocalUsageMetrics,
}

impl LocalUsageReport {
    pub(super) fn from_events(events: Vec<LocalUsageEvent>, context: LocalUsageContext) -> Self {
        let LocalUsageContext {
            start_date,
            end_date,
            profile,
            is_chatgpt,
            storage_unavailable,
            scan_incomplete,
            model_filter,
            message,
        } = context;
        let mut metrics = LocalUsageMetrics::default();
        let mut daily = BTreeMap::<NaiveDate, LocalUsageMetrics>::new();
        let mut models = BTreeMap::<Option<String>, LocalUsageMetrics>::new();
        let mut tools = BTreeMap::<String, LocalUsageMetrics>::new();
        let mut protocols = BTreeMap::<String, LocalUsageMetrics>::new();
        let mut runs = BTreeMap::<Option<String>, LocalUsageMetrics>::new();
        let has_run_ids = events.iter().any(|event| event.run_id.is_some());

        for event in &events {
            metrics.add_event(event);
            if let Some(date) = local_date(event.started_at_unix_ms) {
                daily.entry(date).or_default().add_event(event);
            }
            models
                .entry(event.model.clone())
                .or_default()
                .add_event(event);
            tools
                .entry(event.logical_tool_name.clone())
                .or_default()
                .add_event(event);
            protocols
                .entry(event.protocol.clone())
                .or_default()
                .add_event(event);
            if has_run_ids {
                runs.entry(event.run_id.clone())
                    .or_default()
                    .add_event(event);
            }
        }

        let daily = daily
            .into_iter()
            .map(|(date, metrics)| DailyUsage {
                date: date.format("%Y-%m-%d").to_string(),
                metrics,
            })
            .collect();
        let models = models
            .into_iter()
            .map(|(model, metrics)| ModelUsage { model, metrics })
            .collect();
        let tools = tools
            .into_iter()
            .map(|(tool, metrics)| ToolUsage { tool, metrics })
            .collect();
        let protocols = protocols
            .into_iter()
            .map(|(protocol, metrics)| ProtocolUsage { protocol, metrics })
            .collect();
        let runs = runs
            .into_iter()
            .map(|(run_id, metrics)| RunUsage { run_id, metrics })
            .collect();

        let billing = local_billing(&events, is_chatgpt);
        let status = if storage_unavailable {
            LocalReportStatus::Unavailable
        } else if scan_incomplete {
            LocalReportStatus::Partial
        } else if events.is_empty() {
            LocalReportStatus::Empty
        } else {
            LocalReportStatus::Available
        };
        let message = if storage_unavailable {
            Some("Local usage storage is unavailable.".to_string())
        } else if scan_incomplete {
            Some("Some local usage events could not be read; totals may be incomplete.".to_string())
        } else if events.is_empty() {
            let no_events = "No local usage events were observed in the selected date range.";
            Some(message.map_or_else(
                || no_events.to_string(),
                |context| format!("{no_events} {context}"),
            ))
        } else {
            None
        };

        Self {
            source: "local_gateway",
            status,
            start_date: start_date.format("%Y-%m-%d").to_string(),
            end_date: end_date.format("%Y-%m-%d").to_string(),
            profile,
            metrics,
            daily,
            models,
            tools,
            protocols,
            runs,
            billing,
            message,
            model_filter,
        }
    }

    pub(super) fn print_human(&self) {
        println!("Usage {} through {}", self.start_date, self.end_date);
        println!("Profile {}", self.profile);
        println!("Observed locally");
        if let Some(message) = &self.message {
            println!("  {message}");
        }
        println!(
            "  Requests        {} ({} ok, {} failed, {} incomplete)",
            self.metrics.request_count,
            self.metrics.successful_requests,
            self.metrics.failed_requests,
            self.metrics.incomplete_requests
        );
        println!(
            "  Input total     {}",
            display_count(self.metrics.input_tokens_total)
        );
        println!(
            "  Input uncached  {}",
            display_count(self.metrics.input_tokens_uncached)
        );
        println!(
            "  Cache read      {}",
            display_count(self.metrics.cache_read_input_tokens)
        );
        println!(
            "  Cache write     {}",
            display_count(self.metrics.cache_write_input_tokens)
        );
        println!(
            "  Output          {}",
            display_count(self.metrics.output_tokens)
        );
        println!(
            "  Total           {}",
            display_count(self.metrics.total_tokens)
        );
        println!(
            "  Usage completeness: {} complete, {} partial, {} unavailable",
            self.metrics.usage_completeness.complete_requests,
            self.metrics.usage_completeness.partial_requests,
            self.metrics.usage_completeness.unavailable_requests
        );
        match self.billing.status {
            LocalBillingStatus::NotApplicable => {
                println!("  Spend           not applicable (ChatGPT plan)")
            }
            LocalBillingStatus::Available => println!(
                "  Spend           ${:.6} ({})",
                self.billing.actual_spend.unwrap_or_default(),
                self.billing
                    .source
                    .as_deref()
                    .unwrap_or("authoritative source")
            ),
            LocalBillingStatus::Unavailable => println!("  Spend           unavailable"),
        }

        print_daily(&self.daily);
        print_models(&self.models);
        print_tools(&self.tools);
        print_protocols(&self.protocols);
        print_runs(&self.runs);
    }
}

fn add_known(total: &mut Option<u64>, value: Option<u64>) {
    *total = match (*total, value) {
        (Some(total), Some(value)) => Some(total.saturating_add(value)),
        _ => None,
    };
}

fn local_billing(events: &[LocalUsageEvent], is_chatgpt: bool) -> LocalBilling {
    if is_chatgpt {
        return LocalBilling {
            status: LocalBillingStatus::NotApplicable,
            actual_spend: None,
            source: None,
        };
    }
    if events.is_empty() {
        return LocalBilling {
            status: LocalBillingStatus::Unavailable,
            actual_spend: None,
            source: None,
        };
    }

    let mut total = 0.0;
    let mut source: Option<&str> = None;
    for event in events {
        let (Some(cost), Some(cost_source)) = (event.actual_cost_usd, event.cost_source.as_deref())
        else {
            return LocalBilling {
                status: LocalBillingStatus::Unavailable,
                actual_spend: None,
                source: None,
            };
        };
        if !cost.is_finite() || source.is_some_and(|source| source != cost_source) {
            return LocalBilling {
                status: LocalBillingStatus::Unavailable,
                actual_spend: None,
                source: None,
            };
        }
        source = Some(cost_source);
        total += cost;
    }
    LocalBilling {
        status: LocalBillingStatus::Available,
        actual_spend: Some(total),
        source: source.map(str::to_owned),
    }
}

fn local_date(unix_ms: u64) -> Option<NaiveDate> {
    let timestamp = i64::try_from(unix_ms).ok()?;
    DateTime::<Utc>::from_timestamp_millis(timestamp)
        .map(|date| date.with_timezone(&Local).date_naive())
}

fn display_count(value: Option<u64>) -> String {
    value.map_or_else(|| "unavailable".to_string(), |count| count.to_string())
}

fn print_daily(rows: &[DailyUsage]) {
    println!("Daily");
    if rows.is_empty() {
        println!("  (no events)");
        return;
    }
    for row in rows {
        println!(
            "  {}  {} req  {} tok",
            row.date,
            row.metrics.request_count,
            display_count(row.metrics.total_tokens)
        );
    }
}

fn print_models(rows: &[ModelUsage]) {
    println!("Models");
    if rows.is_empty() {
        println!("  (no model usage in range)");
        return;
    }
    for row in rows {
        println!(
            "  {}  {} req  {} tok",
            row.model.as_deref().unwrap_or("unknown"),
            row.metrics.request_count,
            display_count(row.metrics.total_tokens)
        );
    }
}

fn print_tools(rows: &[ToolUsage]) {
    println!("Tools");
    if rows.is_empty() {
        println!("  (no tool usage in range)");
        return;
    }
    for row in rows {
        println!(
            "  {}  {} req  {} tok",
            row.tool,
            row.metrics.request_count,
            display_count(row.metrics.total_tokens)
        );
    }
}

fn print_protocols(rows: &[ProtocolUsage]) {
    println!("Protocols");
    if rows.is_empty() {
        println!("  (no protocol usage in range)");
        return;
    }
    for row in rows {
        println!("  {}  {} req", row.protocol, row.metrics.request_count);
    }
}

fn print_runs(rows: &[RunUsage]) {
    if rows.is_empty() {
        return;
    }
    println!("Runs");
    for row in rows {
        println!(
            "  {}  {} req  {} tok",
            row.run_id.as_deref().unwrap_or("unscoped"),
            row.metrics.request_count,
            display_count(row.metrics.total_tokens)
        );
    }
}
