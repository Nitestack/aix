use super::transport::{GatewayTransport, TransportError};
use crate::error::AixError;
use crate::secrets::SecretString;
use serde_json::Value;

const KEY_INFO_PATH: &str = "/key/info";
const KEY_GENERATE_PATH: &str = "/key/generate";
const KEY_DELETE_PATH: &str = "/key/delete";
const KEY_LIST_PATH: &str = "/key/list";
const USER_DAILY_ACTIVITY_PATH: &str = "/user/daily/activity";

pub(crate) struct GeneratedVirtualKey {
    pub(crate) key: SecretString,
    pub(crate) expires_at: Option<String>,
}

pub(crate) struct LiteLlmAdminClient {
    transport: GatewayTransport,
}

impl LiteLlmAdminClient {
    pub(crate) fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            transport: GatewayTransport::new(base_url, api_key),
        }
    }

    pub(crate) async fn user_info(&self) -> Result<Value, AixError> {
        let mut info = sanitize_key_info(
            self.transport
                .get_json(KEY_INFO_PATH, &[])
                .await
                .map_err(map_admin_error)?,
        );

        let Some(user_id) = info
            .get("user_id")
            .and_then(|value| value.as_str())
            .filter(|user_id| !user_id.is_empty())
            .map(str::to_owned)
        else {
            return Ok(info);
        };

        // An embedded keys field from /key/info is not an aggregate response.
        // Only /key/list is allowed to make this cache entry user-scoped.
        if let Some(object) = info.as_object_mut() {
            object.remove("keys");
        }

        // /user/info returns every key but can be very large. Fetch the same
        // per-key data in bounded pages so the cache can still warm sibling keys.
        if let Ok(keys) = self.list_keys(&user_id).await {
            if let Some(object) = info.as_object_mut() {
                object.insert("keys".to_string(), Value::Array(keys));
            }
        }

        Ok(info)
    }

    pub(crate) async fn daily_activity(
        &self,
        start_date: &str,
        end_date: &str,
    ) -> Result<Value, AixError> {
        match self
            .transport
            .get_json(
                USER_DAILY_ACTIVITY_PATH,
                &[
                    ("start_date", start_date.to_string()),
                    ("end_date", end_date.to_string()),
                ],
            )
            .await
        {
            Ok(data) => Ok(data),
            Err(TransportError::Gateway {
                status: 404 | 405 | 501,
                ..
            }) => Err(AixError::UsageUnavailable),
            Err(error) => Err(map_admin_error(error)),
        }
    }

    pub(crate) async fn generate_virtual_key(
        &self,
        budget: f64,
        duration: &str,
        key_alias: &str,
        models: &[String],
        tags: &[String],
    ) -> Result<GeneratedVirtualKey, AixError> {
        let mut body = serde_json::json!({
            "duration": duration,
            "max_budget": budget,
            "key_alias": key_alias,
            "metadata": { "tags": tags }
        });
        if !models.is_empty() {
            body["models"] = serde_json::json!(models);
        }

        let response = self
            .transport
            .post_json(KEY_GENERATE_PATH, &body)
            .await
            .map_err(map_generation_error)?;
        let key = response
            .get("key")
            .and_then(Value::as_str)
            .filter(|key| !key.is_empty())
            .ok_or(AixError::LeaseKeyResponseMalformed)?;

        Ok(GeneratedVirtualKey {
            key: SecretString::new(key.to_string()),
            expires_at: response
                .get("expires")
                .and_then(safe_expiry_value)
                .filter(|expiry| !expiry.contains(key)),
        })
    }

    pub(crate) async fn virtual_key_spend(&self, key: &str) -> Result<Option<f64>, AixError> {
        let response = self
            .transport
            .get_json(KEY_INFO_PATH, &[("key", key.to_string())])
            .await
            .map_err(map_admin_error)?;
        let info = response.get("info").unwrap_or(&response);
        Ok(info.get("spend").and_then(Value::as_f64))
    }

    pub(crate) async fn delete_virtual_key(&self, key: &str) -> Result<(), AixError> {
        self.transport
            .post_json(KEY_DELETE_PATH, &serde_json::json!({ "keys": [key] }))
            .await
            .map(|_| ())
            .map_err(map_admin_error)
    }

    async fn list_keys(&self, user_id: &str) -> Result<Vec<Value>, AixError> {
        let mut page = 1_u64;
        let mut keys = Vec::new();

        loop {
            let data = self
                .transport
                .get_json(
                    KEY_LIST_PATH,
                    &[
                        ("page", page.to_string()),
                        ("size", "100".to_string()),
                        ("user_id", user_id.to_string()),
                        ("return_full_object", "true".to_string()),
                    ],
                )
                .await
                .map_err(map_admin_error)?;

            let page_keys = data
                .get("keys")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let page_was_empty = page_keys.is_empty();
            keys.extend(page_keys.into_iter().filter_map(sanitize_key_entry));

            let total_pages = data
                .get("total_pages")
                .and_then(Value::as_u64)
                .unwrap_or(page);
            if page_was_empty || page >= total_pages {
                break;
            }
            page += 1;
        }

        Ok(keys)
    }
}

