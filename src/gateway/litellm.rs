use super::transport::{GatewayTransport, TransportError};
use crate::error::AixError;
use serde_json::Value;

const KEY_INFO_PATH: &str = "/key/info";
const KEY_LIST_PATH: &str = "/key/list";

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

fn map_admin_error(error: TransportError) -> AixError {
    match error {
        TransportError::Gateway { status: 429, body } => {
            if let Some((spend, max_budget)) = parse_budget_exceeded(&body) {
                AixError::BudgetExceeded { spend, max_budget }
            } else {
                AixError::GatewayError { status: 429, body }
            }
        }
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
        if let Some(value) = source.get(field) {
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
                .map(|value| {
                    if value.chars().count() <= 4 {
                        crate::cache::short_key_name(value)
                    } else {
                        format!("sk-...{}", crate::cache::key_suffix(value))
                    }
                })
        });

    let mut object = serde_json::Map::new();
    if let Some(key_name) = key_name {
        object.insert("key_name".to_string(), Value::String(key_name));
    }
    for field in ["spend", "max_budget"] {
        if let Some(value) = source.get(field) {
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
        assert_eq!(result["keys"][0]["key_name"], "sk-...1234");
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

        assert_eq!(entry["key_name"], crate::cache::short_key_name("abcd"));
        assert_eq!(entry["spend"], 1.23);
        let serialized = entry.to_string();
        for secret in ["abcd", "wxyz", "1234"] {
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
