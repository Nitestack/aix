use super::{error_response, read_bounded_body, AuthFailureResponse, LaunchContext, ServerHandle};
use crate::auth::AuthService;
use crate::usage_event::{UsageEventRecorder, UsageOutcome};
use crate::usage_observer::openai::OpenAiResponsesObserver;
use crate::usage_store::UsageStore;
use axum::body::Body;
use axum::extract::{Extension, State};
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::{Method, Request, Response, StatusCode};
use axum::Router;
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};
use url::Url;

mod compatibility;

pub(crate) use compatibility::normalize_response_request;
#[cfg(test)]
pub(crate) use compatibility::{CompatibilityError, UNSUPPORTED_RESPONSE_FIELDS};

const OPENAI_PUBLIC_API_BASE: &str = "https://api.openai.com/v1/";
const FORWARDED_RESPONSE_HEADERS: &[&str] = &[
    "content-type",
    "cache-control",
    "retry-after",
    "x-request-id",
    "openai-request-id",
    "openai-processing-ms",
    "openai-version",
];

struct BridgeState {
    auth: Arc<AuthService>,
    upstream_base: Url,
    client: reqwest::Client,
    usage_store: Option<UsageStore>,
    auth_failure: AuthFailureResponse,
}

/// ChatGPT-plan Responses transport shared by compatible process-local clients.
///
/// The production constructor always fixes the upstream to OpenAI's public API;
/// tests can provide a local mock without adding a runtime endpoint override.
pub(crate) struct ChatGptResponsesHandle {
    server: ServerHandle,
}

