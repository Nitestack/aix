//! Budget lookup and the budget JSON object shared by `aix spend` and
//! `aix status`.

use crate::cache::{Cache, Freshness};
use crate::error::AixError;
use crate::gateway::{BudgetClient, BudgetInfo, BudgetLayer};
use serde::Serialize;
use std::time::Duration;

/// How a command may use the budget cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CachePolicy {
    /// Use a cached record that satisfies the freshness rule.
    Use(Freshness),
    /// Always fetch; the fresh result is still written back.
    Refresh,
}

impl CachePolicy {
    /// `refresh` wins; otherwise `--max-age` overrides the configured TTL.
    pub(crate) fn from_flags(refresh: bool, max_age_secs: Option<u64>) -> Self {
        match (refresh, max_age_secs) {
            (true, _) | (false, Some(0)) => Self::Refresh,
            (false, Some(seconds)) => Self::Use(Freshness::MaxAgeSecs(seconds)),
            (false, None) => Self::Use(Freshness::Configured),
        }
    }
}

pub(crate) struct BudgetLookup {
    pub(crate) budget: BudgetInfo,
    pub(crate) source: SpendSource,
    pub(crate) fetched_at: u64,
}

/// Read the key budget from the cache or the gateway. `Ok(None)` means the
/// gateway has no budget endpoint (HTTP 404/405/501); that is not cached.
pub(crate) async fn lookup_budget(
    cache: &Cache,
    base_url: &str,
    api_key: &str,
    policy: CachePolicy,
    timeout: Duration,
) -> Result<Option<BudgetLookup>, AixError> {
    if let CachePolicy::Use(freshness) = policy {
        let cached = cache.get_slot(
            crate::cache::CacheSlot::Budget,
            base_url,
            api_key,
            freshness,
        );
        if let Some((data, fetched_at)) = cached {
            if let Some(budget) = BudgetInfo::from_cache_value(&data) {
                return Ok(Some(BudgetLookup {
                    budget,
                    source: SpendSource::Cache,
                    fetched_at,
                }));
            }
        }
    }

    match BudgetClient::with_timeout(base_url, api_key, timeout)
        .key_info()
        .await
    {
        Ok(budget) => {
            cache.put(base_url, api_key, &budget.to_cache_value());
            Ok(Some(BudgetLookup {
                budget,
                source: SpendSource::Live,
                fetched_at: crate::cache::now_secs(),
            }))
        }
        Err(AixError::GatewayError {
            status: 404 | 405 | 501,
            ..
        }) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Gateway kind remembered from the last budget response, at any age. This
/// never contacts the gateway.
pub(crate) fn cached_gateway_kind(cache: &Cache, base_url: &str, api_key: &str) -> Option<String> {
    let (data, _) = cache.get_slot(
        crate::cache::CacheSlot::Budget,
        base_url,
        api_key,
        Freshness::AnyAge,
    )?;
    BudgetInfo::from_cache_value(&data)?.gateway
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SpendAvailability {
    Available,
    Unsupported,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SpendSource {
    Cache,
    Live,
    None,
}

#[derive(Serialize)]
pub(crate) struct SpendCacheData {
    pub(crate) source: SpendSource,
    /// Seconds since the budget was fetched; `null` for live reads.
    pub(crate) age_seconds: Option<u64>,
    /// When the budget was fetched from the gateway (RFC 3339, UTC).
    pub(crate) fetched_at: Option<String>,
}

/// The full budget object: `status.data.spend`, and flattened into `spend.data`.
#[derive(Serialize)]
pub(crate) struct SpendData {
    pub(crate) status: SpendAvailability,
    pub(crate) key_spent_usd: Option<f64>,
    pub(crate) remaining_usd: Option<f64>,
    pub(crate) limited_by: Option<String>,
    pub(crate) layers: Option<Vec<BudgetLayer>>,
    pub(crate) expires_at: Option<String>,
    pub(crate) project_id: Option<String>,
    pub(crate) cache: SpendCacheData,
}

impl SpendData {
    pub(crate) fn available(lookup: &BudgetLookup) -> Self {
        let budget = &lookup.budget;
        let age_seconds = (lookup.source == SpendSource::Cache)
            .then(|| crate::cache::now_secs().saturating_sub(lookup.fetched_at));
        Self {
            status: SpendAvailability::Available,
            key_spent_usd: Some(budget.key_spent_usd),
            remaining_usd: budget.remaining_usd,
            limited_by: budget.limited_by.clone(),
            layers: Some(budget.layers.clone()),
            expires_at: budget.expires_at.clone(),
            project_id: budget.project_id.clone(),
            cache: SpendCacheData {
                source: lookup.source,
                age_seconds,
                fetched_at: format_unix(lookup.fetched_at),
            },
        }
    }

    pub(crate) fn unavailable(status: SpendAvailability) -> Self {
        Self {
            status,
            key_spent_usd: None,
            remaining_usd: None,
            limited_by: None,
            layers: None,
            expires_at: None,
            project_id: None,
            cache: SpendCacheData {
                source: SpendSource::None,
                age_seconds: None,
                fetched_at: None,
            },
        }
    }
}

fn format_unix(seconds: u64) -> Option<String> {
    let seconds = i64::try_from(seconds).ok()?;
    chrono::DateTime::from_timestamp(seconds, 0)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_policy_flags() {
        assert_eq!(
            CachePolicy::from_flags(false, None),
            CachePolicy::Use(Freshness::Configured)
        );
        assert_eq!(CachePolicy::from_flags(true, None), CachePolicy::Refresh);
        assert_eq!(
            CachePolicy::from_flags(false, Some(0)),
            CachePolicy::Refresh
        );
        assert_eq!(
            CachePolicy::from_flags(false, Some(60)),
            CachePolicy::Use(Freshness::MaxAgeSecs(60))
        );
    }

    #[test]
    fn fetched_at_is_rfc3339_utc() {
        assert_eq!(
            format_unix(1_793_491_200).as_deref(),
            Some("2026-11-01T00:00:00Z")
        );
    }
}
