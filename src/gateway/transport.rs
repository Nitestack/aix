use crate::error::AixError;
use serde_json::Value;
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) struct GatewayTransport {
    base_url: String,
    api_key: String,
    inner: reqwest::Client,
}

pub(crate) enum TransportError {
    Http(reqwest::Error),
    Gateway { status: u16, body: String },
}

impl TransportError {
    pub(crate) fn into_aix_error(self) -> AixError {
        match self {
            Self::Http(error) => AixError::HttpError(error),
            Self::Gateway { status, body } => AixError::GatewayError { status, body },
        }
    }
}

impl From<reqwest::Error> for TransportError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error)
    }
}

impl GatewayTransport {
    pub(crate) fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            inner: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("reqwest client configuration is valid"),
        }
    }

    pub(crate) async fn get_json(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Value, TransportError> {
        let request = self
            .inner
            .get(self.url(path))
            .header("Authorization", format!("Bearer {}", self.api_key));
        let request = if query.is_empty() {
            request
        } else {
            request.query(query)
        };
        self.send_json(request).await
    }

    #[allow(dead_code)] // Used by the OpenAI-compatible capability client.
    pub(crate) async fn post_json(
        &self,
        path: &str,
        body: &Value,
    ) -> Result<Value, TransportError> {
        let request = self
            .inner
            .post(self.url(path))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(body);
        self.send_json(request).await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    async fn send_json(&self, request: reqwest::RequestBuilder) -> Result<Value, TransportError> {
        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(TransportError::Gateway {
                status: status.as_u16(),
                body,
            });
        }
        Ok(response.json().await?)
    }
}
