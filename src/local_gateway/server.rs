use super::{error_response, AuthFailureResponse, LaunchContext};
use crate::secrets::SecretString;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, Response, StatusCode};
use axum::middleware::{self, Next};
use axum::Router;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const MAX_REQUEST_HEADER_COUNT: usize = 100;
const MAX_REQUEST_HEADER_BYTES: usize = 32 * 1024;
const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(1);

struct GatewayState {
    child_token: SecretString,
    context: LaunchContext,
    auth_failure: AuthFailureResponse,
}

pub(crate) struct ServerHandle {
    address: SocketAddr,
    child_token: SecretString,
    shutdown: Option<oneshot::Sender<()>>,
    server: Option<JoinHandle<std::io::Result<()>>>,
}

impl ServerHandle {
    pub(crate) async fn start(
        app: Router,
        context: LaunchContext,
        auth_failure: AuthFailureResponse,
    ) -> Result<Self, std::io::Error> {
        let child_token = SecretString::new(format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ));
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
        let address = listener.local_addr()?;
        let state = Arc::new(GatewayState {
            child_token: SecretString::new(child_token.expose_secret().to_owned()),
            context,
            auth_failure,
        });
        let app = app.layer(middleware::from_fn_with_state(state, authorize_request));
        let (shutdown, shutdown_receiver) = oneshot::channel();
        let (ready, ready_receiver) = oneshot::channel();
        let server = tokio::spawn(async move {
            let serving = axum::serve(listener, app).with_graceful_shutdown(async {
                let _ = shutdown_receiver.await;
            });
            let _ = ready.send(());
            serving.await
        });
        if ready_receiver.await.is_err() {
            return Err(startup_error(server.await));
        }
        tokio::task::yield_now().await;
        if server.is_finished() {
            return Err(startup_error(server.await));
        }

        Ok(Self {
            address,
            child_token,
            shutdown: Some(shutdown),
            server: Some(server),
        })
    }

    #[cfg(test)]
    pub(crate) fn address(&self) -> SocketAddr {
        self.address
    }

    pub(crate) fn port(&self) -> u16 {
        self.address.port()
    }

    pub(crate) fn child_token(&self) -> SecretString {
        SecretString::new(self.child_token.expose_secret().to_owned())
    }

    pub(crate) async fn stop(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(mut server) = self.server.take() {
            if tokio::time::timeout(SHUTDOWN_GRACE_PERIOD, &mut server)
                .await
                .is_err()
            {
                server.abort();
                let _ = server.await;
            }
        }
    }
}

fn startup_error(result: Result<std::io::Result<()>, tokio::task::JoinError>) -> std::io::Error {
    match result {
        Ok(Err(error)) => error,
        Ok(Ok(())) => std::io::Error::other("local gateway stopped during startup"),
        Err(error) => {
            std::io::Error::other(format!("local gateway task failed during startup: {error}"))
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(server) = self.server.take() {
            server.abort();
        }
    }
}

async fn authorize_request(
    State(state): State<Arc<GatewayState>>,
    mut request: Request,
    next: Next,
) -> Response<Body> {
    if !has_valid_child_token(request.headers(), &state.child_token) {
        return error_response(
            StatusCode::UNAUTHORIZED,
            state.auth_failure.error_type,
            state.auth_failure.message,
        );
    }
    if !headers_are_bounded(request.headers(), request.uri().to_string().len()) {
        return gateway_error(
            StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE,
            "Request headers too large",
        );
    }

    request.extensions_mut().insert(state.context.clone());
    next.run(request).await
}

fn has_valid_child_token(headers: &HeaderMap, expected: &SecretString) -> bool {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let Some(value) = values.next() else {
        return false;
    };
    if values.next().is_some() {
        return false;
    }
    let Some(value) = value
        .to_str()
        .ok()
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    constant_time_equal(expected.expose_secret().as_bytes(), value.as_bytes())
}

fn constant_time_equal(expected: &[u8], provided: &[u8]) -> bool {
    let mut difference = expected.len() ^ provided.len();
    for index in 0..expected.len().max(provided.len()) {
        difference |= usize::from(
            expected.get(index).copied().unwrap_or_default()
                ^ provided.get(index).copied().unwrap_or_default(),
        );
    }
    difference == 0
}

fn headers_are_bounded(headers: &HeaderMap, uri_bytes: usize) -> bool {
    if headers.len() > MAX_REQUEST_HEADER_COUNT {
        return false;
    }
    let bytes = headers.iter().fold(uri_bytes, |total, (name, value)| {
        total
            .saturating_add(name.as_str().len())
            .saturating_add(value.as_bytes().len())
    });
    bytes <= MAX_REQUEST_HEADER_BYTES
}

fn gateway_error(status: StatusCode, message: &str) -> Response<Body> {
    error_response(status, "local_gateway_error", message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Extension;
    use axum::routing::get;
    use reqwest::Client;

    async fn context_endpoint(Extension(context): Extension<LaunchContext>) -> String {
        format!(
            "{}|{}|{}|{}",
            context.profile,
            context.logical_tool_name,
            context.run_id.as_deref().unwrap_or(""),
            context.run_policy.as_deref().unwrap_or("")
        )
    }

    fn context(run_id: &str) -> LaunchContext {
        LaunchContext::new(
            "personal".to_string(),
            "opencode".to_string(),
            Some(run_id.to_string()),
            Some("bounded".to_string()),
        )
    }

    #[tokio::test]
    async fn binds_loopback_ephemeral_port_and_passes_non_secret_context_to_adapter() {
        let app = Router::new().route("/context", get(context_endpoint));
        let server = ServerHandle::start(
            app,
            context("run-123"),
            AuthFailureResponse::new("test_error", "Invalid test credential"),
        )
        .await
        .unwrap();

        assert_eq!(server.address().ip(), Ipv4Addr::LOCALHOST);
        assert_ne!(server.port(), 0);

        let response = Client::new()
            .get(format!("http://{}/context", server.address()))
            .bearer_auth(server.child_token().expose_secret())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.text().await.unwrap(),
            "personal|opencode|run-123|bounded"
        );

        server.stop().await;
    }

    #[tokio::test]
    async fn requires_a_fresh_child_token_for_each_server_and_shuts_down() {
        let app = || Router::new().route("/context", get(context_endpoint));
        let auth_failure = AuthFailureResponse::new("test_error", "Invalid test credential");
        let first = ServerHandle::start(app(), context("run-1"), auth_failure)
            .await
            .unwrap();
        let second = ServerHandle::start(app(), context("run-2"), auth_failure)
            .await
            .unwrap();
        assert_ne!(
            first.child_token().expose_secret(),
            second.child_token().expose_secret()
        );

        let client = Client::new();
        let address = first.address();
        let unauthenticated = client
            .get(format!("http://{address}/context"))
            .send()
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let mut excessive_headers = client
            .get(format!("http://{address}/context"))
            .bearer_auth(first.child_token().expose_secret());
        for index in 0..MAX_REQUEST_HEADER_COUNT {
            excessive_headers = excessive_headers.header(format!("x-aix-test-{index}"), "x");
        }
        let oversized = excessive_headers.send().await.unwrap();
        assert_eq!(
            oversized.status(),
            StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE
        );

        first.stop().await;
        assert!(client
            .get(format!("http://{address}/context"))
            .send()
            .await
            .is_err());
        second.stop().await;
    }
}
