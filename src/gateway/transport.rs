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
    Protocol,
    Gateway {
        status: u16,
        body: String,
        safe_body: String,
    },
}

impl TransportError {
    pub(crate) fn into_aix_error(self) -> AixError {
        match self {
            Self::Http(error) => AixError::HttpError(error.without_url()),
            Self::Protocol => {
                AixError::GatewayProtocolError("gateway returned a malformed JSON response")
            }
            Self::Gateway {
                status, safe_body, ..
            } => AixError::GatewayError {
                status,
                body: safe_body,
            },
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
        let request = self.authenticated(self.inner.get(self.url(path)));
        let request = if query.is_empty() {
            request
        } else {
            request.query(query)
        };
        self.send_json(request).await
    }

    pub(crate) async fn post_json(
        &self,
        path: &str,
        body: &Value,
    ) -> Result<Value, TransportError> {
        let request = self
            .authenticated(self.inner.post(self.url(path)))
            .json(body);
        self.send_json(request).await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn authenticated(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request.bearer_auth(&self.api_key)
    }

    async fn send_json(&self, request: reqwest::RequestBuilder) -> Result<Value, TransportError> {
        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(TransportError::Gateway {
                status: status.as_u16(),
                safe_body: sanitize_error_body(&body, &self.api_key, &self.base_url),
                body,
            });
        }
        let body = response.bytes().await?;
        serde_json::from_slice(&body).map_err(|_| TransportError::Protocol)
    }
}

fn sanitize_error_body(body: &str, api_key: &str, base_url: &str) -> String {
    if let Ok(mut value) = serde_json::from_str::<Value>(body) {
        redact_error_value(&mut value, api_key, base_url);
        return serde_json::to_string(&value)
            .unwrap_or_else(|_| "[gateway response omitted]".to_string());
    }

    redact_sensitive_text(body, api_key, base_url)
}

fn redact_error_value(value: &mut Value, api_key: &str, base_url: &str) {
    match value {
        Value::Object(object) => object.retain(|field, value| {
            if is_sensitive_error_field(field)
                || (!api_key.is_empty() && field.contains(api_key))
                || (!base_url.is_empty() && field.contains(base_url))
            {
                return false;
            }
            redact_error_value(value, api_key, base_url);
            true
        }),
        Value::Array(values) => {
            for value in values {
                redact_error_value(value, api_key, base_url);
            }
        }
        Value::String(value) => {
            *value = redact_sensitive_text(value, api_key, base_url);
        }
        _ => {}
    }
}

fn redact_sensitive_text(value: &str, api_key: &str, base_url: &str) -> String {
    let value = if base_url.is_empty() {
        value.to_string()
    } else {
        value.replace(base_url, "[redacted]")
    };
    if api_key.is_empty() {
        value
    } else {
        value.replace(api_key, "[redacted]")
    }
}

fn is_sensitive_error_field(field: &str) -> bool {
    let field = field.to_ascii_lowercase();
    [
        "key",
        "token",
        "secret",
        "credential",
        "authorization",
        "metadata",
    ]
    .iter()
    .any(|sensitive| field.contains(sensitive))
}
