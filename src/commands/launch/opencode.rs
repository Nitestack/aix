use crate::auth::AuthService;
use crate::commands::launch::LaunchEnv;
use crate::local_gateway::{self, LaunchContext, ServerHandle};
#[cfg(test)]
use crate::secrets::SecretString;
use axum::body::Body;
use axum::extract::{Extension, State};
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::{Method, Request, Response, StatusCode};
use axum::Router;
use color_eyre::Result;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

// OpenAI's current OSS SIWC preview contract: https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations
const UNSUPPORTED_RESPONSE_FIELDS: &[&str] = &[
    "background",
    "conversation",
    "max_output_tokens",
    "max_tool_calls",
    "metadata",
    "moderation",
    "multi_agent",
    "prompt",
    "prompt_cache_retention",
    "safety_identifier",
    "temperature",
    "top_logprobs",
    "top_p",
    "truncation",
    "user",
    "previous_response_id",
];

const UNSUPPORTED_HOSTED_TOOLS: &[&str] = &[
    "code_interpreter",
    "file_search",
    "computer",
    "computer_use_preview",
    "image_generation",
    "mcp",
    "tool_search",
    "programmatic_tool_calling",
];

const UNSUPPORTED_INPUT_ITEM_TYPES: &[&str] = &[
    "code_interpreter_call",
    "computer_call",
    "computer_call_output",
    "file_search_call",
    "file_search_call_output",
    "image_generation_call",
    "mcp_call",
    "tool_search_call",
];

const PROVIDER_ID: &str = "aix-chatgpt";
const PROVIDER_MODEL_ID: &str = "aix-selected";
pub(super) const OPENCODE_CONFIG_ENV: &str = "OPENCODE_CONFIG_CONTENT";
pub(super) const BRIDGE_TOKEN_ENV: &str = "AIX_OPENCODE_BRIDGE_TOKEN";
const RESPONSES_PROVIDER_PACKAGE: &str = "@opencode/ai/providers/openai/responses";
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

pub(super) struct OpenCodeSiwcPlan {
    pub model: String,
}

pub(super) fn print_dry_run(plan: &OpenCodeSiwcPlan) {
    eprintln!(
        "Would use ephemeral OpenCode SIWC bridge with provider aix-chatgpt (model {})",
        plan.model
    );
}

pub(super) fn runtime_config(port: u16, model: &str) -> Result<String, serde_json::Error> {
    let base_url = format!("http://127.0.0.1:{port}/v1");
    let model_reference = format!("{PROVIDER_ID}/{PROVIDER_MODEL_ID}");
    serde_json::to_string(&serde_json::json!({
        "model": model_reference,
        "providers": {
            (PROVIDER_ID): {
                "name": "ChatGPT plan via aix",
                "env": [BRIDGE_TOKEN_ENV],
                "package": RESPONSES_PROVIDER_PACKAGE,
                "settings": {
                    "baseURL": base_url,
                },
                "models": {
                    (PROVIDER_MODEL_ID): {
                        "modelID": model,
                        "name": model,
                        "tool_call": true,
                        "modalities": {
                            "input": ["text"],
                            "output": ["text"],
                        },
                    },
                },
            },
        },
    }))
}

struct BridgeState {
    auth: Arc<AuthService>,
    upstream_base: Url,
    client: reqwest::Client,
}

pub(super) struct BridgeHandle {
    server: ServerHandle,
    runtime_config: String,
}

impl BridgeHandle {
    pub(super) async fn start(
        plan: OpenCodeSiwcPlan,
        context: LaunchContext,
        timeout: Duration,
    ) -> Result<Self> {
        let auth = Arc::new(AuthService::new(timeout)?);
        let upstream_base = Url::parse(OPENAI_PUBLIC_API_BASE)?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .build()?;
        Self::bind(plan, context, auth, upstream_base, client).await
    }

    #[cfg(test)]
    async fn start_for_test(
        context: LaunchContext,
        auth: Arc<AuthService>,
        upstream_base: Url,
        model: &str,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .build()?;
        Self::bind(
            OpenCodeSiwcPlan {
                model: model.to_string(),
            },
            context,
            auth,
            upstream_base,
            client,
        )
        .await
    }

