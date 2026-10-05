use super::{
    error_response, policy_rejection_details, read_bounded_body, AuthFailureResponse,
    LaunchContext, ServerHandle,
};
use crate::commands::launch::LaunchEnv;
use crate::config::ApiFormat;
use crate::local_gateway::{forward_response_passthrough_observed, ResponseBodyObserver};
use crate::secrets::SecretString;
use crate::usage_event::{UsageEventRecorder, UsageOutcome};
use crate::usage_observer::anthropic::AnthropicMessagesObserver;
use crate::usage_observer::openai::{OpenAiProtocol, OpenAiResponsesObserver};
use crate::usage_store::UsageStore;
use axum::body::Body;
use axum::extract::{Extension, Request, State};
use axum::http::header::CONNECTION;
use axum::http::{HeaderMap, Method, Response, StatusCode};
use axum::Router;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use url::Url;

const CHAT_COMPLETIONS_PATH: &str = "/v1/chat/completions";
const MODELS_PATH: &str = "/v1/models";
const RESPONSES_PATH: &str = "/v1/responses";
const MESSAGES_PATH: &str = "/v1/messages";

const AUTH_ENV_VARS: &[&str] = &["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "LITELLM_API_KEY"];
const URL_ENV_VARS: &[&str] = &["ANTHROPIC_BASE_URL", "OPENAI_BASE_URL", "LITELLM_BASE_URL"];

struct ApiKeyGatewayState {
    api_format: ApiFormat,
    upstream_base_url: String,
    upstream_api_key: SecretString,
    client: reqwest::Client,
    usage_store: Option<UsageStore>,
}

pub(crate) struct ApiKeyGatewayHandle {
    server: ServerHandle,
    api_format: ApiFormat,
}

impl ApiKeyGatewayHandle {
    pub(crate) async fn start(
        context: LaunchContext,
        api_format: ApiFormat,
        upstream_base_url: &str,
        upstream_api_key: &str,
    ) -> color_eyre::Result<Self> {
        Self::bind(
            context,
            api_format,
            upstream_base_url,
            upstream_api_key,
            UsageStore::from_environment_or_warn(),
        )
        .await
    }

    #[cfg(test)]
    async fn start_for_test(
        context: LaunchContext,
        api_format: ApiFormat,
        upstream_base_url: &str,
        upstream_api_key: &str,
        usage_store: Option<UsageStore>,
    ) -> color_eyre::Result<Self> {
        Self::bind(
            context,
            api_format,
            upstream_base_url,
            upstream_api_key,
            usage_store,
        )
        .await
    }

    async fn bind(
        context: LaunchContext,
        api_format: ApiFormat,
        upstream_base_url: &str,
        upstream_api_key: &str,
        usage_store: Option<UsageStore>,
    ) -> color_eyre::Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .build()?;
        let state = Arc::new(ApiKeyGatewayState {
            api_format,
            upstream_base_url: upstream_base_url.to_owned(),
            upstream_api_key: SecretString::new(upstream_api_key.to_owned()),
            client,
            usage_store,
        });
        let app = Router::new().fallback(handle_request).with_state(state);
        let server = ServerHandle::start(
            app,
            context,
            AuthFailureResponse::new("aix_local_gateway_error", "Invalid local credential"),
        )
        .await?;
        Ok(Self { server, api_format })
    }

    pub(crate) fn configure_env(&self, env: &mut LaunchEnv) {
        let local_url = format!("http://127.0.0.1:{}", self.server.port());
        let local_v1_url = format!("{local_url}/v1");
        let local_token = self.server.child_token();

        // Do not let inherited or configured profile/tool credentials bypass the
        // selected profile. LiteLLM aliases remain available, but point locally.
        env.vars
            .retain(|(name, _)| !is_transport_credential_var(name));
        env.auth_vars
            .retain(|(name, _)| !is_transport_credential_var(name));
        env.clear_vars
            .extend(AUTH_ENV_VARS.iter().map(|name| name.to_string()));
        env.clear_vars
            .extend(URL_ENV_VARS.iter().map(|name| name.to_string()));
        env.clear_vars.sort_unstable();
        env.clear_vars.dedup();

        if self.api_format.supports_openai() {
            env.vars
                .push(("OPENAI_BASE_URL".to_string(), local_v1_url.clone()));
            env.auth_vars.push((
                "OPENAI_API_KEY".to_string(),
                SecretString::new(local_token.expose_secret().to_owned()),
            ));
        }
        if self.api_format.supports_anthropic() {
            env.vars.push(("ANTHROPIC_BASE_URL".to_string(), local_url));
            env.auth_vars.push((
                "ANTHROPIC_API_KEY".to_string(),
                SecretString::new(local_token.expose_secret().to_owned()),
            ));
        }
        env.vars
            .push(("LITELLM_BASE_URL".to_string(), local_v1_url));
        env.auth_vars.push((
            "LITELLM_API_KEY".to_string(),
            SecretString::new(local_token.expose_secret().to_owned()),
        ));
    }

    pub(crate) fn into_server(self) -> ServerHandle {
        self.server
    }

    #[cfg(test)]
    fn address(&self) -> std::net::SocketAddr {
        self.server.address()
    }

    #[cfg(test)]
    fn child_token(&self) -> SecretString {
        self.server.child_token()
    }

    pub(crate) fn print_dry_run(api_format: ApiFormat) {
        let format = match api_format {
            ApiFormat::OpenAi => "openai",
            ApiFormat::Anthropic => "anthropic",
            ApiFormat::Both => "openai+anthropic",
        };
        eprintln!("local gateway: {format}");
    }

    pub(crate) fn dry_run_variable_names(api_format: ApiFormat) -> Vec<String> {
        let mut names = vec![
            "LITELLM_API_KEY".to_string(),
            "LITELLM_BASE_URL".to_string(),
        ];
        if api_format.supports_openai() {
            names.extend(["OPENAI_API_KEY".to_string(), "OPENAI_BASE_URL".to_string()]);
        }
        if api_format.supports_anthropic() {
            names.extend([
                "ANTHROPIC_API_KEY".to_string(),
                "ANTHROPIC_BASE_URL".to_string(),
            ]);
        }
        names
    }
}

