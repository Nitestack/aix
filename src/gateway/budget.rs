use super::transport::{GatewayTransport, TransportError};
use crate::error::AixError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

const KEY_INFO_PATH: &str = "/v1/key/info";

/// The sanitized, key-scoped budget information exposed by the native endpoint.
/// Monetary fields are parsed to finite numbers; nullable values remain `None`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BudgetInfo {
    pub(crate) key_spent_usd: f64,
    pub(crate) remaining_usd: Option<f64>,
    pub(crate) limited_by: Option<String>,
    pub(crate) layers: Vec<BudgetLayer>,
    pub(crate) expires_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BudgetLayer {
    pub(crate) scope: String,
    pub(crate) budget_usd: Option<f64>,
    pub(crate) spent_usd: f64,
    pub(crate) remaining_usd: Option<f64>,
    pub(crate) period_start: String,
}

pub(crate) struct BudgetClient {
    transport: GatewayTransport,
}

impl BudgetClient {
    pub(crate) fn with_timeout(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            transport: GatewayTransport::with_timeout(base_url, api_key, timeout),
        }
    }

    /// Fetch the budget attached to the authenticated key. This endpoint needs
    /// no key selector: adding one would turn self-key lookup into key lookup.
    pub(crate) async fn key_info(&self) -> Result<BudgetInfo, AixError> {
        let response = self
            .transport
            .get_json(KEY_INFO_PATH, &[])
            .await
            .map_err(map_budget_error)?;
        BudgetInfo::from_response(&response)
    }
}

impl BudgetInfo {
    fn from_response(response: &Value) -> Result<Self, AixError> {
        let malformed = || {
            AixError::GatewayProtocolError("gateway returned malformed native budget information")
        };
        let object = response.as_object().ok_or_else(malformed)?;

        let key_spent_usd =
            required_money(object.get("key_spent_usd"), false).ok_or_else(malformed)?;
        let remaining_usd =
            required_nullable_money(object.get("remaining_usd"), true).ok_or_else(malformed)?;
        let limited_by = required_nullable_scope(object.get("limited_by")).ok_or_else(malformed)?;
        let layers = object
            .get("layers")
            .and_then(Value::as_array)
            .ok_or_else(malformed)?
            .iter()
            .map(parse_layer)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(malformed)?;
        let expires_at = match object.get("expires_at") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) => Some(parse_timestamp(value).ok_or_else(malformed)?),
            Some(_) => return Err(malformed()),
        };

        Ok(Self {
            key_spent_usd,
            remaining_usd,
            limited_by,
            layers,
            expires_at,
        })
    }

    /// Read only the current normalized cache schema. The strict struct
    /// deserializer rejects legacy `spend`/`max_budget` cache records.
    pub(crate) fn from_cache_value(value: &Value) -> Option<Self> {
        if !has_cache_shape(value) {
            return None;
        }
        let info: Self = serde_json::from_value(value.clone()).ok()?;
        info.is_valid().then_some(info)
    }

    pub(crate) fn to_cache_value(&self) -> Value {
        serde_json::to_value(self).expect("budget info is serializable")
    }

    fn is_valid(&self) -> bool {
        self.key_spent_usd.is_finite()
            && self.key_spent_usd >= 0.0
            && self.remaining_usd.is_none_or(f64::is_finite)
            && self.limited_by.as_deref().is_none_or(is_safe_scope)
            && self.expires_at.as_deref().is_none_or(is_safe_timestamp)
            && self.layers.iter().all(BudgetLayer::is_valid)
    }
}

fn has_cache_shape(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if ![
        "key_spent_usd",
        "remaining_usd",
        "limited_by",
        "layers",
        "expires_at",
    ]
    .iter()
    .all(|field| object.contains_key(*field))
    {
        return false;
    }
    object
        .get("layers")
        .and_then(Value::as_array)
        .is_some_and(|layers| {
            layers.iter().all(|layer| {
                let Some(layer) = layer.as_object() else {
                    return false;
                };
                [
                    "scope",
                    "budget_usd",
                    "spent_usd",
                    "remaining_usd",
                    "period_start",
                ]
                .iter()
                .all(|field| layer.contains_key(*field))
            })
        })
}

impl BudgetLayer {
    fn is_valid(&self) -> bool {
        is_safe_scope(&self.scope)
            && self
                .budget_usd
                .is_none_or(|value| value.is_finite() && value >= 0.0)
            && self.spent_usd.is_finite()
            && self.spent_usd >= 0.0
            && self.remaining_usd.is_none_or(f64::is_finite)
            && is_safe_timestamp(&self.period_start)
    }
}

