use super::transport::{GatewayTransport, TransportError};
use crate::error::AixError;
use serde_json::Value;
use std::time::Duration;

const MODELS_PATH: &str = "/v1/models";
const CHAT_COMPLETIONS_PATH: &str = "/v1/chat/completions";

pub(crate) struct OpenAiClient {
    transport: GatewayTransport,
}

impl OpenAiClient {
    pub(crate) fn with_timeout(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            transport: GatewayTransport::with_timeout(base_url, api_key, timeout),
        }
    }

    pub(crate) async fn models(&self) -> Result<Value, AixError> {
        self.transport
            .get_json(MODELS_PATH, &[])
            .await
            .map_err(TransportError::into_aix_error)
    }

    pub(crate) async fn chat_completions(&self, request: &Value) -> Result<Value, AixError> {
        self.transport
            .post_json(CHAT_COMPLETIONS_PATH, request)
            .await
            .map_err(|error| match error {
                TransportError::Http(error) if error.is_decode() => {
                    AixError::GatewayProtocolError("response was not valid JSON")
                }
                error => error.into_aix_error(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn models_uses_v1_path_and_bearer_auth() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("Authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "id": "example-model" }]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = OpenAiClient::with_timeout(server.uri(), "test-key", Duration::from_secs(30));
        assert_eq!(
            client.models().await.unwrap()["data"][0]["id"],
            "example-model"
        );
    }

    #[tokio::test]
    async fn chat_completions_uses_v1_path_and_bearer_auth() {
        let server = MockServer::start().await;
        let request = json!({
            "model": "example-model",
            "messages": [{ "role": "user", "content": "hello" }]
        });
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("Authorization", "Bearer test-key"))
            .and(body_json(request.clone()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{ "message": { "content": "hi" } }]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = OpenAiClient::with_timeout(server.uri(), "test-key", Duration::from_secs(30));
        assert_eq!(
            client.chat_completions(&request).await.unwrap()["choices"][0]["message"]["content"],
            "hi"
        );
    }

    #[tokio::test]
    async fn trailing_slash_is_normalized_for_openai_paths() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
            .expect(1)
            .mount(&server)
            .await;

        let client = OpenAiClient::with_timeout(
            format!("{}/", server.uri()),
            "test-key",
            Duration::from_secs(30),
        );
        client.models().await.unwrap();
    }
}