impl ChatGptResponsesHandle {
    pub(crate) async fn start(
        context: LaunchContext,
        timeout: Duration,
        auth_failure: AuthFailureResponse,
    ) -> color_eyre::Result<Self> {
        let auth = Arc::new(AuthService::new(timeout)?);
        let upstream_base = Url::parse(OPENAI_PUBLIC_API_BASE)?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .build()?;
        Self::bind(
            context,
            auth,
            upstream_base,
            client,
            UsageStore::from_environment_or_warn(),
            auth_failure,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn start_for_test(
        context: LaunchContext,
        auth: Arc<AuthService>,
        upstream_base: Url,
        auth_failure: AuthFailureResponse,
    ) -> color_eyre::Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .build()?;
        Self::bind(context, auth, upstream_base, client, None, auth_failure).await
    }

    #[cfg(test)]
    pub(crate) async fn start_for_test_with_usage_store(
        context: LaunchContext,
        auth: Arc<AuthService>,
        upstream_base: Url,
        usage_store: UsageStore,
        auth_failure: AuthFailureResponse,
    ) -> color_eyre::Result<Self> {
        Self::bind(
            context,
            auth,
            upstream_base,
            reqwest::Client::new(),
            Some(usage_store),
            auth_failure,
        )
        .await
    }

    async fn bind(
        context: LaunchContext,
        auth: Arc<AuthService>,
        upstream_base: Url,
        client: reqwest::Client,
        usage_store: Option<UsageStore>,
        auth_failure: AuthFailureResponse,
    ) -> color_eyre::Result<Self> {
        let state = Arc::new(BridgeState {
            auth,
            upstream_base,
            client,
            usage_store,
            auth_failure,
        });
        let app = Router::new().fallback(handle_request).with_state(state);
        let server = ServerHandle::start(app, context, auth_failure).await?;
        Ok(Self { server })
    }

    pub(crate) fn port(&self) -> u16 {
        self.server.port()
    }

    pub(crate) fn child_token(&self) -> crate::secrets::SecretString {
        self.server.child_token()
    }

    pub(crate) fn into_server(self) -> ServerHandle {
        self.server
    }

    #[cfg(test)]
    pub(crate) fn address(&self) -> std::net::SocketAddr {
        self.server.address()
    }

    #[cfg(test)]
    pub(crate) async fn stop(self) {
        self.server.stop().await;
    }
}

async fn handle_request(
    State(state): State<Arc<BridgeState>>,
    Extension(context): Extension<LaunchContext>,
    request: Request<Body>,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let is_inference = parts.uri.path() == "/v1/responses";
    let mut usage = is_inference
        .then(|| UsageEventRecorder::new(state.usage_store.clone(), &context, "openai_responses"));
    if is_inference {
        if let Err(rejection) = context.enforcement.check_deadline(Instant::now()) {
            let (status, category, message) = super::policy_rejection_details(rejection);
            reject_inference_request(&mut usage, status, category);
            return gateway_error(state.auth_failure.error_type, status, message);
        }
    }
    if parts.uri.query().is_some() {
        reject_inference_request(&mut usage, StatusCode::NOT_FOUND, "local_compatibility");
        return gateway_error(
            state.auth_failure.error_type,
            StatusCode::NOT_FOUND,
            "Route not found",
        );
    }

    let (endpoint, method, request_body) = match (&parts.method, parts.uri.path()) {
        (&Method::POST, "/v1/responses") => {
            let is_json = parts
                .headers
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("application/json"));
            if !is_json {
                reject_inference_request(
                    &mut usage,
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "local_compatibility",
                );
                return gateway_error(
                    state.auth_failure.error_type,
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "Expected JSON request",
                );
            }
            let body = match read_bounded_body(body).await {
                Ok(body) => body,
                Err(_) => {
                    reject_inference_request(
                        &mut usage,
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "local_compatibility",
                    );
                    return gateway_error(
                        state.auth_failure.error_type,
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "Request body too large",
                    );
                }
            };
            let mut request: Value = match serde_json::from_slice(&body) {
                Ok(request) => request,
                Err(_) => {
                    reject_inference_request(&mut usage, StatusCode::BAD_REQUEST, "invalid_json");
                    return gateway_error(
                        state.auth_failure.error_type,
                        StatusCode::BAD_REQUEST,
                        "Invalid JSON request",
                    );
                }
            };
            if let Some(recorder) = &mut usage {
                recorder.set_model(request.get("model").and_then(Value::as_str));
            }
            if let Err(rejection) = context
                .enforcement
                .check_model(request.get("model").and_then(Value::as_str))
            {
                let (status, category, message) = super::policy_rejection_details(rejection);
                reject_inference_request(&mut usage, status, category);
                return gateway_error(state.auth_failure.error_type, status, message);
            }
            let normalized = match normalize_response_request(&mut request) {
                Ok(request) => request,
                Err(error) => {
                    reject_inference_request(
                        &mut usage,
                        StatusCode::BAD_REQUEST,
                        "local_compatibility",
                    );
                    return gateway_error(
                        state.auth_failure.error_type,
                        StatusCode::BAD_REQUEST,
                        error.to_string().as_str(),
                    );
                }
            };
            let body = match serde_json::to_vec(&normalized) {
                Ok(body) => body,
                Err(_) => {
                    reject_inference_request(&mut usage, StatusCode::BAD_REQUEST, "invalid_json");
                    return gateway_error(
                        state.auth_failure.error_type,
                        StatusCode::BAD_REQUEST,
                        "Invalid JSON request",
                    );
                }
            };
            ("responses", Method::POST, Some(body))
        }
        (&Method::GET, "/v1/models") => ("models", Method::GET, None),
        (_, "/v1/responses") => {
            reject_inference_request(
                &mut usage,
                StatusCode::METHOD_NOT_ALLOWED,
                "local_compatibility",
            );
            return gateway_error(
                state.auth_failure.error_type,
                StatusCode::METHOD_NOT_ALLOWED,
                "Method not allowed",
            );
        }
        (_, "/v1/models") => {
            return gateway_error(
                state.auth_failure.error_type,
                StatusCode::METHOD_NOT_ALLOWED,
                "Method not allowed",
            )
        }
        _ => {
            return gateway_error(
                state.auth_failure.error_type,
                StatusCode::NOT_FOUND,
                "Route not found",
            )
        }
    };