    async fn bind(
        plan: OpenCodeSiwcPlan,
        context: LaunchContext,
        auth: Arc<AuthService>,
        upstream_base: Url,
        client: reqwest::Client,
    ) -> Result<Self> {
        let state = Arc::new(BridgeState {
            auth,
            upstream_base,
            client,
        });
        let app = Router::new().fallback(handle_request).with_state(state);
        let server = ServerHandle::start(
            app,
            context,
            local_gateway::AuthFailureResponse::new(
                "aix_opencode_bridge_error",
                "Invalid bridge credential",
            ),
        )
        .await?;
        let runtime_config = runtime_config(server.port(), &plan.model)?;
        Ok(Self {
            server,
            runtime_config,
        })
    }

    pub(super) fn configure_env(&self, env: &mut LaunchEnv) {
        env.vars
            .push((OPENCODE_CONFIG_ENV.to_string(), self.runtime_config.clone()));
        env.auth_vars
            .push((BRIDGE_TOKEN_ENV.to_string(), self.server.child_token()));
    }

    pub(super) fn into_server(self) -> ServerHandle {
        self.server
    }

    #[cfg(test)]
    pub(super) fn port(&self) -> u16 {
        self.server.port()
    }

    #[cfg(test)]
    fn address(&self) -> std::net::SocketAddr {
        self.server.address()
    }

    #[cfg(test)]
    pub(super) fn child_token(&self) -> SecretString {
        self.server.child_token()
    }

    #[cfg(test)]
    pub(super) async fn stop(self) {
        self.server.stop().await;
    }
}

async fn handle_request(
    State(state): State<Arc<BridgeState>>,
    Extension(context): Extension<LaunchContext>,
    request: Request<Body>,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    if parts.uri.query().is_some() {
        return error_response(StatusCode::NOT_FOUND, "Route not found");
    }

    let (endpoint, method, request_body) = match (&parts.method, parts.uri.path()) {
        (&Method::POST, "/v1/responses") => {
            let is_json = parts
                .headers
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("application/json"));
            if !is_json {
                return error_response(StatusCode::UNSUPPORTED_MEDIA_TYPE, "Expected JSON request");
            }
            let body = match local_gateway::read_bounded_body(body).await {
                Ok(body) => body,
                Err(_) => {
                    return error_response(StatusCode::PAYLOAD_TOO_LARGE, "Request body too large")
                }
            };
            let mut request: Value = match serde_json::from_slice(&body) {
                Ok(request) => request,
                Err(_) => return error_response(StatusCode::BAD_REQUEST, "Invalid JSON request"),
            };
            let normalized = match normalize_response_request(&mut request) {
                Ok(request) => request,
                Err(error) => {
                    return error_response(StatusCode::BAD_REQUEST, error.to_string().as_str())
                }
            };
            let body = match serde_json::to_vec(&normalized) {
                Ok(body) => body,
                Err(_) => return error_response(StatusCode::BAD_REQUEST, "Invalid JSON request"),
            };
            ("responses", Method::POST, Some(body))
        }
        (&Method::GET, "/v1/models") => ("models", Method::GET, None),
        (_, "/v1/responses" | "/v1/models") => {
            return error_response(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed")
        }
        _ => return error_response(StatusCode::NOT_FOUND, "Route not found"),
    };

    proxy_upstream(state, &context.profile, endpoint, method, request_body).await
}

async fn proxy_upstream(
    state: Arc<BridgeState>,
    profile_name: &str,
    endpoint: &str,
    method: Method,
    request_body: Option<Vec<u8>>,
) -> Response<Body> {
    let token = match state.auth.access_token(profile_name).await {
        Ok(token) => token,
        Err(_) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "Could not obtain the selected ChatGPT profile's access token",
            )
        }
    };
    let Ok(url) = state.upstream_base.join(endpoint) else {
        return error_response(StatusCode::BAD_GATEWAY, "Upstream request failed");
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
        Err(_) => return error_response(StatusCode::BAD_GATEWAY, "Upstream request failed"),
    };
    match local_gateway::forward_response(upstream, FORWARDED_RESPONSE_HEADERS) {
        Ok(response) => response,
        Err(_) => error_response(StatusCode::BAD_GATEWAY, "Upstream response failed"),
    }
}

