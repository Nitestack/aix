#[cfg(test)]
use crate::auth::AuthService;
use crate::commands::launch::LaunchEnv;
use crate::local_gateway::{self, ChatGptResponsesHandle, LaunchContext, ServerHandle};
#[cfg(test)]
use crate::secrets::SecretString;
#[cfg(test)]
use crate::usage_store::UsageEventFilter;
#[cfg(test)]
use crate::usage_store::UsageStore;
use color_eyre::Result;
#[cfg(test)]
use serde_json::Value;
#[cfg(test)]
use std::sync::Arc;
use std::time::Duration;
#[cfg(test)]
use url::Url;

const PROVIDER_ID: &str = "aix-chatgpt";
// Mirror the ChatGPT-eligible models in OpenCode's v2 OpenAI catalog. The
// canonical provider supplies their labels and capabilities; this distinct
// provider ID prevents saved OpenCode credentials from overriding the bridge.
pub(super) const CHATGPT_MODEL_IDS: &[&str] = &[
    "gpt-5.3-codex-spark",
    "gpt-5.5",
    "gpt-5.5-fast",
    "gpt-5.6",
    "gpt-5.6-fast",
    "gpt-5.6-luna",
    "gpt-5.6-luna-fast",
    "gpt-5.6-luna-pro",
    "gpt-5.6-pro",
    "gpt-5.6-sol",
    "gpt-5.6-sol-fast",
    "gpt-5.6-sol-pro",
    "gpt-5.6-terra",
    "gpt-5.6-terra-fast",
    "gpt-5.6-terra-pro",
    "gpt-6-astra",
    "gpt-6-astra-fast",
    "gpt-6-astra-pro",
    "gpt-6-astra-ultrafast",
    "gpt-6-luna",
    "gpt-6-luna-fast",
    "gpt-6-luna-pro",
    "gpt-6-sol",
    "gpt-6-sol-fast",
    "gpt-6-sol-pro",
    "gpt-6.1-sol",
    "gpt-6.1-sol-fast",
    "gpt-6.1-sol-pro",
];
pub(super) const OPENCODE_CONFIG_ENV: &str = "OPENCODE_CONFIG_CONTENT";
pub(super) const BRIDGE_TOKEN_ENV: &str = "AIX_OPENCODE_BRIDGE_TOKEN";
const RESPONSES_PROVIDER_PACKAGE: &str = "@opencode/ai/providers/openai/responses";

pub(super) fn print_dry_run() {
    eprintln!(
        "Would use ephemeral OpenCode SIWC bridge with OpenCode's native ChatGPT model picker"
    );
}

pub(super) fn runtime_config(port: u16) -> Result<String, serde_json::Error> {
    let base_url = format!("http://127.0.0.1:{port}/v1");
    let models = CHATGPT_MODEL_IDS
        .iter()
        .map(|model| ((*model).to_string(), serde_json::json!({})))
        .collect::<serde_json::Map<String, serde_json::Value>>();
    serde_json::to_string(&serde_json::json!({
        "providers": {
            (PROVIDER_ID): {
                "name": "ChatGPT plan via aix",
                "env": [BRIDGE_TOKEN_ENV],
                "package": RESPONSES_PROVIDER_PACKAGE,
                "canonical": "openai",
                "settings": {
                    "baseURL": base_url,
                    "transport": "http",
                },
                "models": models,
            },
        },
    }))
}

pub(super) struct BridgeHandle {
    gateway: ChatGptResponsesHandle,
    runtime_config: String,
}

impl BridgeHandle {
    pub(super) async fn start(context: LaunchContext, timeout: Duration) -> Result<Self> {
        let gateway = ChatGptResponsesHandle::start(
            context,
            timeout,
            local_gateway::AuthFailureResponse::new(
                "aix_opencode_bridge_error",
                "Invalid bridge credential",
            ),
        )
        .await?;
        Self::from_gateway(gateway)
    }

    #[cfg(test)]
    async fn start_for_test(
        context: LaunchContext,
        auth: Arc<AuthService>,
        upstream_base: Url,
    ) -> Result<Self> {
        let gateway = ChatGptResponsesHandle::start_for_test(
            context,
            auth,
            upstream_base,
            local_gateway::AuthFailureResponse::new(
                "aix_opencode_bridge_error",
                "Invalid bridge credential",
            ),
        )
        .await?;
        Self::from_gateway(gateway)
    }