    if is_inference {
        if let Err(rejection) = context.enforcement.check_deadline(Instant::now()) {
            let (status, category, message) = super::policy_rejection_details(rejection);
            reject_inference_request(&mut usage, status, category);
            return gateway_error(state.auth_failure.error_type, status, message);
        }
    }

    proxy_upstream(
        state,
        &context.profile,
        endpoint,
        method,
        request_body,
        usage,
    )
    .await
}

fn reject_inference_request(
    recorder: &mut Option<UsageEventRecorder>,
    status: StatusCode,
    category: &str,
) {
    if let Some(mut recorder) = recorder.take() {
        recorder.finish(UsageOutcome::Failed, Some(status.as_u16()), Some(category));
    }
}

async fn proxy_upstream(
    state: Arc<BridgeState>,
    profile_name: &str,
    endpoint: &str,
    method: Method,
    request_body: Option<Vec<u8>>,
    mut usage: Option<UsageEventRecorder>,
) -> Response<Body> {
    let token = match state.auth.access_token(profile_name).await {
        Ok(token) => token,
        Err(_) => {
            if let Some(mut recorder) = usage.take() {
                recorder.finish(
                    UsageOutcome::Failed,
                    Some(StatusCode::BAD_GATEWAY.as_u16()),
                    Some("authentication"),
                );
            }
            return gateway_error(
                state.auth_failure.error_type,
                StatusCode::BAD_GATEWAY,
                "Could not obtain the selected ChatGPT profile's access token",
            );
        }
    };
    let Ok(url) = state.upstream_base.join(endpoint) else {
        if let Some(mut recorder) = usage.take() {
            recorder.finish(
                UsageOutcome::Failed,
                Some(StatusCode::BAD_GATEWAY.as_u16()),
                Some("upstream_transport"),
            );
        }
        return gateway_error(
            state.auth_failure.error_type,
            StatusCode::BAD_GATEWAY,
            "Upstream request failed",
        );
    };
    let mut request = state
        .client
        .request(method, url)
        .bearer_auth(token.expose_secret());
    if let Some(body) = request_body {
        request = request
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "text/event-stream")
            .body(body);
    } else {
        request = request.header(ACCEPT, "application/json");
    }

    let upstream = match request.send().await {
        Ok(response) => response,
        Err(_) => {
            if let Some(mut recorder) = usage.take() {
                recorder.finish(
                    UsageOutcome::Failed,
                    Some(StatusCode::BAD_GATEWAY.as_u16()),
                    Some("upstream_transport"),
                );
            }
            return gateway_error(
                state.auth_failure.error_type,
                StatusCode::BAD_GATEWAY,
                "Upstream request failed",
            );
        }
    };
    if endpoint == "responses" {
        let status = upstream.status().as_u16();
        let observer = Box::new(OpenAiResponsesObserver::new(
            usage
                .take()
                .expect("Responses request has a usage recorder"),
            status,
        ));
        return match super::forward_response_observed(
            upstream,
            FORWARDED_RESPONSE_HEADERS,
            observer,
        ) {
            Ok(response) => response,
            Err(_) => gateway_error(
                state.auth_failure.error_type,
                StatusCode::BAD_GATEWAY,
                "Upstream response failed",
            ),
        };
    }
    match super::forward_response(upstream, FORWARDED_RESPONSE_HEADERS) {
        Ok(response) => response,
        Err(_) => gateway_error(
            state.auth_failure.error_type,
            StatusCode::BAD_GATEWAY,
            "Upstream response failed",
        ),
    }
}

fn gateway_error(error_type: &str, status: StatusCode, message: &str) -> Response<Body> {
    error_response(status, error_type, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_upstream_is_the_public_openai_responses_api() {
        let upstream = Url::parse(OPENAI_PUBLIC_API_BASE).unwrap();
        assert_eq!(upstream.scheme(), "https");
        assert_eq!(upstream.host_str(), Some("api.openai.com"));
        assert_eq!(upstream.path(), "/v1/");
    }
}