fn map_generation_error(error: TransportError) -> AixError {
    match error {
        TransportError::Gateway {
            status: status @ (401 | 403),
            ..
        } => AixError::LeaseGenerationDenied { status },
        TransportError::Gateway { status, .. } => AixError::LeaseGenerationFailed { status },
        TransportError::Http(_) | TransportError::Protocol => AixError::LeaseGenerationUnavailable,
    }
}

fn safe_expiry_value(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => Some(number.to_string()),
        Value::String(expiry)
            if !expiry.is_empty()
                && expiry.len() <= 64
                && expiry.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'+' | b':' | b'.' | b'_')
                }) =>
        {
            Some(expiry.clone())
        }
        _ => None,
    }
}

fn map_admin_error(error: TransportError) -> AixError {
    match error {
        TransportError::Gateway {
            status: 429,
            body,
            safe_body,
        } => {
            if let Some((spend, max_budget)) = parse_budget_exceeded(&body) {
                AixError::BudgetExceeded { spend, max_budget }
            } else {
                AixError::GatewayError {
                    status: 429,
                    body: safe_body,
                }
            }
        }
        TransportError::Gateway {
            status, safe_body, ..
        } => AixError::GatewayError {
            status,
            body: safe_body,
        },
        error => error.into_aix_error(),
    }
}

fn sanitize_key_info(data: Value) -> Value {
    let source = match data {
        Value::Object(mut envelope) => envelope.remove("info").unwrap_or(Value::Object(envelope)),
        other => other,
    };

    let Some(source) = source.as_object() else {
        return Value::Object(serde_json::Map::new());
    };
    let mut info = serde_json::Map::new();
    for field in ["user_id", "spend", "max_budget"] {
        if let Some(value) = source.get(field).filter(|value| {
            if field == "user_id" {
                value.is_string()
            } else {
                value.is_number()
            }
        }) {
            info.insert(field.to_string(), value.clone());
        }
    }
    if let Some(keys) = source.get("keys").and_then(Value::as_array) {
        let keys = keys
            .iter()
            .cloned()
            .filter_map(sanitize_key_entry)
            .collect();
        info.insert("keys".to_string(), Value::Array(keys));
    }

    Value::Object(info)
}

fn sanitize_key_entry(data: Value) -> Option<Value> {
    let source = data.as_object()?;
    let key_name = ["api_key", "key_name", "token"]
        .into_iter()
        .find_map(|field| {
            source
                .get(field)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(crate::cache::sanitized_key_name)
        });

    let mut object = serde_json::Map::new();
    if let Some(key_name) = key_name {
        object.insert("key_name".to_string(), Value::String(key_name));
    }
    for field in ["spend", "max_budget"] {
        if let Some(value) = source.get(field).filter(|value| value.is_number()) {
            object.insert(field.to_string(), value.clone());
        }
    }

    Some(Value::Object(object))
}

/// LiteLLM reports budget_exceeded as a 429 whose `error.message` embeds the
/// figures as free text (e.g. "Current cost: 50.17, Max budget: 50.0")
/// rather than as structured fields, so this pulls them back out.
fn parse_budget_exceeded(body: &str) -> Option<(f64, f64)> {
    let value: Value = serde_json::from_str(body).ok()?;
    let error = value.get("error")?;
    if error.get("type").and_then(Value::as_str) != Some("budget_exceeded") {
        return None;
    }
    let message = error.get("message").and_then(Value::as_str)?;
    let spend = extract_number_after(message, "Current cost: ")?;
    let max_budget = extract_number_after(message, "Max budget: ")?;
    Some((spend, max_budget))
}