    #[cfg(test)]
    async fn start_for_test_with_usage_store(
        context: LaunchContext,
        auth: Arc<AuthService>,
        upstream_base: Url,
        usage_store: UsageStore,
    ) -> Result<Self> {
        let gateway = ChatGptResponsesHandle::start_for_test_with_usage_store(
            context,
            auth,
            upstream_base,
            usage_store,
            local_gateway::AuthFailureResponse::new(
                "aix_opencode_bridge_error",
                "Invalid bridge credential",
            ),
        )
        .await?;
        Self::from_gateway(gateway)
    }

    fn from_gateway(gateway: ChatGptResponsesHandle) -> Result<Self> {
        let runtime_config = runtime_config(gateway.port())?;
        Ok(Self {
            gateway,
            runtime_config,
        })
    }

    pub(super) fn configure_env(&self, env: &mut LaunchEnv) {
        env.vars
            .push((OPENCODE_CONFIG_ENV.to_string(), self.runtime_config.clone()));
        env.auth_vars
            .push((BRIDGE_TOKEN_ENV.to_string(), self.gateway.child_token()));
    }

    pub(super) fn into_server(self) -> ServerHandle {
        self.gateway.into_server()
    }

    #[cfg(test)]
    pub(super) fn port(&self) -> u16 {
        self.gateway.port()
    }

    #[cfg(test)]
    fn address(&self) -> std::net::SocketAddr {
        self.gateway.address()
    }

    #[cfg(test)]
    pub(super) fn child_token(&self) -> SecretString {
        self.gateway.child_token()
    }

