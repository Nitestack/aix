use crate::error::AixError;

pub struct LiteLlmClient {
    base_url: String,
    api_key: String,
    inner: reqwest::Client,
}

impl LiteLlmClient {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            inner: reqwest::Client::new(),
        }
    }

    pub async fn user_info(&self) -> Result<serde_json::Value, AixError> {
        let url = format!("{}/user/info", self.base_url);
        let resp = self
            .inner
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let code = status.as_u16();
            let body = resp.text().await.unwrap_or_default();
            if code == 429 {
                if let Some((spend, max_budget)) = parse_budget_exceeded(&body) {
                    return Err(AixError::BudgetExceeded { spend, max_budget });
                }
            }
            return Err(AixError::GatewayError { status: code, body });
        }
        Ok(resp.json().await?)
    }
}

/// LiteLLM reports budget_exceeded as a 429 whose `error.message` embeds the
/// figures as free text (e.g. "Current cost: 50.17, Max budget: 50.0")
/// rather than as structured fields, so this pulls them back out.
fn parse_budget_exceeded(body: &str) -> Option<(f64, f64)> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let error = value.get("error")?;
    if error.get("type").and_then(|t| t.as_str()) != Some("budget_exceeded") {
        return None;
    }
    let message = error.get("message").and_then(|m| m.as_str())?;
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
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn user_info_calls_correct_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/info"))
            .and(header("Authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "user_id": "u123",
                "spend": 1.23
            })))
            .mount(&server)
            .await;

        let client = LiteLlmClient::new(server.uri(), "test-key");
        let result = client.user_info().await.unwrap();
        assert_eq!(result["user_id"], "u123");
        assert_eq!(result["spend"], 1.23);
    }

    #[tokio::test]
    async fn non_200_response_returns_gateway_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/info"))
            .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
            .mount(&server)
            .await;

        let client = LiteLlmClient::new(server.uri(), "test-key");
        let err = client.user_info().await.unwrap_err();
        assert!(matches!(err, AixError::GatewayError { status: 403, .. }));
    }

    #[tokio::test]
    async fn budget_exceeded_429_returns_parsed_figures() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/info"))
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

        let client = LiteLlmClient::new(server.uri(), "test-key");
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
    async fn base_url_trailing_slash_is_normalized() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;

        let uri_with_slash = format!("{}/", server.uri());
        let client = LiteLlmClient::new(uri_with_slash, "k");
        client.user_info().await.unwrap();
    }
}