fn parse_layer(value: &Value) -> Option<BudgetLayer> {
    let object = value.as_object()?;
    let scope = object.get("scope").and_then(Value::as_str)?;
    let budget_usd = required_nullable_money(object.get("budget_usd"), false)?;
    let spent_usd = required_money(object.get("spent_usd"), false)?;
    let remaining_usd = required_nullable_money(object.get("remaining_usd"), true)?;
    let period_start = object.get("period_start").and_then(Value::as_str)?;

    let layer = BudgetLayer {
        scope: parse_scope(scope)?,
        budget_usd,
        spent_usd,
        remaining_usd,
        period_start: parse_timestamp(period_start)?,
    };
    layer.is_valid().then_some(layer)
}

fn required_money(value: Option<&Value>, allow_negative: bool) -> Option<f64> {
    match value? {
        Value::String(value) => parse_decimal(value, allow_negative),
        _ => None,
    }
}

fn required_nullable_money(value: Option<&Value>, allow_negative: bool) -> Option<Option<f64>> {
    match value? {
        Value::Null => Some(None),
        Value::String(value) => parse_decimal(value, allow_negative).map(Some),
        _ => None,
    }
}

fn parse_decimal(value: &str, allow_negative: bool) -> Option<f64> {
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let digits = if allow_negative && bytes.first() == Some(&b'-') {
        &bytes[1..]
    } else {
        bytes
    };
    if digits.is_empty() {
        return None;
    }

    let mut decimal_points = 0;
    let mut digits_before_point = 0;
    let mut digits_after_point = 0;
    let mut after_point = false;
    for byte in digits {
        match byte {
            b'0'..=b'9' if after_point => digits_after_point += 1,
            b'0'..=b'9' => digits_before_point += 1,
            b'.' if decimal_points == 0 => {
                decimal_points += 1;
                after_point = true;
            }
            _ => return None,
        }
    }
    if digits_before_point == 0 || (decimal_points == 1 && digits_after_point == 0) {
        return None;
    }

    let parsed = value.parse::<f64>().ok()?;
    (parsed.is_finite() && (allow_negative || parsed >= 0.0)).then_some(parsed)
}

fn required_nullable_scope(value: Option<&Value>) -> Option<Option<String>> {
    match value? {
        Value::Null => Some(None),
        Value::String(value) => parse_scope(value).map(Some),
        _ => None,
    }
}

fn parse_scope(value: &str) -> Option<String> {
    is_safe_scope(value).then(|| value.to_owned())
}

fn is_safe_scope(value: &str) -> bool {
    matches!(value, "key" | "personal" | "project" | "tenant")
}

fn parse_timestamp(value: &str) -> Option<String> {
    is_safe_timestamp(value).then(|| value.to_owned())
}

fn is_safe_timestamp(value: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(value).is_ok()
}