fn extract_number_after(s: &str, marker: &str) -> Option<f64> {
    let start = s.find(marker)? + marker.len();
    let rest = &s[start..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn user_info_calls_key_endpoints_and_sanitizes_nested_secrets() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(KEY_INFO_PATH))
            .and(header("Authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "key": "server-key",
                "info": {
                    "user_id": "u123",
                    "spend": 1.23,
                    "key_name": "sk-...1234",
                    "token": "info-token-secret"
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(KEY_LIST_PATH))
            .and(header("Authorization", "Bearer test-key"))
            .and(query_param("user_id", "u123"))
            .and(query_param("page", "1"))
            .and(query_param("size", "100"))
            .and(query_param("return_full_object", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "keys": [
                    {
                        "token": "token-secret",
                        "key_name": "sk-...1234",
                        "metadata": { "api_key": "nested-secret" },
                        "spend": 1.23
                    },
                    { "api_key": "server-key-5678", "spend": 2.34 }
                ],
                "total_pages": 2
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(KEY_LIST_PATH))
            .and(header("Authorization", "Bearer test-key"))
            .and(query_param("user_id", "u123"))
            .and(query_param("page", "2"))
            .and(query_param("size", "100"))
            .and(query_param("return_full_object", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "keys": [{ "api_key": "server-key-9abc", "spend": 3.45 }],
                "total_pages": 2
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = LiteLlmAdminClient::new(server.uri(), "test-key");
        let result = client.user_info().await.unwrap();
        assert_eq!(result["user_id"], "u123");
        assert_eq!(result["spend"], 1.23);
        assert!(result.get("key_name").is_none());
        assert!(result.get("token").is_none());
        assert_eq!(result["keys"].as_array().unwrap().len(), 3);
        assert_eq!(
            result["keys"][0]["key_name"],
            crate::cache::sanitized_key_name("sk-...1234")
        );
        assert!(result["keys"][0].get("api_key").is_none());
        assert!(result["keys"][0].get("token").is_none());
        assert!(result["keys"][0].get("metadata").is_none());
        let serialized = result.to_string();
        for secret in [
            "server-key",
            "token-secret",
            "info-token-secret",
            "nested-secret",
        ] {
            assert!(
                !serialized.contains(secret),
                "leaked {secret} in {serialized}"
            );
        }
    }

    #[test]
    fn sanitization_never_returns_short_raw_credentials() {
        let entry = sanitize_key_entry(json!({
            "api_key": "abcd",
            "key_name": "wxyz",
            "token": "1234",
            "spend": 1.23
        }))
        .unwrap();

        assert_eq!(entry["key_name"], crate::cache::sanitized_key_name("abcd"));
        assert_eq!(entry["spend"], 1.23);
        let serialized = entry.to_string();
        for secret in ["abcd", "wxyz", "1234"] {
            assert!(
                !serialized.contains(secret),
                "leaked {secret} in {serialized}"
            );
        }
    }

    #[test]
    fn sanitizer_does_not_trust_internal_key_name_prefixes() {
        let raw_key = "sk-short-sensitive-token";
        let entry = sanitize_key_entry(json!({ "api_key": raw_key })).unwrap();

        assert_ne!(entry["key_name"], raw_key);
        assert!(!entry.to_string().contains(raw_key));
    }

    #[test]
    fn sanitization_drops_nested_values_in_scalar_fields() {
        let data = sanitize_key_info(json!({
            "info": {
                "user_id": { "token": "raw-user-token" },
                "spend": { "token": "raw-spend-token" },
                "max_budget": { "metadata": { "api_key": "raw-budget-key" } },
                "keys": [{
                    "api_key": "server-token",
                    "spend": { "metadata": { "token": "nested-spend-token" } },
                    "max_budget": { "api_key": "nested-budget-key" }
                }]
            }
        }));

        assert!(data.get("user_id").is_none());
        assert!(data.get("spend").is_none());
        assert!(data.get("max_budget").is_none());
        let serialized = data.to_string();
        for secret in [
            "raw-user-token",
            "raw-spend-token",
            "raw-budget-key",
            "nested-spend-token",
            "nested-budget-key",
        ] {
            assert!(
                !serialized.contains(secret),
                "leaked {secret} in {serialized}"
            );
        }
    }

    #[tokio::test]
    async fn non_success_response_preserves_safe_gateway_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(KEY_INFO_PATH))
            .respond_with(ResponseTemplate::new(403).set_body_string("Invalid API key: test-key"))
            .mount(&server)
            .await;

        let client = LiteLlmAdminClient::new(server.uri(), "test-key");
        let err = client.user_info().await.unwrap_err();
        assert!(matches!(
            err,
            AixError::GatewayError {
                status: 403,
                ref body
            } if body == "Invalid API key: [redacted]"
        ));
        assert!(!err.to_string().contains("test-key"));
    }

    #[tokio::test]
    async fn json_error_body_redacts_credentials_and_secret_field_names() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(KEY_INFO_PATH))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({
                "message": "credential test-key rejected",
                "api_key": "other-key",
                "access_token": "other-token",
                "metadata": { "token": "nested-token" },
                "test-key-field": "other-secret"
            })))
            .mount(&server)
            .await;

        let client = LiteLlmAdminClient::new(server.uri(), "test-key");
        let err = client.user_info().await.unwrap_err();
        let AixError::GatewayError { status: 403, body } = err else {
            panic!("expected GatewayError, got {err:?}");
        };
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["message"], "credential [redacted] rejected");
        assert!(body.get("api_key").is_none());
        assert!(body.get("access_token").is_none());
        assert!(body.get("metadata").is_none());
        assert!(body.get("test-key-field").is_none());
    }

    #[tokio::test]
    async fn budget_exceeded_429_returns_parsed_figures() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(KEY_INFO_PATH))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({
                "error": {
                    "message": "Budget has been exceeded! Current cost: 50.16580493999999, Max budget: 50.0",
                    "type": "budget_exceeded",
                    "param": null,
                    "code": "429"
                }
            })))
            .mount(&server)
            .await;

        let client = LiteLlmAdminClient::new(server.uri(), "test-key");
        let err = client.user_info().await.unwrap_err();
        match err {
            AixError::BudgetExceeded { spend, max_budget } => {
                assert!((spend - 50.16580493999999).abs() < f64::EPSILON);
                assert_eq!(max_budget, 50.0);
            }
            other => panic!("expected BudgetExceeded, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn trailing_slash_is_normalized_for_management_paths() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(KEY_INFO_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        let client = LiteLlmAdminClient::new(format!("{}/", server.uri()), "test-key");
        client.user_info().await.unwrap();
    }
}