fn is_transport_credential_var(name: &str) -> bool {
    AUTH_ENV_VARS
        .iter()
        .chain(URL_ENV_VARS)
        .any(|candidate| *candidate == name)
}

async fn handle_request(
    State(state): State<Arc<ApiKeyGatewayState>>,
    Extension(context): Extension<LaunchContext>,
    request: Request<Body>,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path();
    let protocol = match path {
        CHAT_COMPLETIONS_PATH => Some("openai_chat_completions"),
        RESPONSES_PATH => Some("openai_responses"),
        MESSAGES_PATH => Some("anthropic_messages"),
        _ => None,
    };
    let mut usage =
        protocol.map(|name| UsageEventRecorder::new(state.usage_store.clone(), &context, name));

    let route = match classify_route(&parts.method, path, state.api_format) {
        RouteDecision::Allowed(route) => route,
        RouteDecision::MethodNotAllowed => {
            finish_local_rejection(&mut usage, StatusCode::METHOD_NOT_ALLOWED);
            return error_response(
                StatusCode::METHOD_NOT_ALLOWED,
                "local_gateway_error",
                "Method not allowed",
            );
        }
        RouteDecision::NotFound => {
            finish_local_rejection(&mut usage, StatusCode::NOT_FOUND);
            return error_response(
                StatusCode::NOT_FOUND,
                "local_gateway_error",
                "Route not found",
            );
        }
    };

    if route.is_inference() {
        if let Err(rejection) = context.enforcement.check_deadline(Instant::now()) {
            let (status, category, message) = policy_rejection_details(rejection);
            return policy_rejection(&mut usage, status, category, message);
        }
    }

    let request_body = if route.is_inference() {
        let bytes = match read_bounded_body(body).await {
            Ok(bytes) => bytes,
            Err(_) => {
                finish_local_rejection(&mut usage, StatusCode::PAYLOAD_TOO_LARGE);
                return error_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "local_gateway_error",
                    "Request body too large",
                );
            }
        };
        let parsed = serde_json::from_slice::<Value>(&bytes).ok();
        let model = parsed
            .as_ref()
            .and_then(|value| value.get("model"))
            .and_then(Value::as_str);
        if let Some(recorder) = &mut usage {
            recorder.set_model(model);
        }
        if let Err(rejection) = context.enforcement.check_model(model) {
            let (status, category, message) = policy_rejection_details(rejection);
            return policy_rejection(&mut usage, status, category, message);
        }
        Some(bytes)
    } else {
        None
    };

    if route.is_inference() {
        if let Err(rejection) = context.enforcement.check_deadline(Instant::now()) {
            let (status, category, message) = policy_rejection_details(rejection);
            return policy_rejection(&mut usage, status, category, message);
        }
    }

    let url = match upstream_url(
        &state.upstream_base_url,
        route.upstream_path(),
        parts.uri.query(),
    ) {
        Some(url) => url,
        None => {
            finish_transport_failure(&mut usage);
            return error_response(
                StatusCode::BAD_GATEWAY,
                "local_gateway_error",
                "Upstream request failed",
            );
        }
    };
    let headers = forwarded_request_headers(&parts.headers);
    let mut upstream_request = state.client.request(parts.method, url).headers(headers);
    upstream_request = match route {
        Route::Models | Route::ChatCompletions | Route::Responses => {
            upstream_request.bearer_auth(state.upstream_api_key.expose_secret())
        }
        Route::Messages => {
            upstream_request.header("x-api-key", state.upstream_api_key.expose_secret())
        }
    };
    if let Some(request_body) = request_body {
        upstream_request = upstream_request.body(request_body);
    }

    let upstream = match upstream_request.send().await {
        Ok(response) => response,
        Err(_) => {
            finish_transport_failure(&mut usage);
            return error_response(
                StatusCode::BAD_GATEWAY,
                "local_gateway_error",
                "Upstream request failed",
            );
        }
    };

    if !route.is_inference() {
        return match super::forward_response_passthrough_observed(upstream, Box::new(NoopObserver))
        {
            Ok(response) => response,
            Err(_) => error_response(
                StatusCode::BAD_GATEWAY,
                "local_gateway_error",
                "Upstream response failed",
            ),
        };
    }

    let status = upstream.status().as_u16();
    let is_sse = upstream
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
        });
    let observer: Box<dyn ResponseBodyObserver> = match route {
        Route::ChatCompletions => Box::new(OpenAiResponsesObserver::for_protocol(
            usage
                .take()
                .expect("inference request has a usage recorder"),
            status,
            OpenAiProtocol::ChatCompletions,
            is_sse,
        )),
        Route::Responses => Box::new(OpenAiResponsesObserver::for_protocol(
            usage
                .take()
                .expect("inference request has a usage recorder"),
            status,
            OpenAiProtocol::Responses,
            is_sse,
        )),
        Route::Messages => Box::new(AnthropicMessagesObserver::new(
            usage
                .take()
                .expect("inference request has a usage recorder"),
            status,
            is_sse,
        )),
        Route::Models => unreachable!("models is not an inference route"),
    };
    match forward_response_passthrough_observed(upstream, observer) {
        Ok(response) => response,
        Err(_) => error_response(
            StatusCode::BAD_GATEWAY,
            "local_gateway_error",
            "Upstream response failed",
        ),
    }
}

