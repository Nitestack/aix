use chrono::{Days, NaiveDate};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Default, Serialize)]
struct UsageMetrics {
    spend: f64,
    #[serde(skip)]
    spend_available: Option<bool>,
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    request_count: u64,
}

impl UsageMetrics {
    fn add_assign(&mut self, other: Self) {
        self.spend += other.spend;
        self.spend_available = match (self.spend_available, other.spend_available) {
            (Some(left), Some(right)) => Some(left && right),
            (None, value) | (value, None) => value,
        };
        self.prompt_tokens = self.prompt_tokens.saturating_add(other.prompt_tokens);
        self.completion_tokens = self
            .completion_tokens
            .saturating_add(other.completion_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
        self.request_count = self.request_count.saturating_add(other.request_count);
    }
}

#[derive(Serialize)]
pub(super) struct UsageReport {
    source: &'static str,
    status: &'static str,
    start_date: String,
    end_date: String,
    profile: String,
    #[serde(flatten)]
    metrics: UsageMetrics,
    billing: LiteLlmBilling,
    daily: Vec<DailyUsage>,
    models: Vec<ModelUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_filter: Option<String>,
}

#[derive(Serialize)]
struct LiteLlmBilling {
    status: &'static str,
    actual_spend: Option<f64>,
    source: Option<&'static str>,
}

#[derive(Serialize)]
struct DailyUsage {
    date: String,
    #[serde(flatten)]
    metrics: UsageMetrics,
}

#[derive(Serialize)]
struct ModelUsage {
    model: String,
    #[serde(flatten)]
    metrics: UsageMetrics,
}

impl UsageReport {
    pub(super) fn from_response(
        response: Value,
        start_date: NaiveDate,
        end_date: NaiveDate,
        model_filter: Option<String>,
        profile: String,
    ) -> Self {
        let mut daily_metrics = BTreeMap::<NaiveDate, UsageMetrics>::new();
        let mut daily_model_totals = BTreeMap::<String, UsageMetrics>::new();

        if let Some(results) = response.get("results").and_then(Value::as_array) {
            for result in results {
                let Some(date) = result
                    .get("date")
                    .and_then(Value::as_str)
                    .and_then(parse_response_date)
                    .filter(|date| *date >= start_date && *date <= end_date)
                else {
                    continue;
                };

                let models = result
                    .get("breakdown")
                    .and_then(|breakdown| breakdown.get("models"))
                    .and_then(model_metrics);
                if let Some(models) = &models {
                    add_model_metrics(&mut daily_model_totals, models);
                }

                let metrics = if let Some(model_filter) = model_filter.as_deref() {
                    let Some(metrics) = models
                        .as_ref()
                        .and_then(|models| models.get(model_filter))
                        .copied()
                    else {
                        continue;
                    };
                    metrics
                } else {
                    metrics_from_value(result.get("metrics").unwrap_or(result))
                };
                daily_metrics
                    .entry(date)
                    .and_modify(|total| total.add_assign(metrics))
                    .or_insert(metrics);
            }
        }

        let top_level_models = response
            .get("models")
            .or_else(|| {
                response
                    .get("breakdown")
                    .and_then(|breakdown| breakdown.get("models"))
            })
            .and_then(model_metrics)
            .filter(|models| !models.is_empty());
        let models = top_level_models.unwrap_or(daily_model_totals);
        let model_rows = model_rows(&models, model_filter.as_deref());

        let daily = daily_metrics
            .iter()
            .map(|(date, metrics)| DailyUsage {
                date: date.format("%Y-%m-%d").to_string(),
                metrics: *metrics,
            })
            .collect::<Vec<_>>();

        let metrics = if let Some(model_filter) = model_filter.as_deref() {
            models.get(model_filter).copied().unwrap_or_default()
        } else {
            let upstream_metrics = response
                .get("metrics")
                .or_else(|| response.get("metadata"))
                .filter(|metrics| metrics.is_object())
                .map(metrics_from_value);
            upstream_metrics.unwrap_or_else(|| {
                daily
                    .iter()
                    .fold(UsageMetrics::default(), |mut total, day| {
                        total.add_assign(day.metrics);
                        total
                    })
            })
        };

        let billing_available = metrics.spend_available == Some(true);
        Self {
            source: "litellm",
            status: "available",
            start_date: start_date.format("%Y-%m-%d").to_string(),
            end_date: end_date.format("%Y-%m-%d").to_string(),
            profile,
            metrics,
            billing: LiteLlmBilling {
                status: if billing_available {
                    "available"
                } else {
                    "unavailable"
                },
                actual_spend: billing_available.then_some(metrics.spend),
                source: billing_available.then_some("litellm"),
            },
            daily,
            models: model_rows,
            model_filter,
        }
    }
}

fn parse_response_date(value: &str) -> Option<NaiveDate> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    (date.format("%Y-%m-%d").to_string() == value).then_some(date)
}

fn metric<'a>(value: &'a Value, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|name| value.get(*name))
}

fn as_spend_optional(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|spend| spend.is_finite())
}

fn as_count(value: Option<&Value>) -> u64 {
    value
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_i64().and_then(|count| u64::try_from(count).ok()))
        })
        .or_else(|| {
            value
                .and_then(Value::as_f64)
                .filter(|count| count.is_finite() && *count >= 0.0)
                .map(|count| count as u64)
        })
        .unwrap_or_default()
}