    #[cfg(test)]
    pub(super) async fn stop(self) {
        self.gateway.stop().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_gateway::{
        normalize_response_request, CompatibilityError, UNSUPPORTED_RESPONSE_FIELDS,
    };
    use crate::usage_event::UsageOutcome;
    use assert_fs::TempDir;
    use axum::body::{to_bytes, Body, Bytes};
    use axum::extract::State;
    use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
    use axum::http::{Request, Response, StatusCode};
    use axum::Router;
    use futures_util::stream;
    use serde_json::json;
    use std::convert::Infallible;
    use std::fs;
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
        release_stream_failure: Arc<Notify>,
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
        let model = body.get("model").and_then(Value::as_str);
        let is_failed_model = model == Some("failed-model");
        let is_stream_failure = model == Some("stream-failure-model");
        let is_no_usage_model = model == Some("no-usage-model");
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
        if is_failed_model {
            return Response::builder()
                .status(StatusCode::TOO_MANY_REQUESTS)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"error":"RESPONSE_MARKER_DO_NOT_PERSIST"}"#))
                .unwrap();
        }
        if is_no_usage_model {
            return Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from(
                    "event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n",
                ))
                .unwrap();
        }
        if is_stream_failure {
            let release = Arc::clone(&state.release_stream_failure);
            let body = stream::unfold((0u8, release), |(index, release)| async move {
                match index {
                    0 => Some((
                        Ok::<_, std::io::Error>(Bytes::from_static(
                            b"event: response.created\ndata: {}\n\n",
                        )),
                        (1, release),
                    )),
                    1 => {
                        release.notified().await;
                        Some((
                            Err(std::io::Error::other("mock stream failure")),
                            (2, release),
                        ))
                    }
                    _ => None,
                }
            });
            return Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(body))
                .unwrap();
        }
        let release = Arc::clone(&state.release_second_chunk);
        let body = stream::unfold(
            (0u8, request_index, release),
            |(chunk_index, request_index, release)| async move {
                match (chunk_index, request_index) {
                    (0, _) => Some((
                        Ok::<_, Infallible>(Bytes::from_static(
                            b"event: response.created\ndata: {}\n\nevent: response.future_type\ndata: {\"future\":true}\n\nevent: response.output_text.delta\ndata: {\"delta\":\"RESPONSE_MARKER_DO_NOT_PERSIST\"}\n\n",
                        )),
                        (1, request_index, release),
                    )),
                    (1, 0) => {
                        release.notified().await;
                        Some((
                            Ok::<_, Infallible>(Bytes::from_static(
                                b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":7,\"input_tokens_details\":{\"cached_tokens\":2},\"output_tokens\":3,\"total_tokens\":10}}}\n\n",
                            )),
                            (2, request_index, release),
                        ))
                    }
                    (1, _) => Some((
                        Ok::<_, Infallible>(Bytes::from_static(
                            b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":7,\"input_tokens_details\":{\"cached_tokens\":2},\"output_tokens\":3,\"total_tokens\":10}}}\n\n",
                        )),
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

    fn run_launch_context() -> LaunchContext {
        LaunchContext::new(
            "personal".to_string(),
            "opencode".to_string(),
            Some("run-25-test".to_string()),
            Some("bounded".to_string()),
        )
    }

    fn codex_run_launch_context() -> LaunchContext {
        let resolved = super::super::ResolvedRunLaunch {
            env: LaunchEnv {
                vars: Vec::new(),
                auth_vars: Vec::new(),
                display_only_vars: Vec::new(),
                clear_vars: Vec::new(),
                remove_vars: Vec::new(),
                profile_name: "personal".to_string(),
            },
            program: "codex".to_string(),
            logical_tool_name: Some("codex".to_string()),
            parent_gateway: None,
            allowed_models: Vec::new(),
            policy: Some(super::super::ResolvedRunPolicy {
                name: "bounded".to_string(),
                max_budget: Some(10.0),
                max_duration: "1h".to_string(),
                tags: Vec::new(),
            }),
            prepend_args: Vec::new(),
            codex_provider_overrides: Vec::new(),
            codex_config: None,
            sidecar_plan: None,
        };
        super::super::sidecar_context(
            &resolved,
            Some(uuid::Uuid::parse_str("4b9a85df-51d9-49a4-9a17-69d7f0dc91f1").unwrap()),
            None,
        )
    }

    fn restricted_run_launch_context(deadline: Option<std::time::Instant>) -> LaunchContext {
        LaunchContext::with_enforcement(
            "personal".to_string(),
            "opencode".to_string(),
            Some("run-policy-test".to_string()),
            Some("bounded".to_string()),
            local_gateway::RequestEnforcement {
                allowed_models: vec!["gpt-6-luna".to_string()],
                deadline,
            },
        )
    }

    fn persisted_state(state_dir: &std::path::Path) -> String {
        fn collect_files(directory: &std::path::Path, contents: &mut Vec<String>) {
            let Ok(entries) = fs::read_dir(directory) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_dir() {
                    collect_files(&path, contents);
                } else if file_type.is_file() {
                    if let Ok(content) = fs::read_to_string(path) {
                        contents.push(content);
                    }
                }
            }
        }

        let mut contents = Vec::new();
        collect_files(state_dir, &mut contents);
        contents.join("\n")
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
    fn runtime_config_offers_native_model_choices_without_selecting_or_overriding_openai() {
        let config: serde_json::Value =
            serde_json::from_str(&runtime_config(43127).unwrap()).unwrap();

        assert!(config.get("model").is_none());
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
            config["providers"]["aix-chatgpt"]["settings"]["transport"],
            "http"
        );
        assert_eq!(config["providers"]["aix-chatgpt"]["canonical"], "openai");
        assert_eq!(
            config["providers"]["aix-chatgpt"]["models"]["gpt-6-luna"],
            json!({})
        );
        assert_eq!(
            config["providers"]["aix-chatgpt"]["models"]
                .as_object()
                .unwrap()
                .len(),
            CHATGPT_MODEL_IDS.len()
        );
        assert!(config["providers"].get("openai").is_none());
        assert!(config.get("provider").is_none());
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
        let bridge =
            BridgeHandle::start_for_test(launch_context(), Arc::clone(&auth), upstream.clone())
                .await
                .unwrap();
        let second_bridge = BridgeHandle::start_for_test(launch_context(), auth, upstream)
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
        let usage_dir = TempDir::new().unwrap();
        let bridge = BridgeHandle::start_for_test_with_usage_store(
            run_launch_context(),
            Arc::clone(&auth),
            upstream,
            UsageStore::new(usage_dir.path()),
        )
        .await
        .unwrap();
        let token = bridge.child_token();
        let address = bridge.address();
        let client = reqwest::Client::new();
        let request = json!({
            "model": "gpt-test",
            "instructions": "existing instructions",
            "input": [
                { "type": "message", "role": "system", "content": "SYSTEM_PROMPT_MARKER_DO_NOT_PERSIST" },
                { "type": "message", "role": "user", "content": "PROMPT_MARKER_DO_NOT_PERSIST" },
                { "type": "function_call", "name": "inspect", "call_id": "call-1", "arguments": "TOOL_ARGUMENT_MARKER_DO_NOT_PERSIST" },
                { "type": "function_call_output", "call_id": "call-1", "output": "TOOL_RESULT_MARKER_DO_NOT_PERSIST" }
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
        assert_eq!(
            first_chunk,
            "event: response.created\ndata: {}\n\nevent: response.future_type\ndata: {\"future\":true}\n\nevent: response.output_text.delta\ndata: {\"delta\":\"RESPONSE_MARKER_DO_NOT_PERSIST\"}\n\n"
        );
        capture.release_second_chunk.notify_one();
        let remaining = response.bytes().await.unwrap();
        assert_eq!(
            format!(
                "{}{}",
                String::from_utf8_lossy(&first_chunk),
                String::from_utf8_lossy(&remaining)
            ),
            "event: response.created\ndata: {}\n\nevent: response.future_type\ndata: {\"future\":true}\n\nevent: response.output_text.delta\ndata: {\"delta\":\"RESPONSE_MARKER_DO_NOT_PERSIST\"}\n\nevent: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":7,\"input_tokens_details\":{\"cached_tokens\":2},\"output_tokens\":3,\"total_tokens\":10}}}\n\n"
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
            "existing instructions\n\nSYSTEM_PROMPT_MARKER_DO_NOT_PERSIST"
        );
        assert_eq!(requests[0].body["input"].as_array().unwrap().len(), 3);
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

        let events = UsageStore::new(usage_dir.path())
            .events(&UsageEventFilter {
                start_unix_ms: 0,
                end_unix_ms: u64::MAX,
                ..UsageEventFilter::default()
            })
            .unwrap();
        assert_eq!(events.len(), 2, "model discovery must not be recorded");
        for event in &events {
            assert_eq!(event.profile, "personal");
            assert_eq!(event.logical_tool_name, "opencode");
            assert_eq!(event.run_id.as_deref(), Some("run-25-test"));
            assert_eq!(event.run_policy.as_deref(), Some("bounded"));
            assert_eq!(event.protocol, "openai_responses");
            assert_eq!(event.model.as_deref(), Some("gpt-test"));
            assert_eq!(event.input_tokens_total, Some(7));
            assert_eq!(event.input_tokens_uncached, Some(5));
            assert_eq!(event.cache_read_input_tokens, Some(2));
            assert_eq!(event.output_tokens, Some(3));
            assert_eq!(event.total_tokens, Some(10));
            assert_eq!(event.request_count, 1);
            assert_eq!(
                event.usage_completeness,
                crate::usage_event::UsageCompleteness::Complete
            );
            assert_eq!(event.actual_cost_usd, None);
            assert_eq!(event.cost_source, None);
        }
        let persisted = persisted_state(usage_dir.path());
        for marker in [
            "SYSTEM_PROMPT_MARKER_DO_NOT_PERSIST",
            "PROMPT_MARKER_DO_NOT_PERSIST",
            "TOOL_ARGUMENT_MARKER_DO_NOT_PERSIST",
            "TOOL_RESULT_MARKER_DO_NOT_PERSIST",
            "RESPONSE_MARKER_DO_NOT_PERSIST",
            "initial-access-token",
            "refreshed-access-token",
            "saved-refresh-token",
        ] {
            assert!(
                !persisted.contains(marker),
                "persisted private marker {marker}"
            );
        }

        bridge.stop().await;
        upstream_task.abort();
    }

    #[tokio::test]
    async fn codex_responses_record_usage_with_run_attribution() {
        let directory = TempDir::new().unwrap();
        let auth = auth_service(
            &directory,
            Url::parse("https://auth.openai.com/api/accounts/oauth/token").unwrap(),
            "codex-access-token",
        );
        let (upstream, capture, upstream_task) = fake_api_server().await;
        let usage_dir = TempDir::new().unwrap();
        let bridge = BridgeHandle::start_for_test_with_usage_store(
            codex_run_launch_context(),
            auth,
            upstream,
            UsageStore::new(usage_dir.path()),
        )
        .await
        .unwrap();
        let token = bridge.child_token();
        let address = bridge.address();
        let client = reqwest::Client::new();

        // The fake upstream pauses before its usage-bearing terminal event.
        // Release it up front so this test only needs to await the full stream.
        capture.release_second_chunk.notify_one();
        let response = client
            .post(format!("http://{address}/v1/responses"))
            .bearer_auth(token.expose_secret())
            .json(&json!({ "model": "gpt-codex-test", "input": [] }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = response.bytes().await.unwrap();

        let events = UsageStore::new(usage_dir.path())
            .events(&UsageEventFilter {
                start_unix_ms: 0,
                end_unix_ms: u64::MAX,
                ..UsageEventFilter::default()
            })
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].profile, "personal");
        assert_eq!(events[0].logical_tool_name, "codex");
        assert_eq!(
            events[0].run_id.as_deref(),
            Some("4b9a85df-51d9-49a4-9a17-69d7f0dc91f1")
        );
        assert_eq!(events[0].run_policy.as_deref(), Some("bounded"));
        assert_eq!(events[0].protocol, "openai_responses");
        assert_eq!(events[0].model.as_deref(), Some("gpt-codex-test"));
        assert_eq!(events[0].input_tokens_total, Some(7));
        assert_eq!(events[0].output_tokens, Some(3));
        assert_eq!(events[0].total_tokens, Some(10));

        bridge.stop().await;
        upstream_task.abort();
    }

    #[tokio::test]
    async fn bridge_enforces_the_shared_model_allowlist_for_each_responses_request() {
        let directory = TempDir::new().unwrap();
        let auth = auth_service(
            &directory,
            Url::parse("https://auth.openai.com/api/accounts/oauth/token").unwrap(),
            "allowed-access-token",
        );
        let (upstream, capture, upstream_task) = fake_api_server().await;
        let bridge =
            BridgeHandle::start_for_test(restricted_run_launch_context(None), auth, upstream)
                .await
                .unwrap();
        let client = reqwest::Client::new();
        let endpoint = format!("http://{}/v1/responses", bridge.address());
        let token = bridge.child_token();

        let allowed = client
            .post(&endpoint)
            .bearer_auth(token.expose_secret())
            .json(&json!({ "model": "gpt-6-luna", "input": [] }))
            .send()
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);
        capture.release_second_chunk.notify_one();
        let _ = allowed.bytes().await.unwrap();

        let switched = client
            .post(&endpoint)
            .bearer_auth(token.expose_secret())
            .json(&json!({ "model": "gpt-6-sol", "input": [] }))
            .send()
            .await
            .unwrap();
        assert_eq!(switched.status(), StatusCode::FORBIDDEN);

        let missing = client
            .post(&endpoint)
            .bearer_auth(token.expose_secret())
            .json(&json!({ "input": [] }))
            .send()
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::BAD_REQUEST);

        assert_eq!(capture.requests.lock().await.len(), 1);
        bridge.stop().await;
        upstream_task.abort();
    }

    #[tokio::test]
    async fn bridge_rejects_inference_after_the_shared_deadline() {
        let directory = TempDir::new().unwrap();
        let auth = auth_service(
            &directory,
            Url::parse("https://auth.openai.com/api/accounts/oauth/token").unwrap(),
            "allowed-access-token",
        );
        let (upstream, capture, upstream_task) = fake_api_server().await;
        let bridge = BridgeHandle::start_for_test(
            restricted_run_launch_context(Some(
                std::time::Instant::now() - std::time::Duration::from_secs(1),
            )),
            auth,
            upstream,
        )
        .await
        .unwrap();
        let response = reqwest::Client::new()
            .post(format!("http://{}/v1/responses", bridge.address()))
            .bearer_auth(bridge.child_token().expose_secret())
            .json(&json!({ "model": "gpt-6-luna", "input": [] }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
        assert_eq!(capture.requests.lock().await.len(), 0);
        bridge.stop().await;
        upstream_task.abort();
    }

    #[tokio::test]
    async fn records_local_rejections_and_upstream_http_failures_without_content() {
        let directory = TempDir::new().unwrap();
        let auth = auth_service(
            &directory,
            Url::parse("https://auth.openai.com/api/accounts/oauth/token").unwrap(),
            "ACCESS_SECRET_MARKER_DO_NOT_PERSIST",
        );
        let (upstream, capture, upstream_task) = fake_api_server().await;
        let usage_dir = TempDir::new().unwrap();
        let bridge = BridgeHandle::start_for_test_with_usage_store(
            launch_context(),
            auth,
            upstream,
            UsageStore::new(usage_dir.path()),
        )
        .await
        .unwrap();
        let client = reqwest::Client::new();
        let token = bridge.child_token();
        let address = bridge.address();

        let rejected = client
            .post(format!("http://{address}/v1/responses"))
            .bearer_auth(token.expose_secret())
            .json(&json!({
                "model": "rejected-model",
                "input": [{"type":"message","role":"user","content":"PROMPT_MARKER_DO_NOT_PERSIST"}],
                "tools": [{"type":"file_search","vector_store_ids":["TOOL_ARGUMENT_MARKER_DO_NOT_PERSIST"]}]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        let _ = rejected.bytes().await.unwrap();

        let failed = client
            .post(format!("http://{address}/v1/responses"))
            .bearer_auth(token.expose_secret())
            .json(&json!({
                "model": "failed-model",
                "input": [{"type":"message","role":"user","content":"ANOTHER_PROMPT_MARKER_DO_NOT_PERSIST"}]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(failed.status(), StatusCode::TOO_MANY_REQUESTS);
        let _ = failed.bytes().await.unwrap();

        let no_usage = client
            .post(format!("http://{address}/v1/responses"))
            .bearer_auth(token.expose_secret())
            .json(&json!({
                "model": "no-usage-model",
                "input": []
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(no_usage.status(), StatusCode::OK);
        let _ = no_usage.bytes().await.unwrap();

        let stream_failure = client
            .post(format!("http://{address}/v1/responses"))
            .bearer_auth(token.expose_secret())
            .json(&json!({
                "model": "stream-failure-model",
                "input": []
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(stream_failure.status(), StatusCode::OK);
        capture.release_stream_failure.notify_one();
        assert!(stream_failure.bytes().await.is_err());

        let events = UsageStore::new(usage_dir.path())
            .events(&UsageEventFilter {
                start_unix_ms: 0,
                end_unix_ms: u64::MAX,
                ..UsageEventFilter::default()
            })
            .unwrap();
        assert_eq!(events.len(), 4);
        assert!(events
            .iter()
            .all(|event| event.run_id.is_none() && event.run_policy.is_none()));
        assert!(events.iter().any(|event| {
            event.model.as_deref() == Some("rejected-model")
                && event.outcome == UsageOutcome::Failed
                && event.http_status == Some(StatusCode::BAD_REQUEST.as_u16())
                && event.error_category.as_deref() == Some("local_compatibility")
        }));
        assert!(events.iter().any(|event| {
            event.model.as_deref() == Some("failed-model")
                && event.outcome == UsageOutcome::Failed
                && event.http_status == Some(StatusCode::TOO_MANY_REQUESTS.as_u16())
                && event.error_category.as_deref() == Some("upstream_http")
                && event.usage_completeness == crate::usage_event::UsageCompleteness::Unavailable
        }));
        assert!(events.iter().any(|event| {
            event.model.as_deref() == Some("stream-failure-model")
                && event.outcome == UsageOutcome::Failed
                && event.http_status == Some(StatusCode::OK.as_u16())
                && event.error_category.as_deref() == Some("upstream_stream")
        }));
        assert!(events.iter().any(|event| {
            event.model.as_deref() == Some("no-usage-model")
                && event.outcome == UsageOutcome::Succeeded
                && event.usage_completeness == crate::usage_event::UsageCompleteness::Unavailable
        }));
        let persisted = persisted_state(usage_dir.path());
        for marker in [
            "PROMPT_MARKER_DO_NOT_PERSIST",
            "ANOTHER_PROMPT_MARKER_DO_NOT_PERSIST",
            "TOOL_ARGUMENT_MARKER_DO_NOT_PERSIST",
            "RESPONSE_MARKER_DO_NOT_PERSIST",
            "ACCESS_SECRET_MARKER_DO_NOT_PERSIST",
        ] {
            assert!(
                !persisted.contains(marker),
                "persisted private marker {marker}"
            );
        }

        bridge.stop().await;
        upstream_task.abort();
    }

    #[tokio::test]
    async fn records_auth_refresh_failure_without_persisting_request_or_credentials() {
        let oauth = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&oauth)
            .await;
        let directory = TempDir::new().unwrap();
        let auth = auth_service(
            &directory,
            Url::parse(&format!("{}/token", oauth.uri())).unwrap(),
            "ACCESS_SECRET_MARKER_DO_NOT_PERSIST",
        );
        auth.expire_access_token_for_test("personal").unwrap();
        let usage_dir = TempDir::new().unwrap();
        let bridge = BridgeHandle::start_for_test_with_usage_store(
            launch_context(),
            auth,
            Url::parse("http://127.0.0.1:9/v1/").unwrap(),
            UsageStore::new(usage_dir.path()),
        )
        .await
        .unwrap();

        let response = reqwest::Client::new()
            .post(format!("http://{}/v1/responses", bridge.address()))
            .bearer_auth(bridge.child_token().expose_secret())
            .json(&json!({
                "model": "auth-failure-model",
                "input": [{"type":"message","role":"user","content":"PROMPT_SECRET_MARKER_DO_NOT_PERSIST"}]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let _ = response.bytes().await.unwrap();

        let events = UsageStore::new(usage_dir.path())
            .events(&UsageEventFilter {
                start_unix_ms: 0,
                end_unix_ms: u64::MAX,
                ..UsageEventFilter::default()
            })
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].profile, "personal");
        assert_eq!(events[0].logical_tool_name, "opencode");
        assert_eq!(events[0].run_id, None);
        assert_eq!(events[0].model.as_deref(), Some("auth-failure-model"));
        assert_eq!(
            events[0].http_status,
            Some(StatusCode::BAD_GATEWAY.as_u16())
        );
        assert_eq!(events[0].error_category.as_deref(), Some("authentication"));
        let persisted = persisted_state(usage_dir.path());
        for marker in [
            "PROMPT_SECRET_MARKER_DO_NOT_PERSIST",
            "ACCESS_SECRET_MARKER_DO_NOT_PERSIST",
            "saved-refresh-token",
        ] {
            assert!(
                !persisted.contains(marker),
                "persisted private marker {marker}"
            );
        }
        assert_eq!(oauth.received_requests().await.unwrap().len(), 1);

        bridge.stop().await;
    }

    #[tokio::test]
    async fn usage_storage_failure_does_not_break_a_successful_inference_request() {
        let auth_directory = TempDir::new().unwrap();
        let auth = auth_service(
            &auth_directory,
            Url::parse("https://auth.openai.com/api/accounts/oauth/token").unwrap(),
            "valid-access-token",
        );
        let (upstream, capture, upstream_task) = fake_api_server().await;
        let state_file = auth_directory.path().join("not-a-directory");
        fs::write(&state_file, "block usage store directory creation").unwrap();
        let bridge = BridgeHandle::start_for_test_with_usage_store(
            launch_context(),
            auth,
            upstream,
            UsageStore::new(&state_file),
        )
        .await
        .unwrap();

        let response = reqwest::Client::new()
            .post(format!("http://{}/v1/responses", bridge.address()))
            .bearer_auth(bridge.child_token().expose_secret())
            .json(&json!({"model":"gpt-test","input":[]}))
            .send()
            .await
            .unwrap();
        capture.release_second_chunk.notify_one();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.text().await.unwrap();
        assert!(body.contains("response.completed"));
        assert!(!body.contains("PROMPT"));

        bridge.stop().await;
        upstream_task.abort();
    }
}
