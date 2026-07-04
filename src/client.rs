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
        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AixError::GatewayError { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn spend_logs(&self, limit: u32) -> Result<serde_json::Value, AixError> {
        let url = format!("{}/spend/logs", self.base_url);
        let resp = self
            .inner
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .query(&[("limit", limit.to_string())])
            .send()
            .await?;
        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AixError::GatewayError { status, body });
        }
        Ok(resp.json().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
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
    async fn spend_logs_calls_correct_endpoint_with_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/spend/logs"))
            .and(header("Authorization", "Bearer test-key"))
            .and(query_param("limit", "25"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"model": "gpt-4", "spend": 0.01}
            ])))
            .mount(&server)
            .await;

        let client = LiteLlmClient::new(server.uri(), "test-key");
        let result = client.spend_logs(25).await.unwrap();
        assert_eq!(result[0]["model"], "gpt-4");
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