fn metrics_from_value(value: &Value) -> UsageMetrics {
    let prompt_tokens = as_count(metric(value, &["prompt_tokens", "total_prompt_tokens"]));
    let completion_tokens = as_count(metric(
        value,
        &["completion_tokens", "total_completion_tokens"],
    ));
    let spend = as_spend_optional(metric(value, &["spend", "total_spend"]));
    UsageMetrics {
        spend: spend.unwrap_or_default(),
        spend_available: Some(spend.is_some()),
        prompt_tokens,
        completion_tokens,
        total_tokens: metric(value, &["total_tokens"])
            .map(|value| as_count(Some(value)))
            .unwrap_or_else(|| prompt_tokens.saturating_add(completion_tokens)),
        request_count: as_count(metric(
            value,
            &[
                "request_count",
                "api_requests",
                "total_requests",
                "total_api_requests",
            ],
        )),
    }
}

fn model_metrics(value: &Value) -> Option<BTreeMap<String, UsageMetrics>> {
    let mut models = BTreeMap::new();
    match value {
        Value::Object(values) => {
            for (name, metrics) in values {
                let metrics = metrics
                    .get("metrics")
                    .filter(|metrics| metrics.is_object())
                    .unwrap_or(metrics);
                models.insert(name.clone(), metrics_from_value(metrics));
            }
        }
        Value::Array(values) => {
            for entry in values {
                let Some(name) = entry
                    .get("model")
                    .or_else(|| entry.get("model_name"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                let metrics = entry
                    .get("metrics")
                    .filter(|metrics| metrics.is_object())
                    .unwrap_or(entry);
                models.insert(name.to_string(), metrics_from_value(metrics));
            }
        }
        _ => return None,
    }
    Some(models)
}

fn add_model_metrics(
    totals: &mut BTreeMap<String, UsageMetrics>,
    models: &BTreeMap<String, UsageMetrics>,
) {
    for (name, metrics) in models {
        totals
            .entry(name.clone())
            .and_modify(|total| total.add_assign(*metrics))
            .or_insert(*metrics);
    }
}

fn model_rows(
    models: &BTreeMap<String, UsageMetrics>,
    model_filter: Option<&str>,
) -> Vec<ModelUsage> {
    let mut rows: Vec<_> = models
        .iter()
        .filter(|(model, _)| model_filter.is_none_or(|filter| model.as_str() == filter))
        .map(|(model, metrics)| ModelUsage {
            model: model.clone(),
            metrics: *metrics,
        })
        .collect();
    rows.sort_by(|left, right| {
        right
            .metrics
            .spend
            .total_cmp(&left.metrics.spend)
            .then_with(|| left.model.cmp(&right.model))
    });
    rows
}

fn sum_daily_spend(report: &UsageReport, start_date: &str, end_date: &str) -> Option<f64> {
    let days: Vec<_> = report
        .daily
        .iter()
        .filter(|day| day.date.as_str() >= start_date && day.date.as_str() <= end_date)
        .collect();
    if days.is_empty()
        || days
            .iter()
            .any(|day| day.metrics.spend_available != Some(true))
    {
        return None;
    }
    Some(days.iter().map(|day| day.metrics.spend).sum())
}

fn display_spend(spend: Option<f64>) -> String {
    spend.map_or_else(|| "unavailable".to_string(), |spend| format!("${spend:.6}"))
}

pub(super) fn print_human(report: &UsageReport, is_default_range: bool) {
    if let Some(model) = &report.model_filter {
        println!("Model: {model}");
    }

    if is_default_range {
        let start_7d = NaiveDate::parse_from_str(&report.end_date, "%Y-%m-%d")
            .ok()
            .and_then(|today| today.checked_sub_days(Days::new(6)))
            .map(|date| date.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| report.start_date.clone());
        println!(
            "Spend today: {}",
            display_spend(sum_daily_spend(report, &report.end_date, &report.end_date))
        );
        println!(
            "Spend last 7 days ({start_7d} through {}): {}",
            report.end_date,
            display_spend(sum_daily_spend(report, &start_7d, &report.end_date))
        );
        println!(
            "Spend last 30 days ({} through {}): {}",
            report.start_date,
            report.end_date,
            display_spend(report.billing.actual_spend)
        );
    } else {
        println!(
            "Usage for {} through {} (inclusive)",
            report.start_date, report.end_date
        );
        println!("Spend: {}", display_spend(report.billing.actual_spend));
    }

    println!("Prompt tokens: {}", report.metrics.prompt_tokens);
    println!("Completion tokens: {}", report.metrics.completion_tokens);
    println!("Total tokens: {}", report.metrics.total_tokens);
    println!("Requests: {}", report.metrics.request_count);
    println!("Models:");
    if report.models.is_empty() {
        println!("  (no model usage in range)");
    } else {
        for model in &report.models {
            let spend = if model.metrics.spend_available == Some(true) {
                display_spend(Some(model.metrics.spend))
            } else {
                "unavailable".to_string()
            };
            println!(
                "  {}  {}  {} requests",
                model.model, spend, model.metrics.request_count
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn response_metadata_aliases_and_missing_optional_breakdowns_are_safe() {
        let start_date = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
        let report = UsageReport::from_response(
            json!({
                "results": [{
                    "date": "2026-09-01",
                    "metrics": { "spend": 1.0, "prompt_tokens": 2, "completion_tokens": 3 }
                }],
                "metadata": {
                    "total_spend": 1.0,
                    "total_prompt_tokens": 2,
                    "total_completion_tokens": 3,
                    "total_api_requests": 4
                }
            }),
            start_date,
            start_date,
            None,
            "test-profile".to_string(),
        );

        assert_eq!(report.metrics.spend, 1.0);
        assert_eq!(report.metrics.total_tokens, 5);
        assert_eq!(report.metrics.request_count, 4);
        assert!(report.models.is_empty());
    }
}