#[derive(Clone, Copy)]
enum Route {
    Models,
    ChatCompletions,
    Responses,
    Messages,
}

impl Route {
    fn is_inference(self) -> bool {
        !matches!(self, Self::Models)
    }

    fn upstream_path(self) -> &'static str {
        match self {
            Self::Models => MODELS_PATH,
            Self::ChatCompletions => CHAT_COMPLETIONS_PATH,
            Self::Responses => RESPONSES_PATH,
            Self::Messages => MESSAGES_PATH,
        }
    }
}

enum RouteDecision {
    Allowed(Route),
    MethodNotAllowed,
    NotFound,
}

fn classify_route(method: &Method, path: &str, api_format: ApiFormat) -> RouteDecision {
    let (route, expected_method, supported) = match path {
        MODELS_PATH => (Route::Models, Method::GET, api_format.supports_openai()),
        CHAT_COMPLETIONS_PATH => (
            Route::ChatCompletions,
            Method::POST,
            api_format.supports_openai(),
        ),
        RESPONSES_PATH => (Route::Responses, Method::POST, api_format.supports_openai()),
        MESSAGES_PATH => (
            Route::Messages,
            Method::POST,
            api_format.supports_anthropic(),
        ),
        _ => return RouteDecision::NotFound,
    };
    if !supported {
        RouteDecision::NotFound
    } else if method != expected_method {
        RouteDecision::MethodNotAllowed
    } else {
        RouteDecision::Allowed(route)
    }
}

fn finish_local_rejection(recorder: &mut Option<UsageEventRecorder>, status: StatusCode) {
    if let Some(mut recorder) = recorder.take() {
        recorder.finish(
            UsageOutcome::Failed,
            Some(status.as_u16()),
            Some("local_compatibility"),
        );
    }
}

fn policy_rejection(
    recorder: &mut Option<UsageEventRecorder>,
    status: StatusCode,
    category: &str,
    message: &str,
) -> Response<Body> {
    if let Some(mut recorder) = recorder.take() {
        recorder.finish(UsageOutcome::Failed, Some(status.as_u16()), Some(category));
    }
    error_response(status, "local_gateway_policy_error", message)
}