fn map_budget_error(error: TransportError) -> AixError {
    error.into_aix_error()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn response() -> Value {
        json!({
            "key_spent_usd": "41.53",
            "remaining_usd": "12.00",
            "limited_by": "project",
            "layers": [
                {
                    "scope": "key",
                    "budget_usd": "100.00",
                    "spent_usd": "41.53",
                    "remaining_usd": "58.47",
                    "period_start": "2026-11-01T00:00:00Z"
                },
                {
                    "scope": "project",
                    "budget_usd": "20.00",
                    "spent_usd": "8.00",
                    "remaining_usd": "12.00",
                    "period_start": "2026-11-01T00:00:00Z"
                }
            ],
            "expires_at": "2026-12-01T00:00:00Z",
            "key_id": "must-not-persist",
            "name": "must-not-persist",
            "metadata": { "api_key": "must-not-persist" }
        })
    }

    #[tokio::test]
    async fn requests_only_the_authenticated_self_key_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(KEY_INFO_PATH))
            .and(header("Authorization", "Bearer self-secret-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response()))
            .expect(1)
            .mount(&server)
            .await;

        let client =
            BudgetClient::with_timeout(server.uri(), "self-secret-key", Duration::from_secs(30));
        let info = client.key_info().await.unwrap();

        assert_eq!(info.key_spent_usd, 41.53);
        assert_eq!(info.remaining_usd, Some(12.0));
        assert_eq!(info.limited_by.as_deref(), Some("project"));
        assert_eq!(info.layers.len(), 2);
        assert_eq!(info.layers[0].budget_usd, Some(100.0));
        assert_eq!(info.layers[1].remaining_usd, Some(12.0));
        assert_eq!(info.expires_at.as_deref(), Some("2026-12-01T00:00:00Z"));

        let normalized = info.to_cache_value().to_string();
        for private in ["key_id", "must-not-persist", "metadata", "api_key"] {
            assert!(
                !normalized.contains(private),
                "leaked {private}: {normalized}"
            );
        }
    }

    #[test]
    fn null_remaining_and_layer_budgets_preserve_unlimited_state() {
        let info = BudgetInfo::from_response(&json!({
            "key_spent_usd": "1.25",
            "remaining_usd": null,
            "limited_by": null,
            "layers": [{
                "scope": "tenant",
                "budget_usd": null,
                "spent_usd": "1.25",
                "remaining_usd": null,
                "period_start": "2026-11-01T00:00:00Z"
            }]
        }))
        .unwrap();

        assert_eq!(info.key_spent_usd, 1.25);
        assert_eq!(info.remaining_usd, None);
        assert_eq!(info.limited_by, None);
        assert_eq!(info.layers[0].budget_usd, None);
        assert_eq!(info.layers[0].remaining_usd, None);
    }

    #[test]
    fn rejects_missing_or_invalid_required_fields_and_unsafe_decimals() {
        let valid = response();
        for invalid in [
            json!({}),
            json!({ "key_spent_usd": "NaN", "remaining_usd": null, "limited_by": null, "layers": [] }),
            json!({ "key_spent_usd": "1e999", "remaining_usd": null, "limited_by": null, "layers": [] }),
            json!({ "key_spent_usd": "1.0", "limited_by": null, "layers": [] }),
            json!({ "key_spent_usd": "1.0", "remaining_usd": null, "limited_by": null }),
            json!({ "key_spent_usd": " 1.0", "remaining_usd": null, "limited_by": null, "layers": [] }),
            json!({ "key_spent_usd": "-1.0", "remaining_usd": null, "limited_by": null, "layers": [] }),
            json!({ "key_spent_usd": 1.0, "remaining_usd": null, "limited_by": null, "layers": [] }),
        ] {
            assert!(
                BudgetInfo::from_response(&invalid).is_err(),
                "accepted invalid payload: {invalid}"
            );
        }

        assert!(BudgetInfo::from_response(&valid).is_ok());
        assert_eq!(parse_decimal("-0.50", true), Some(-0.5));
        assert!(parse_decimal("-0.50", false).is_none());
    }

    #[test]
    fn rejects_malformed_layer_fields_and_strips_unrecognized_metadata() {
        let invalid_layer = json!({
            "key_spent_usd": "1.0",
            "remaining_usd": null,
            "limited_by": null,
            "layers": [{
                "scope": "key",
                "budget_usd": "Infinity",
                "spent_usd": "1.0",
                "remaining_usd": null,
                "period_start": "2026-11-01T00:00:00Z"
            }]
        });
        assert!(BudgetInfo::from_response(&invalid_layer).is_err());

        let invalid_timestamp = json!({
            "key_spent_usd": "1.0",
            "remaining_usd": null,
            "limited_by": null,
            "layers": [{
                "scope": "key",
                "budget_usd": null,
                "spent_usd": "1.0",
                "remaining_usd": null,
                "period_start": "not-a-timestamp"
            }]
        });
        assert!(BudgetInfo::from_response(&invalid_timestamp).is_err());

        let invalid_scope = json!({
            "key_spent_usd": "1.0",
            "remaining_usd": null,
            "limited_by": "private-team-name",
            "layers": []
        });
        assert!(BudgetInfo::from_response(&invalid_scope).is_err());

        let info = BudgetInfo::from_response(&response()).unwrap();
        let serialized = serde_json::to_string(&info).unwrap();
        for private in ["key_id", "must-not-persist", "metadata", "api_key", "name"] {
            assert!(
                !serialized.contains(private),
                "leaked {private}: {serialized}"
            );
        }
    }

    #[test]
    fn cache_schema_rejects_legacy_budget_fields() {
        let legacy = json!({
            "spend": 41.53,
            "max_budget": 500.0,
            "remaining_budget": 458.47
        });
        assert!(BudgetInfo::from_cache_value(&legacy).is_none());

        let info = BudgetInfo::from_response(&response()).unwrap();
        let cached = info.to_cache_value();
        assert_eq!(BudgetInfo::from_cache_value(&cached), Some(info));
    }

    #[test]
    fn invalid_cache_numbers_and_legacy_extras_are_rejected() {
        let invalid = json!({
            "key_spent_usd": -1.0,
            "remaining_usd": null,
            "limited_by": null,
            "layers": [],
            "expires_at": null
        });
        assert!(BudgetInfo::from_cache_value(&invalid).is_none());

        let with_old_fields = json!({
            "key_spent_usd": 1.0,
            "remaining_usd": null,
            "limited_by": null,
            "layers": [],
            "expires_at": null,
            "max_budget": 500.0
        });
        assert!(BudgetInfo::from_cache_value(&with_old_fields).is_none());

        let missing_nullable_field = json!({
            "key_spent_usd": 1.0,
            "limited_by": null,
            "layers": [],
            "expires_at": null
        });
        assert!(BudgetInfo::from_cache_value(&missing_nullable_field).is_none());
    }
}
