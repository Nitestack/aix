use crate::error::AixError;
use serde_json::Value;
use std::time::Duration;

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
    pub(crate) fn with_timeout(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            inner: reqwest::Client::builder()
                .timeout(timeout)
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
            let body = response.text().await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[tokio::test]
    async fn timeout_while_reading_error_body_remains_a_network_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let responder = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            stream
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 7\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            stream.flush().unwrap();
            std::thread::sleep(Duration::from_millis(200));
            let _ = stream.write_all(b"delayed");
        });

        let transport = GatewayTransport::with_timeout(
            format!("http://{address}"),
            "test-key",
            Duration::from_millis(50),
        );
        let error = transport.get_json("/error", &[]).await.unwrap_err();
        match error {
            TransportError::Http(error) => {
                assert!(error.is_timeout());
                assert_eq!(AixError::HttpError(error).exit_code(), 5);
            }
            _ => panic!("expected an HTTP timeout while reading the error body"),
        }
        responder.join().unwrap();
    }
}
