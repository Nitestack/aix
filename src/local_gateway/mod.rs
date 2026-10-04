mod api_key;
mod context;
mod server;
mod stream;

pub(crate) use api_key::ApiKeyGatewayHandle;
pub(crate) use context::LaunchContext;
pub(crate) use server::ServerHandle;
pub(crate) use stream::{
    forward_response, forward_response_observed, forward_response_passthrough_observed,
    ResponseBodyObserver, ResponseStreamEnd,
};

use axum::body::{to_bytes, Body, Bytes};
use axum::http::{Response, StatusCode};

#[derive(Clone, Copy)]
pub(crate) struct AuthFailureResponse {
    pub(crate) error_type: &'static str,
    pub(crate) message: &'static str,
}

impl AuthFailureResponse {
    pub(crate) const fn new(error_type: &'static str, message: &'static str) -> Self {
        Self {
            error_type,
            message,
        }
    }
}

pub(crate) const MAX_REQUEST_BODY_BYTES: usize = 8 * 1024 * 1024;

pub(crate) async fn read_bounded_body(body: Body) -> Result<Bytes, axum::Error> {
    to_bytes(body, MAX_REQUEST_BODY_BYTES).await
}

pub(crate) fn error_response(
    status: StatusCode,
    error_type: &str,
    message: &str,
) -> Response<Body> {
    let body = serde_json::json!({
        "error": {
            "type": error_type,
            "message": message,
        }
    })
    .to_string();
    Response::builder()
        .status(status)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("static gateway error response is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bounds_buffered_request_bodies() {
        let body = Body::from(vec![b'x'; MAX_REQUEST_BODY_BYTES + 1]);

        assert!(read_bounded_body(body).await.is_err());
    }
}