fn finish_transport_failure(recorder: &mut Option<UsageEventRecorder>) {
    if let Some(mut recorder) = recorder.take() {
        recorder.finish(
            UsageOutcome::Failed,
            Some(StatusCode::BAD_GATEWAY.as_u16()),
            Some("upstream_transport"),
        );
    }
}

fn upstream_url(base_url: &str, path: &str, request_query: Option<&str>) -> Option<Url> {
    let mut url = Url::parse(base_url).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    let mut upstream_path = url.path().trim_end_matches('/').to_owned();
    upstream_path.push_str(path);
    url.set_path(&upstream_path);
    url.set_fragment(None);
    if let Some(request_query) = request_query {
        let query = match url.query() {
            Some(base_query) if !base_query.is_empty() => {
                format!("{base_query}&{request_query}")
            }
            _ => request_query.to_owned(),
        };
        url.set_query(Some(&query));
    }
    Some(url)
}

fn forwarded_request_headers(headers: &HeaderMap) -> HeaderMap {
    let connection_headers = connection_header_names(headers);
    let mut forwarded = HeaderMap::new();
    for (name, value) in headers {
        if should_forward_request_header(name.as_str(), &connection_headers) {
            forwarded.append(name.clone(), value.clone());
        }
    }
    forwarded
}

fn should_forward_request_header(name: &str, connection_headers: &HashSet<String>) -> bool {
    !matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization"
            | "x-api-key"
            | "host"
            | "content-length"
            | "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    ) && !connection_headers.contains(&name.to_ascii_lowercase())
}

fn connection_header_names(headers: &HeaderMap) -> HashSet<String> {
    headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .collect()
}

struct NoopObserver;

impl ResponseBodyObserver for NoopObserver {
    fn observe(&mut self, _bytes: &[u8]) {}

    fn finish(&mut self, _stream_end: super::ResponseStreamEnd) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_event::UsageCompleteness;
    use crate::usage_store::UsageEventFilter;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::header::AUTHORIZATION;
    use axum::routing::post;
    use futures_util::stream;
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio::sync::Notify;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const UPSTREAM_KEY: &str = "parent-profile-secret";

    fn context() -> LaunchContext {
        LaunchContext::new(
            "work".to_string(),
            "review".to_string(),
            Some("run-123".to_string()),
            Some("bounded".to_string()),
        )
    }

    async fn gateway(
        format: ApiFormat,
        upstream: &str,
        store: Option<UsageStore>,
    ) -> ApiKeyGatewayHandle {
        gateway_with_context(context(), format, upstream, store).await
    }

    async fn gateway_with_context(
        context: LaunchContext,
        format: ApiFormat,
        upstream: &str,
        store: Option<UsageStore>,
    ) -> ApiKeyGatewayHandle {
        ApiKeyGatewayHandle::start_for_test(context, format, upstream, UPSTREAM_KEY, store)
            .await
            .unwrap()
    }

    fn all_events(store: &UsageStore) -> Vec<crate::usage_event::LocalUsageEvent> {
        store
            .events(&UsageEventFilter {
                start_unix_ms: 0,
                end_unix_ms: u64::MAX,
                ..UsageEventFilter::default()
            })
            .unwrap()
    }