fn error_response(status: StatusCode, message: &str) -> Response<Body> {
    local_gateway::error_response(status, "aix_opencode_bridge_error", message)
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
enum CompatibilityError {
    #[error("Responses request must be a JSON object")]
    InvalidRequest,
    #[error("system input must contain only text content")]
    InvalidSystemContent,
    #[error("Responses tools must use supported function/custom or web search forms")]
    UnsupportedTool,
}

fn normalize_response_request(request: &mut Value) -> Result<Value, CompatibilityError> {
    let object = request
        .as_object_mut()
        .ok_or(CompatibilityError::InvalidRequest)?;

    for field in UNSUPPORTED_RESPONSE_FIELDS {
        object.remove(*field);
    }

    if let Some(tools) = object.get("tools") {
        validate_tool_list(tools)?;
    }
    if let Some(tools) = object.get("additional_tools") {
        validate_tool_list(tools)?;
    }

    let mut system_texts = Vec::new();
    if let Some(input) = object.get_mut("input") {
        if let Some(items) = input.as_array_mut() {
            let mut retained = Vec::with_capacity(items.len());
            for item in items.drain(..) {
                if item
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| UNSUPPORTED_INPUT_ITEM_TYPES.contains(&kind))
                {
                    return Err(CompatibilityError::UnsupportedTool);
                }
                if item.get("role").and_then(Value::as_str) == Some("system") {
                    let content = item
                        .get("content")
                        .ok_or(CompatibilityError::InvalidSystemContent)?;
                    system_texts.push(extract_system_text(content)?);
                } else {
                    retained.push(item);
                }
            }
            *items = retained;
        }
    }

    if !system_texts.is_empty() {
        let system_context = system_texts.join("\n\n");
        let existing = object
            .remove("instructions")
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or(CompatibilityError::InvalidSystemContent)
            })
            .transpose()?;
        object.insert(
            "instructions".to_string(),
            Value::String(match existing.filter(|text| !text.is_empty()) {
                Some(existing) => format!("{existing}\n\n{system_context}"),
                None => system_context,
            }),
        );
    }

    object.insert("store".to_string(), Value::Bool(false));
    object.insert("stream".to_string(), Value::Bool(true));
    Ok(request.clone())
}

fn validate_tool_list(tools: &Value) -> Result<(), CompatibilityError> {
    let tools = tools
        .as_array()
        .ok_or(CompatibilityError::UnsupportedTool)?;
    for tool in tools {
        let kind = tool
            .get("type")
            .and_then(Value::as_str)
            .ok_or(CompatibilityError::UnsupportedTool)?;
        if UNSUPPORTED_HOSTED_TOOLS.contains(&kind)
            || !matches!(
                kind,
                "function" | "custom" | "web_search" | "web_search_preview"
            )
        {
            return Err(CompatibilityError::UnsupportedTool);
        }
    }
    Ok(())
}