    #[tokio::test]
    async fn openai_chat_requests_replace_auth_preserve_body_and_record_usage() {
        let upstream = MockServer::start().await;
        let state = assert_fs::TempDir::new().unwrap();
        let usage_store = UsageStore::new(state.path());
        let response_body = br#"{"id":"resp-secret","choices":[{"message":{"content":"response-marker"}}],"usage":{"prompt_tokens":12,"completion_tokens":4,"total_tokens":16,"prompt_tokens_details":{"cached_tokens":3}}}"#.to_vec();
        Mock::given(method("POST"))
            .and(path(CHAT_COMPLETIONS_PATH))
            .and(header(AUTHORIZATION, format!("Bearer {UPSTREAM_KEY}")))
            .and(header("x-aix-forwarded", "preserve-me"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-request-id", "upstream-request")
                    .set_body_raw(response_body.clone(), "application/json"),
            )
            .expect(1)
            .mount(&upstream)
            .await;
        let response_response_body =
            br#"{"id":"responses-secret","usage":{"input_tokens":5,"output_tokens":2}}"#.to_vec();
        Mock::given(method("POST"))
            .and(path(RESPONSES_PATH))
            .and(header(AUTHORIZATION, format!("Bearer {UPSTREAM_KEY}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(response_response_body.clone(), "application/json"),
            )
            .expect(1)
            .mount(&upstream)
            .await;
        Mock::given(method("GET"))
            .and(path(MODELS_PATH))
            .and(header(AUTHORIZATION, format!("Bearer {UPSTREAM_KEY}")))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"data":[]}"#))
            .expect(1)
            .mount(&upstream)
            .await;

        let gateway = gateway(
            ApiFormat::OpenAi,
            &upstream.uri(),
            Some(usage_store.clone()),
        )
        .await;
        let address = gateway.address();
        let local_key = gateway.child_token();
        let request_body =
            br#"{ "model": "model-7", "messages": [{"role":"user","content":"prompt-marker"}] }"#;
        let client = reqwest::Client::new();
        let response = client
            .post(format!(
                "http://{address}{CHAT_COMPLETIONS_PATH}?trace=kept"
            ))
            .header(
                "authorization",
                format!("Bearer {}", local_key.expose_secret()),
            )
            .header("host", "caller-controlled.invalid")
            .header("x-aix-forwarded", "preserve-me")
            .body(request_body.to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-request-id"], "upstream-request");
        assert_eq!(response.bytes().await.unwrap().as_ref(), response_body);

        let responses = client
            .post(format!("http://{address}{RESPONSES_PATH}"))
            .bearer_auth(local_key.expose_secret())
            .header("content-type", "application/json")
            .body(r#"{"model":"responses-model","input":"private"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(responses.status(), StatusCode::OK);
        assert_eq!(
            responses.bytes().await.unwrap().as_ref(),
            response_response_body
        );

        let models = client
            .get(format!("http://{address}{MODELS_PATH}"))
            .bearer_auth(local_key.expose_secret())
            .send()
            .await
            .unwrap();
        let models_status = models.status();
        let models_body = models.text().await.unwrap();
        assert_eq!(models_status, StatusCode::OK, "{models_body}");
        let received = upstream.received_requests().await.unwrap();
        let inference_request = received
            .iter()
            .find(|request| request.url.path() == CHAT_COMPLETIONS_PATH)
            .unwrap();
        assert_eq!(inference_request.body, request_body);
        assert_ne!(
            inference_request.url.host_str(),
            Some("caller-controlled.invalid")
        );
        assert_eq!(
            inference_request.headers["authorization"],
            format!("Bearer {UPSTREAM_KEY}")
        );
        assert!(!inference_request.headers.values().any(|value| {
            value
                .to_str()
                .is_ok_and(|value| value.contains(local_key.expose_secret()))
        }));

        let events = all_events(&usage_store);
        assert_eq!(events.len(), 2, "model discovery is not an inference event");
        assert_eq!(events[0].protocol, "openai_chat_completions");
        assert_eq!(events[0].model.as_deref(), Some("model-7"));
        assert_eq!(events[0].outcome, UsageOutcome::Succeeded, "{events:?}");
        assert_eq!(events[0].input_tokens_total, Some(12));
        assert_eq!(events[0].input_tokens_uncached, Some(9));
        assert_eq!(events[0].cache_read_input_tokens, Some(3));
        assert_eq!(events[0].output_tokens, Some(4));
        assert_eq!(events[0].total_tokens, Some(16));
        assert_eq!(events[0].usage_completeness, UsageCompleteness::Complete);
        assert_eq!(events[1].protocol, "openai_responses");
        assert_eq!(events[1].model.as_deref(), Some("responses-model"));
        assert_eq!(events[1].input_tokens_total, Some(5));
        assert_eq!(events[1].output_tokens, Some(2));
        assert_eq!(events[1].total_tokens, Some(7));
        assert_eq!(events[1].outcome, UsageOutcome::Succeeded);

        let event_dir = std::fs::read_dir(state.path().join("usage/events"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let event_file = std::fs::read_dir(event_dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        let persisted = std::fs::read_to_string(event_file.path()).unwrap();
        for marker in [
            "prompt-marker",
            "response-marker",
            "resp-secret",
            "responses-secret",
            "private",
            UPSTREAM_KEY,
        ] {
            assert!(!persisted.contains(marker), "persisted {marker}");
        }
    }

    #[tokio::test]
    async fn policy_model_allowlist_is_shared_by_openai_and_anthropic_routes() {
        let upstream = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "response",
                "choices": [],
                "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
            })))
            .expect(2)
            .mount(&upstream)
            .await;
        let context = LaunchContext::with_enforcement(
            "work".to_string(),
            "review".to_string(),
            Some("run-policy-models".to_string()),
            Some("bounded".to_string()),
            super::super::RequestEnforcement {
                allowed_models: vec!["resolved-model".to_string()],
                deadline: None,
            },
        );
        let gateway = gateway_with_context(context, ApiFormat::Both, &upstream.uri(), None).await;
        let client = reqwest::Client::new();
        let address = gateway.address();
        let token = gateway.child_token();

        for path in [CHAT_COMPLETIONS_PATH, MESSAGES_PATH] {
            let allowed = client
                .post(format!("http://{address}{path}"))
                .bearer_auth(token.expose_secret())
                .json(&serde_json::json!({ "model": "resolved-model" }))
                .send()
                .await
                .unwrap();
            assert_eq!(allowed.status(), StatusCode::OK, "{path}");
            let _ = allowed.bytes().await.unwrap();

            let switched = client
                .post(format!("http://{address}{path}"))
                .bearer_auth(token.expose_secret())
                .json(&serde_json::json!({ "model": "other-model" }))
                .send()
                .await
                .unwrap();
            assert_eq!(switched.status(), StatusCode::FORBIDDEN, "{path}");
            assert!(switched
                .text()
                .await
                .unwrap()
                .contains("not allowed by the selected run policy"));

            let missing = client
                .post(format!("http://{address}{path}"))
                .bearer_auth(token.expose_secret())
                .json(&serde_json::json!({ "messages": [] }))
                .send()
                .await
                .unwrap();
            assert_eq!(missing.status(), StatusCode::BAD_REQUEST, "{path}");
            assert!(missing
                .text()
                .await
                .unwrap()
                .contains("request model is required"));
        }

        assert_eq!(upstream.received_requests().await.unwrap().len(), 2);
        gateway.into_server().stop().await;
    }

    #[tokio::test]
    async fn expired_run_deadline_rejects_inference_before_upstream_forwarding() {
        let upstream = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&upstream)
            .await;
        let context = LaunchContext::with_enforcement(
            "work".to_string(),
            "review".to_string(),
            Some("run-expired".to_string()),
            Some("bounded".to_string()),
            super::super::RequestEnforcement {
                allowed_models: Vec::new(),
                deadline: Some(Instant::now() - Duration::from_secs(1)),
            },
        );
        let gateway =
            gateway_with_context(context, ApiFormat::Anthropic, &upstream.uri(), None).await;
        let response = reqwest::Client::new()
            .post(format!("http://{}/v1/messages", gateway.address()))
            .header("x-api-key", gateway.child_token().expose_secret())
            .json(&serde_json::json!({ "model": "any-model" }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
        assert!(response
            .text()
            .await
            .unwrap()
            .contains("duration limit has been reached"));
        assert!(upstream.received_requests().await.unwrap().is_empty());
        gateway.into_server().stop().await;
    }

    #[tokio::test]
    async fn anthropic_messages_replace_x_api_key_and_preserve_version_beta_and_sse() {
        let upstream = MockServer::start().await;
        let state = assert_fs::TempDir::new().unwrap();
        let usage_store = UsageStore::new(state.path());
        let response_body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3,\"cache_creation_input_tokens\":1,\"cache_read_input_tokens\":2,\"output_tokens\":0}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"response-marker\"}}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":4}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n"
        )
        .as_bytes()
        .to_vec();
        Mock::given(method("POST"))
            .and(path(MESSAGES_PATH))
            .and(header("x-api-key", UPSTREAM_KEY))
            .and(header("anthropic-version", "2023-06-01"))
            .and(header("anthropic-beta", "prompt-caching-2024-07-31"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(response_body.clone(), "text/event-stream"),
            )
            .expect(1)
            .mount(&upstream)
            .await;

        let gateway = gateway(
            ApiFormat::Anthropic,
            &upstream.uri(),
            Some(usage_store.clone()),
        )
        .await;
        let address = gateway.address();
        let local_key = gateway.child_token();
        let request_body = br#"{"model":"claude-test","messages":[{"role":"user","content":"prompt-marker"}],"stream":true}"#;
        let response = reqwest::Client::new()
            .post(format!("http://{address}{MESSAGES_PATH}"))
            .header("x-api-key", local_key.expose_secret())
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "prompt-caching-2024-07-31")
            .header("content-type", "application/json")
            .body(request_body.to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.bytes().await.unwrap().as_ref(), response_body);

        let received = upstream.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].body, request_body);
        assert_eq!(received[0].headers["x-api-key"], UPSTREAM_KEY);
        assert!(!received[0].headers.contains_key("authorization"));
        assert_eq!(received[0].headers["anthropic-version"], "2023-06-01");
        assert_eq!(
            received[0].headers["anthropic-beta"],
            "prompt-caching-2024-07-31"
        );

        let events = all_events(&usage_store);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].protocol, "anthropic_messages");
        assert_eq!(events[0].input_tokens_total, Some(6));
        assert_eq!(events[0].input_tokens_uncached, Some(3));
        assert_eq!(events[0].cache_read_input_tokens, Some(2));
        assert_eq!(events[0].cache_write_input_tokens, Some(1));
        assert_eq!(events[0].output_tokens, Some(4));
        assert_eq!(events[0].total_tokens, Some(10));
        assert_eq!(events[0].outcome, UsageOutcome::Succeeded, "{events:?}");
        assert_eq!(events[0].usage_completeness, UsageCompleteness::Complete);
    }

    #[tokio::test]
    async fn anthropic_messages_json_usage_is_observed_without_mutating_the_response() {
        let upstream = MockServer::start().await;
        let state = assert_fs::TempDir::new().unwrap();
        let usage_store = UsageStore::new(state.path());
        let response_body = br#"{"id":"message-secret","content":[{"type":"text","text":"response-marker"}],"usage":{"input_tokens":8,"cache_creation_input_tokens":2,"cache_read_input_tokens":4,"output_tokens":5}}"#.to_vec();
        Mock::given(method("POST"))
            .and(path(MESSAGES_PATH))
            .and(header("x-api-key", UPSTREAM_KEY))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(response_body.clone(), "application/json"),
            )
            .expect(1)
            .mount(&upstream)
            .await;

        let gateway = gateway(
            ApiFormat::Anthropic,
            &upstream.uri(),
            Some(usage_store.clone()),
        )
        .await;
        let response = reqwest::Client::new()
            .post(format!("http://{}{MESSAGES_PATH}", gateway.address()))
            .header("x-api-key", gateway.child_token().expose_secret())
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .body(r#"{"model":"claude-json","messages":[]}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.bytes().await.unwrap().as_ref(), response_body);

        let events = all_events(&usage_store);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].model.as_deref(), Some("claude-json"));
        assert_eq!(events[0].input_tokens_total, Some(14));
        assert_eq!(events[0].input_tokens_uncached, Some(8));
        assert_eq!(events[0].cache_read_input_tokens, Some(4));
        assert_eq!(events[0].cache_write_input_tokens, Some(2));
        assert_eq!(events[0].output_tokens, Some(5));
        assert_eq!(events[0].total_tokens, Some(19));
        assert_eq!(events[0].outcome, UsageOutcome::Succeeded);
    }

    #[tokio::test]
    async fn unsupported_routes_and_methods_are_rejected_before_upstream_forwarding() {
        let upstream = MockServer::start().await;
        let gateway = gateway(ApiFormat::OpenAi, &upstream.uri(), None).await;
        let address = gateway.address();
        let token = gateway.child_token();
        let client = reqwest::Client::new();

        for (method, path, expected) in [
            (Method::POST, MESSAGES_PATH, StatusCode::NOT_FOUND),
            (Method::POST, "/v1/files", StatusCode::NOT_FOUND),
            (Method::GET, RESPONSES_PATH, StatusCode::METHOD_NOT_ALLOWED),
        ] {
            let response = client
                .request(method, format!("http://{address}{path}"))
                .bearer_auth(token.expose_secret())
                .body("{}")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        assert!(upstream.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn missing_usage_does_not_change_a_successful_model_response() {
        let upstream = MockServer::start().await;
        let state = assert_fs::TempDir::new().unwrap();
        let usage_store = UsageStore::new(state.path());
        let response_body = br#"{"choices":[{"message":{"content":"response-marker"}}]}"#.to_vec();
        Mock::given(method("POST"))
            .and(path(CHAT_COMPLETIONS_PATH))
            .and(header(AUTHORIZATION, format!("Bearer {UPSTREAM_KEY}")))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(response_body.clone(), "application/json"),
            )
            .expect(1)
            .mount(&upstream)
            .await;
        let gateway = gateway(
            ApiFormat::OpenAi,
            &upstream.uri(),
            Some(usage_store.clone()),
        )
        .await;
        let response = reqwest::Client::new()
            .post(format!(
                "http://{}{CHAT_COMPLETIONS_PATH}",
                gateway.address()
            ))
            .bearer_auth(gateway.child_token().expose_secret())
            .header("content-type", "application/json")
            .body(r#"{"model":"no-usage-model","messages":[]}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.bytes().await.unwrap().as_ref(), response_body);

        let events = all_events(&usage_store);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].outcome, UsageOutcome::Succeeded);
        assert_eq!(events[0].usage_completeness, UsageCompleteness::Unavailable);
    }

    #[test]
    fn route_allowlist_is_limited_to_declared_api_formats_and_methods() {
        assert!(matches!(
            classify_route(&Method::GET, MODELS_PATH, ApiFormat::OpenAi),
            RouteDecision::Allowed(Route::Models)
        ));
        assert!(matches!(
            classify_route(&Method::POST, RESPONSES_PATH, ApiFormat::OpenAi),
            RouteDecision::Allowed(Route::Responses)
        ));
        assert!(matches!(
            classify_route(&Method::POST, MESSAGES_PATH, ApiFormat::Anthropic),
            RouteDecision::Allowed(Route::Messages)
        ));
        assert!(matches!(
            classify_route(&Method::POST, MESSAGES_PATH, ApiFormat::OpenAi),
            RouteDecision::NotFound
        ));
        assert!(matches!(
            classify_route(&Method::GET, RESPONSES_PATH, ApiFormat::Both),
            RouteDecision::MethodNotAllowed
        ));
        assert!(matches!(
            classify_route(&Method::CONNECT, "/anything", ApiFormat::Both),
            RouteDecision::NotFound
        ));
        assert!(matches!(
            classify_route(&Method::POST, "/v1/files", ApiFormat::Both),
            RouteDecision::NotFound
        ));
    }

    struct DelayedStream {
        first: &'static [u8],
        second: &'static [u8],
        release_second: Arc<Notify>,
    }

    async fn delayed_stream(State(state): State<Arc<DelayedStream>>) -> Response<Body> {
        let body = stream::unfold((state, 0), |(state, index)| async move {
            match index {
                0 => Some((
                    Ok::<_, std::io::Error>(Bytes::from_static(state.first)),
                    (state, 1),
                )),
                1 => {
                    state.release_second.notified().await;
                    Some((
                        Ok::<_, std::io::Error>(Bytes::from_static(state.second)),
                        (state, 2),
                    ))
                }
                _ => None,
            }
        });
        Response::builder()
            .header("content-type", "text/event-stream")
            .body(Body::from_stream(body))
            .unwrap()
    }

    async fn assert_stream_is_incremental(
        api_format: ApiFormat,
        path: &'static str,
        auth_header: &'static str,
        first: &'static [u8],
        second: &'static [u8],
    ) {
        let release_second = Arc::new(Notify::new());
        let state = Arc::new(DelayedStream {
            first,
            second,
            release_second: Arc::clone(&release_second),
        });
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let upstream_address = listener.local_addr().unwrap();
        let app = Router::new()
            .route(CHAT_COMPLETIONS_PATH, post(delayed_stream))
            .route(MESSAGES_PATH, post(delayed_stream))
            .with_state(state);
        let upstream_task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let upstream_url = format!("http://{upstream_address}");
        let gateway = gateway(api_format, &upstream_url, None).await;
        let local_key = gateway.child_token();
        let mut request = reqwest::Client::new()
            .post(format!("http://{}{path}", gateway.address()))
            .header("content-type", "application/json")
            .body(r#"{"model":"stream-model"}"#);
        request = if auth_header == "authorization" {
            request.bearer_auth(local_key.expose_secret())
        } else {
            request.header(auth_header, local_key.expose_secret())
        };
        let mut response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let first_chunk = tokio::time::timeout(Duration::from_secs(2), response.chunk()).await;
        release_second.notify_one();
        let first_chunk = first_chunk.unwrap().unwrap().unwrap();
        assert!(first_chunk.starts_with(first));
        let mut forwarded = first_chunk.to_vec();
        forwarded.extend_from_slice(&response.bytes().await.unwrap());
        assert_eq!(forwarded, [first, second].concat());
        upstream_task.abort();
    }

    #[tokio::test]
    async fn openai_and_anthropic_sse_streams_forward_incrementally() {
        assert_stream_is_incremental(
            ApiFormat::OpenAi,
            CHAT_COMPLETIONS_PATH,
            "authorization",
            b"data: {\"choices\":[{\"delta\":{\"content\":\"first\"}}]}\n\n",
            b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":1}}\n\ndata: [DONE]\n\n",
        )
        .await;
        assert_stream_is_incremental(
            ApiFormat::Anthropic,
            MESSAGES_PATH,
            "x-api-key",
            b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
            b"event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":1}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        )
        .await;
    }
}