fn extract_system_text(content: &Value) -> Result<String, CompatibilityError> {
    if let Some(text) = content.as_str() {
        return Ok(text.to_owned());
    }
    let parts = content
        .as_array()
        .ok_or(CompatibilityError::InvalidSystemContent)?;
    let text = parts
        .iter()
        .map(|part| {
            let kind = part.get("type").and_then(Value::as_str);
            if !matches!(kind, None | Some("text") | Some("input_text")) {
                return Err(CompatibilityError::InvalidSystemContent);
            }
            part.get("text")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or(CompatibilityError::InvalidSystemContent)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(text.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use axum::body::{to_bytes, Bytes};
    use axum::extract::State;
    use axum::http::header::AUTHORIZATION;
    use axum::http::Request;
    use futures_util::stream;
    use serde_json::json;
    use std::convert::Infallible;
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio::sync::{Mutex, Notify};
    use tokio::task::JoinHandle;
    use tokio::time::timeout;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[derive(Clone, Debug)]
    struct CapturedRequest {
        path: String,
        authorization: Option<String>,
        body: Value,
    }

    #[derive(Clone, Default)]
    struct FakeApiState {
        requests: Arc<Mutex<Vec<CapturedRequest>>>,
        release_second_chunk: Arc<Notify>,
    }

    async fn fake_api_handler(
        State(state): State<FakeApiState>,
        request: Request<Body>,
    ) -> Response<Body> {
        let path = request.uri().path().to_owned();
        let authorization = request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|header| header.to_str().ok())
            .map(str::to_owned);
        let body_bytes = to_bytes(request.into_body(), local_gateway::MAX_REQUEST_BODY_BYTES)
            .await
            .unwrap();
        let body = if body_bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body_bytes).unwrap()
        };
        let request_index = {
            let mut requests = state.requests.lock().await;
            let index = requests.len();
            requests.push(CapturedRequest {
                path,
                authorization,
                body,
            });
            index
        };
        let release = Arc::clone(&state.release_second_chunk);
        let body = stream::unfold(
            (0u8, request_index, release),
            |(chunk_index, request_index, release)| async move {
                match (chunk_index, request_index) {
                    (0, _) => Some((
                        Ok::<_, Infallible>(Bytes::from_static(b"data: response.created\n\n")),
                        (1, request_index, release),
                    )),
                    (1, 0) => {
                        release.notified().await;
                        Some((
                            Ok::<_, Infallible>(Bytes::from_static(
                                b"data: response.completed\n\n",
                            )),
                            (2, request_index, release),
                        ))
                    }
                    (1, _) => Some((
                        Ok::<_, Infallible>(Bytes::from_static(b"data: response.completed\n\n")),
                        (2, request_index, release),
                    )),
                    _ => None,
                }
            },
        );

        Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/event-stream")
            .header("x-request-id", "req-aix-test-23")
            .body(Body::from_stream(body))
            .unwrap()
    }

    async fn fake_api_server() -> (Url, FakeApiState, JoinHandle<()>) {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let state = FakeApiState::default();
        let app = Router::new()
            .fallback(fake_api_handler)
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (
            Url::parse(&format!("http://{address}/v1/")).unwrap(),
            state,
            task,
        )
    }

    fn auth_service(
        directory: &TempDir,
        token_endpoint: Url,
        access_token: &str,
    ) -> Arc<AuthService> {
        let expiry = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        Arc::new(
            AuthService::with_test_profile_token(
                directory.path().to_path_buf(),
                "personal",
                access_token,
                "saved-refresh-token",
                expiry,
                token_endpoint,
            )
            .unwrap(),
        )
    }

    fn launch_context() -> LaunchContext {
        LaunchContext::new("personal".to_string(), "opencode".to_string(), None, None)
    }

    #[test]
    fn normalization_enforces_siwc_and_preserves_history_and_system_context() {
        let mut request = json!({
            "model": "gpt-test",
            "instructions": "existing instructions",
            "input": [
                {
                    "type": "message",
                    "role": "system",
                    "content": [
                        { "type": "input_text", "text": "system one" },
                        { "type": "input_text", "text": "system two" }
                    ]
                },
                { "type": "message", "role": "user", "content": "hello" },
                { "type": "function_call", "name": "inspect", "call_id": "call-1" },
                { "type": "function_call_output", "call_id": "call-1", "output": "done" },
                { "type": "message", "role": "system", "content": "system three" }
            ],
            "tools": [
                { "type": "function", "name": "inspect", "parameters": {} },
                { "type": "custom", "name": "shell", "format": { "type": "text" } }
            ],
            "store": true,
            "stream": false,
            "previous_response_id": "resp-old",
            "max_output_tokens": 1024,
            "temperature": 0.4,
            "metadata": { "trace": "discard" }
        });

        let normalized = normalize_response_request(&mut request).unwrap();

        assert_eq!(normalized["store"], false);
        assert_eq!(normalized["stream"], true);
        assert_eq!(
            normalized["instructions"],
            "existing instructions\n\nsystem one\nsystem two\n\nsystem three"
        );
        assert_eq!(
            normalized["input"],
            json!([
                { "type": "message", "role": "user", "content": "hello" },
                { "type": "function_call", "name": "inspect", "call_id": "call-1" },
                { "type": "function_call_output", "call_id": "call-1", "output": "done" }
            ])
        );
        assert_eq!(normalized["tools"], request["tools"]);
        for unsupported in [
            "previous_response_id",
            "max_output_tokens",
            "temperature",
            "metadata",
        ] {
            assert!(normalized.get(unsupported).is_none(), "kept {unsupported}");
        }
    }

    #[test]
    fn normalization_omits_every_field_rejected_by_the_current_preview() {
        let mut request = json!({ "model": "gpt-test", "input": [] });
        for field in UNSUPPORTED_RESPONSE_FIELDS {
            request[field] = json!("unsupported");
        }

        let normalized = normalize_response_request(&mut request).unwrap();

        for field in UNSUPPORTED_RESPONSE_FIELDS {
            assert!(normalized.get(*field).is_none(), "kept {field}");
        }
    }

    #[test]
    fn normalization_rejects_unsupported_hosted_tools_without_downgrading_them() {
        let mut request = json!({
            "model": "gpt-test",
            "input": [],
            "tools": [{ "type": "file_search", "vector_store_ids": ["vs-secret"] }]
        });

        assert_eq!(
            normalize_response_request(&mut request),
            Err(CompatibilityError::UnsupportedTool)
        );
        assert_eq!(request["tools"][0]["type"], "file_search");
    }

    #[test]
    fn normalization_rejects_non_text_system_context_instead_of_dropping_it() {
        let mut request = json!({
            "input": [{
                "type": "message",
                "role": "system",
                "content": [{ "type": "input_image", "image_url": "data:..." }]
            }]
        });

        assert_eq!(
            normalize_response_request(&mut request),
            Err(CompatibilityError::InvalidSystemContent)
        );
    }

    #[test]
    fn runtime_config_selects_the_v2_responses_provider_and_profile_model() {
        let config: serde_json::Value =
            serde_json::from_str(&runtime_config(43127, "gpt-test").unwrap()).unwrap();

        assert_eq!(config["model"], "aix-chatgpt/aix-selected");
        assert_eq!(
            config["providers"]["aix-chatgpt"]["package"],
            "@opencode/ai/providers/openai/responses"
        );
        assert_eq!(
            config["providers"]["aix-chatgpt"]["env"],
            json!(["AIX_OPENCODE_BRIDGE_TOKEN"])
        );
        assert_eq!(
            config["providers"]["aix-chatgpt"]["settings"]["baseURL"],
            "http://127.0.0.1:43127/v1"
        );
        assert_eq!(
            config["providers"]["aix-chatgpt"]["models"]["aix-selected"]["modelID"],
            "gpt-test"
        );
        assert!(config.get("provider").is_none());
        assert!(config["providers"]["aix-chatgpt"]["package"] != "openai");
    }

    #[tokio::test]
    async fn bridge_is_random_loopback_only_authenticated_and_stops_cleanly() {
        let directory = TempDir::new().unwrap();
        let auth = auth_service(
            &directory,
            Url::parse("https://auth.openai.com/api/accounts/oauth/token").unwrap(),
            "real-access-token",
        );
        let upstream = Url::parse("http://127.0.0.1:9/v1/").unwrap();
        let bridge = BridgeHandle::start_for_test(
            launch_context(),
            Arc::clone(&auth),
            upstream.clone(),
            "gpt-test",
        )
        .await
        .unwrap();
        let second_bridge =
            BridgeHandle::start_for_test(launch_context(), auth, upstream, "gpt-test")
                .await
                .unwrap();

        assert_eq!(bridge.address().ip(), Ipv4Addr::LOCALHOST);
        assert_ne!(bridge.port(), 0);
        assert_ne!(
            bridge.child_token().expose_secret(),
            second_bridge.child_token().expose_secret()
        );
        assert_ne!(bridge.child_token().expose_secret(), "real-access-token");
        assert_eq!(format!("{:?}", bridge.child_token()), "[secret]");

        let mut child_env = LaunchEnv {
            vars: Vec::new(),
            auth_vars: Vec::new(),
            display_only_vars: Vec::new(),
            clear_vars: Vec::new(),
            remove_vars: Vec::new(),
            profile_name: "personal".to_string(),
        };
        bridge.configure_env(&mut child_env);
        assert_eq!(
            child_env
                .vars
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            [OPENCODE_CONFIG_ENV]
        );
        assert_eq!(
            child_env
                .auth_vars
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            [BRIDGE_TOKEN_ENV]
        );
        assert_eq!(
            child_env.auth_vars[0].1.expose_secret(),
            bridge.child_token().expose_secret()
        );
        assert!(!child_env.auth_vars[0]
            .1
            .expose_secret()
            .contains("real-access-token"));

        let address = bridge.address();
        let token = bridge.child_token();
        let client = reqwest::Client::new();
        let url = format!("http://{address}/v1/responses");
        let unauthenticated = client
            .post(&url)
            .header(CONTENT_TYPE, "application/json")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            unauthenticated.json::<Value>().await.unwrap(),
            json!({
                "error": {
                    "type": "aix_opencode_bridge_error",
                    "message": "Invalid bridge credential"
                }
            })
        );

        let invalid = client
            .get(format!("http://{address}/v1/models"))
            .bearer_auth("not-the-bridge-secret")
            .send()
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);

        let unknown_route = client
            .get(format!("http://{address}/v1/arbitrary"))
            .bearer_auth(token.expose_secret())
            .send()
            .await
            .unwrap();
        assert_eq!(unknown_route.status(), StatusCode::NOT_FOUND);

        let invalid_method = client
            .get(format!("http://{address}/v1/responses"))
            .bearer_auth(token.expose_secret())
            .send()
            .await
            .unwrap();
        assert_eq!(invalid_method.status(), StatusCode::METHOD_NOT_ALLOWED);

        let with_query = client
            .get(format!("http://{address}/v1/models?host=attacker.example"))
            .bearer_auth(token.expose_secret())
            .send()
            .await
            .unwrap();
        assert_eq!(with_query.status(), StatusCode::NOT_FOUND);

        bridge.stop().await;
        assert!(client
            .get(format!("http://{address}/v1/models"))
            .send()
            .await
            .is_err());
        second_bridge.stop().await;
    }

    #[tokio::test]
    async fn bridge_normalizes_responses_streams_sse_and_refreshes_auth_without_restart() {
        let oauth = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "refreshed-access-token",
                "refresh_token": "rotated-refresh-token",
                "expires_in": 3600,
                "scope": "chatgpt.tokens.use.direct"
            })))
            .mount(&oauth)
            .await;
        let directory = TempDir::new().unwrap();
        let auth = auth_service(
            &directory,
            Url::parse(&format!("{}/token", oauth.uri())).unwrap(),
            "initial-access-token",
        );
        let (upstream, capture, upstream_task) = fake_api_server().await;
        let bridge =
            BridgeHandle::start_for_test(launch_context(), Arc::clone(&auth), upstream, "gpt-test")
                .await
                .unwrap();
        let token = bridge.child_token();
        let address = bridge.address();
        let client = reqwest::Client::new();
        let request = json!({
            "model": "gpt-test",
            "instructions": "existing instructions",
            "input": [
                { "type": "message", "role": "system", "content": "system context" },
                { "type": "message", "role": "user", "content": "hello" },
                { "type": "function_call", "name": "inspect", "call_id": "call-1" }
            ],
            "store": true,
            "stream": false,
            "temperature": 0.7,
            "previous_response_id": "resp-previous"
        });

        let mut response = timeout(
            Duration::from_secs(2),
            client
                .post(format!("http://{address}/v1/responses"))
                .bearer_auth(token.expose_secret())
                .json(&request)
                .send(),
        )
        .await
        .expect("bridge should return response headers before the full SSE stream")
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("x-request-id")
                .and_then(|value| value.to_str().ok()),
            Some("req-aix-test-23")
        );
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
        let first_chunk = timeout(Duration::from_secs(2), response.chunk())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(first_chunk, "data: response.created\n\n");
        capture.release_second_chunk.notify_one();
        let remaining = response.bytes().await.unwrap();
        assert_eq!(
            format!(
                "{}{}",
                String::from_utf8_lossy(&first_chunk),
                String::from_utf8_lossy(&remaining)
            ),
            "data: response.created\n\ndata: response.completed\n\n"
        );

        auth.expire_access_token_for_test("personal").unwrap();
        let refreshed_response = client
            .post(format!("http://{address}/v1/responses"))
            .bearer_auth(token.expose_secret())
            .json(&json!({ "model": "gpt-test", "input": [] }))
            .send()
            .await
            .unwrap();
        assert!(refreshed_response.status().is_success());
        let _ = refreshed_response.bytes().await.unwrap();

        let models_response = client
            .get(format!("http://{address}/v1/models"))
            .bearer_auth(token.expose_secret())
            .send()
            .await
            .unwrap();
        assert!(models_response.status().is_success());
        let _ = models_response.bytes().await.unwrap();

        let requests = capture.requests.lock().await.clone();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].path, "/v1/responses");
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer initial-access-token")
        );
        assert_eq!(requests[0].body["store"], false);
        assert_eq!(requests[0].body["stream"], true);
        assert_eq!(
            requests[0].body["instructions"],
            "existing instructions\n\nsystem context"
        );
        assert_eq!(requests[0].body["input"].as_array().unwrap().len(), 2);
        assert!(requests[0].body.get("temperature").is_none());
        assert!(requests[0].body.get("previous_response_id").is_none());
        assert_ne!(
            requests[0].authorization.as_deref(),
            Some(token.expose_secret())
        );
        assert_eq!(
            requests[1].authorization.as_deref(),
            Some("Bearer refreshed-access-token")
        );
        assert_eq!(requests[2].path, "/v1/models");
        assert_eq!(
            requests[2].authorization.as_deref(),
            Some("Bearer refreshed-access-token")
        );
        assert_eq!(oauth.received_requests().await.unwrap().len(), 1);

        bridge.stop().await;
        upstream_task.abort();
    }
}
