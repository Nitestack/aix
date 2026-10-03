use crate::error::AixError;
use crate::secrets::SecretString;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use url::Url;
use uuid::Uuid;
use zeroize::Zeroize;

pub(super) const ISSUER: &str = "https://auth.openai.com";
pub(super) const REQUIRED_SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
pub(super) const RESOURCE: &str = "https://api.openai.com/v1";

const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_CALLBACK_REQUEST_LINE_BYTES: u64 = 16 * 1024;
const CALLBACK_PATH: &str = "/auth/callback";
const AUTHORIZE_URL: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";
const DISCOVERY_URL: &str = "https://auth.openai.com/.well-known/openid-configuration";
pub(super) const DIRECT_SCOPE: &str = "chatgpt.tokens.use.direct";

#[derive(Clone, Debug)]
pub(super) struct OAuthEndpoints {
    pub authorize: Url,
    pub token: Url,
    pub discovery: Url,
}

impl OAuthEndpoints {
    pub fn production() -> Self {
        Self {
            authorize: Url::parse(AUTHORIZE_URL).expect("valid OpenAI authorization URL"),
            token: Url::parse(TOKEN_URL).expect("valid OpenAI token URL"),
            discovery: Url::parse(DISCOVERY_URL).expect("valid OpenID discovery URL"),
        }
    }
}

pub(super) struct LoginAttempt {
    pub state: String,
    pub nonce: String,
    pub verifier: SecretString,
    pub redirect_uri: String,
}

pub(super) struct Callback {
    pub code: SecretString,
    pub client_id: Option<String>,
    pub scope: Option<String>,
}

pub(super) struct TokenSet {
    pub access_token: SecretString,
    pub refresh_token: Option<SecretString>,
    pub id_token: Option<SecretString>,
    pub scopes: Option<Vec<String>>,
    pub expires_at: u64,
    pub earliest_refresh_at: Option<u64>,
}

pub(super) struct VerifiedIdentity {
    pub subject: String,
    pub email: Option<String>,
}

#[derive(Deserialize)]
struct OpenIdMetadata {
    issuer: String,
    jwks_uri: String,
    revocation_endpoint: Option<String>,
}

pub(super) struct DiscoveredEndpoints {
    pub jwks_uri: Url,
    pub revocation_endpoint: Option<Url>,
}

pub(super) fn begin_login(
    endpoints: &OAuthEndpoints,
    client_id: Option<&str>,
    host_id: &str,
    id_token_hint: Option<&SecretString>,
    login_hint: Option<&str>,
) -> Result<(TcpListener, Url, LoginAttempt), AixError> {
    let listener =
        std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|_| AixError::AuthNetwork {
            operation: "callback listener",
        })?;
    listener
        .set_nonblocking(true)
        .map_err(|_| AixError::AuthNetwork {
            operation: "callback listener",
        })?;
    let listener = TcpListener::from_std(listener).map_err(|_| AixError::AuthNetwork {
        operation: "callback listener",
    })?;
    let port = listener
        .local_addr()
        .map_err(|_| AixError::AuthNetwork {
            operation: "callback listener",
        })?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");

    let state = random_url_token();
    let nonce = random_url_token();
    let verifier = random_url_token();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));

    let mut authorize_url = endpoints.authorize.clone();
    {
        let mut query = authorize_url.query_pairs_mut();
        query
            .append_pair("client_id", client_id.unwrap_or("dynamic_agent_client"))
            .append_pair("ext_agent_host_id", host_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("scope", REQUIRED_SCOPES)
            .append_pair("resource", RESOURCE)
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge_method", "S256")
            .append_pair("code_challenge", &challenge);
        if client_id.is_none() {
            query.append_pair("agent_name_hint", "aix");
        }
        if let Some(hint) = id_token_hint {
            query.append_pair("id_token_hint", hint.expose_secret());
        }
        if let Some(email) = login_hint {
            query.append_pair("login_hint", email);
        }
    }

    Ok((
        listener,
        authorize_url,
        LoginAttempt {
            state,
            nonce,
            verifier: SecretString::new(verifier),
            redirect_uri,
        },
    ))
}

pub(super) async fn wait_for_callback(
    listener: TcpListener,
    expected_state: &str,
) -> Result<Callback, AixError> {
    let (stream, _) = tokio::time::timeout(CALLBACK_TIMEOUT, listener.accept())
        .await
        .map_err(|_| AixError::AuthCallbackTimeout)?
        .map_err(|_| AixError::AuthNetwork {
            operation: "browser callback",
        })?;
    let (mut stream, mut query) = read_callback(stream).await?;

    let returned_state = query.get("state").map(String::as_str);
    if returned_state != Some(expected_state) {
        respond(&mut stream, false).await;
        return Err(AixError::AuthCallbackInvalid);
    }
    if query.contains_key("error") {
        respond(&mut stream, false).await;
        return Err(AixError::AuthAuthorizationDenied);
    }
    let Some(code) = query.remove("code").filter(|code| !code.is_empty()) else {
        respond(&mut stream, false).await;
        return Err(AixError::AuthCallbackInvalid);
    };
    respond(&mut stream, true).await;
    Ok(Callback {
        code: SecretString::new(code),
        client_id: query.remove("client_id"),
        scope: query.remove("scope"),
    })
}

async fn read_callback(
    mut stream: TcpStream,
) -> Result<(TcpStream, std::collections::HashMap<String, String>), AixError> {
    let mut request_line = String::new();
    let read = {
        let mut reader = BufReader::new(&mut stream).take(MAX_CALLBACK_REQUEST_LINE_BYTES + 1);
        tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut request_line))
            .await
            .map_err(|_| AixError::AuthCallbackTimeout)?
            .map_err(|_| AixError::AuthNetwork {
                operation: "browser callback",
            })?
    };
    if read == 0 || read as u64 > MAX_CALLBACK_REQUEST_LINE_BYTES || !request_line.ends_with("\r\n")
    {
        respond(&mut stream, false).await;
        return Err(AixError::AuthCallbackInvalid);
    }
    let mut line = request_line.split_whitespace();
    let (Some("GET"), Some(target), Some("HTTP/1.1" | "HTTP/1.0")) =
        (line.next(), line.next(), line.next())
    else {
        respond(&mut stream, false).await;
        return Err(AixError::AuthCallbackInvalid);
    };
    let callback_url = match Url::parse(&format!("http://127.0.0.1{target}")) {
        Ok(url) => url,
        Err(_) => {
            respond(&mut stream, false).await;
            return Err(AixError::AuthCallbackInvalid);
        }
    };
    if callback_url.path() != CALLBACK_PATH {
        respond(&mut stream, false).await;
        return Err(AixError::AuthCallbackInvalid);
    }
    let query = callback_url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    Ok((stream, query))
}

async fn respond(stream: &mut TcpStream, accepted: bool) {
    let (status, message) = if accepted {
        (
            "200 OK",
            "Sign-in response received. You may close this tab.",
        )
    } else {
        (
            "400 Bad Request",
            "Sign-in could not be verified. Return to aix.",
        )
    };
    let body = format!("<!doctype html><title>aix sign-in</title><p>{message}</p>");
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

pub(super) async fn exchange_code(
    client: &reqwest::Client,
    endpoints: &OAuthEndpoints,
    callback: &Callback,
    attempt: &LoginAttempt,
    client_id: &str,
) -> Result<TokenSet, AixError> {
    let verifier = attempt.verifier.expose_secret();
    let params = [
        ("grant_type", "authorization_code"),
        ("code", callback.code.expose_secret()),
        ("client_id", client_id),
        ("code_verifier", verifier),
        ("redirect_uri", attempt.redirect_uri.as_str()),
        ("resource", RESOURCE),
    ];
    request_tokens(client, &endpoints.token, &params, "token exchange", false).await
}

pub(super) async fn refresh_tokens(
    client: &reqwest::Client,
    endpoints: &OAuthEndpoints,
    client_id: &str,
    refresh_token: &SecretString,
) -> Result<TokenSet, AixError> {
    let params = [
        ("grant_type", "refresh_token"),
        ("client_id", client_id),
        ("refresh_token", refresh_token.expose_secret()),
        ("resource", RESOURCE),
    ];
    request_tokens(client, &endpoints.token, &params, "token refresh", true).await
}

async fn request_tokens(
    client: &reqwest::Client,
    endpoint: &Url,
    params: &[(&str, &str)],
    operation: &'static str,
    require_refresh_token: bool,
) -> Result<TokenSet, AixError> {
    let response = client
        .post(endpoint.clone())
        .header(reqwest::header::ACCEPT, "application/json")
        .form(params)
        .send()
        .await
        .map_err(|_| AixError::AuthNetwork { operation })?;
    let status = response.status();
    if !status.is_success() {
        let body = response.json::<Value>().await.ok();
        let oauth_error = body
            .as_ref()
            .and_then(|body| body.get("error"))
            .and_then(Value::as_str);
        if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AixError::AuthNetwork { operation });
        }
        return Err(AixError::AuthOAuthRejected {
            status: status.as_u16(),
            terminal: matches!(
                oauth_error,
                Some(
                    "invalid_grant"
                        | "invalid_refresh_token"
                        | "token_expired"
                        | "refresh_token_expired"
                        | "refresh_token_invalidated"
                        | "refresh_token_reused"
                        | "invalid_client"
                )
            ) || matches!(status.as_u16(), 401 | 403),
        });
    }
    let mut body = response
        .json::<Value>()
        .await
        .map_err(|_| AixError::AuthProtocol)?;

    let access_token =
        take_optional_secret(&mut body, "access_token").ok_or(AixError::AuthProtocol)?;
    let refresh_token = take_optional_secret(&mut body, "refresh_token");
    if require_refresh_token && refresh_token.is_none() {
        return Err(AixError::AuthProtocol);
    }
    let id_token = take_optional_secret(&mut body, "id_token");
    let expires_in = take_u64(&mut body, "expires_in").ok_or(AixError::AuthProtocol)?;
    let now = unix_time();
    let expires_at = now.checked_add(expires_in).ok_or(AixError::AuthProtocol)?;
    let scopes = take_optional_string(&mut body, "scope").map(|scope| split_scopes(&scope));
    let earliest_refresh_at = body
        .as_object_mut()
        .and_then(|object| object.remove("earliest_refresh_at"))
        .and_then(timestamp_value);
    Ok(TokenSet {
        access_token,
        refresh_token,
        id_token,
        scopes,
        expires_at,
        earliest_refresh_at,
    })
}

pub(super) async fn discover(
    client: &reqwest::Client,
    endpoints: &OAuthEndpoints,
) -> Result<DiscoveredEndpoints, AixError> {
    let response = client
        .get(endpoints.discovery.clone())
        .send()
        .await
        .map_err(|_| AixError::AuthNetwork {
            operation: "OpenID discovery",
        })?;
    if !response.status().is_success() {
        return Err(AixError::AuthNetwork {
            operation: "OpenID discovery",
        });
    }
    let metadata = response
        .json::<OpenIdMetadata>()
        .await
        .map_err(|_| AixError::AuthProtocol)?;
    if metadata.issuer != ISSUER {
        return Err(AixError::AuthProtocol);
    }
    let jwks_uri = Url::parse(&metadata.jwks_uri).map_err(|_| AixError::AuthProtocol)?;
    let revocation_endpoint = metadata
        .revocation_endpoint
        .map(|endpoint| Url::parse(&endpoint).map_err(|_| AixError::AuthProtocol))
        .transpose()?;
    Ok(DiscoveredEndpoints {
        jwks_uri,
        revocation_endpoint,
    })
}

pub(super) async fn validate_id_token(
    client: &reqwest::Client,
    endpoints: &OAuthEndpoints,
    id_token: &SecretString,
    client_id: &str,
    expected_nonce: &str,
) -> Result<VerifiedIdentity, AixError> {
    let header =
        decode_header(id_token.expose_secret()).map_err(|_| AixError::AuthIdentityInvalid)?;
    if header.alg != Algorithm::RS256 {
        return Err(AixError::AuthIdentityInvalid);
    }
    let kid = header.kid.ok_or(AixError::AuthIdentityInvalid)?;
    let discovered = discover(client, endpoints).await?;
    let response =
        client
            .get(discovered.jwks_uri)
            .send()
            .await
            .map_err(|_| AixError::AuthNetwork {
                operation: "JWKS lookup",
            })?;
    if !response.status().is_success() {
        return Err(AixError::AuthNetwork {
            operation: "JWKS lookup",
        });
    }
    let jwks = response
        .json::<JwkSet>()
        .await
        .map_err(|_| AixError::AuthProtocol)?;
    let jwk = jwks
        .keys
        .iter()
        .find(|key| key.common.key_id.as_deref() == Some(kid.as_str()))
        .ok_or(AixError::AuthIdentityInvalid)?;
    let decoding_key = DecodingKey::from_jwk(jwk).map_err(|_| AixError::AuthIdentityInvalid)?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[client_id]);
    validation.leeway = 60;
    validation.required_spec_claims.insert("sub".to_owned());
    validation.required_spec_claims.insert("iss".to_owned());
    validation.required_spec_claims.insert("aud".to_owned());
    let decoded = decode::<Value>(id_token.expose_secret(), &decoding_key, &validation)
        .map_err(|_| AixError::AuthIdentityInvalid)?;
    let claims = decoded.claims;
    let subject = claims
        .get("sub")
        .and_then(Value::as_str)
        .filter(|subject| !subject.is_empty())
        .ok_or(AixError::AuthIdentityInvalid)?
        .to_owned();
    if claims.get("nonce").and_then(Value::as_str) != Some(expected_nonce) {
        return Err(AixError::AuthIdentityInvalid);
    }
    let email = claims
        .get("email")
        .and_then(Value::as_str)
        .filter(|email| !email.is_empty())
        .map(str::to_owned);
    Ok(VerifiedIdentity { subject, email })
}

pub(super) async fn revoke(
    client: &reqwest::Client,
    endpoints: &OAuthEndpoints,
    client_id: &str,
    refresh_token: &SecretString,
) -> bool {
    let Ok(discovered) = discover(client, endpoints).await else {
        return false;
    };
    let Some(endpoint) = discovered.revocation_endpoint else {
        return false;
    };
    for attempt in 0..3 {
        let result = client
            .post(endpoint.clone())
            .form(&[
                ("token", refresh_token.expose_secret()),
                ("token_type_hint", "refresh_token"),
                ("client_id", client_id),
            ])
            .send()
            .await;
        match result {
            Ok(response) if response.status().as_u16() == 200 => return true,
            Ok(response) if !response.status().is_server_error() => return false,
            _ if attempt < 2 => {
                tokio::time::sleep(Duration::from_millis(200 * (attempt + 1))).await;
            }
            _ => return false,
        }
    }
    false
}

pub(super) fn random_url_token() -> String {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    let token = URL_SAFE_NO_PAD.encode(bytes);
    bytes.zeroize();
    token
}

pub(super) fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn take_optional_secret(body: &mut Value, name: &str) -> Option<SecretString> {
    match body.as_object_mut()?.remove(name)? {
        Value::String(value) if !value.is_empty() => Some(SecretString::new(value)),
        _ => None,
    }
}

fn take_optional_string(body: &mut Value, name: &str) -> Option<String> {
    match body.as_object_mut()?.remove(name)? {
        Value::String(value) => Some(value),
        _ => None,
    }
}

fn take_u64(body: &mut Value, name: &str) -> Option<u64> {
    body.as_object_mut()?.remove(name)?.as_u64()
}

fn timestamp_value(value: Value) -> Option<u64> {
    if let Some(timestamp) = value.as_u64() {
        return Some(timestamp);
    }
    let value = value.as_str()?;
    if let Ok(timestamp) = value.parse::<u64>() {
        return Some(timestamp);
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .and_then(|date_time| u64::try_from(date_time.timestamp()).ok())
}

pub(super) fn split_scopes(scopes: &str) -> Vec<String> {
    scopes
        .split_ascii_whitespace()
        .filter(|scope| !scope.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn authorization_request_uses_dynamic_registration_only_for_new_profile() {
        let endpoints = OAuthEndpoints::production();
        let (listener, url, attempt) =
            begin_login(&endpoints, None, "urn:uuid:stable", None, None).unwrap();
        drop(listener);
        let query = url
            .query_pairs()
            .into_owned()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(query["client_id"], "dynamic_agent_client");
        assert_eq!(query["agent_name_hint"], "aix");
        assert_eq!(query["ext_agent_host_id"], "urn:uuid:stable");
        assert_eq!(query["response_type"], "code");
        assert_eq!(query["redirect_uri"], attempt.redirect_uri);
        assert_eq!(query["scope"], REQUIRED_SCOPES);
        assert_eq!(query["resource"], RESOURCE);
        assert_eq!(query["code_challenge_method"], "S256");
        assert!(!query["state"].is_empty());
        assert!(!query["nonce"].is_empty());
        assert!(!query["code_challenge"].is_empty());
    }

    #[tokio::test]
    async fn returning_authorization_reuses_client_and_redacts_hint_from_debug() {
        let endpoints = OAuthEndpoints::production();
        let hint = SecretString::new("retained-id-token-secret".into());
        let (listener, url, _) = begin_login(
            &endpoints,
            Some("issued-client"),
            "urn:uuid:stable",
            Some(&hint),
            Some("user@example.test"),
        )
        .unwrap();
        drop(listener);
        assert_eq!(
            url.query_pairs().find(|(k, _)| k == "client_id").unwrap().1,
            "issued-client"
        );
        assert!(url
            .query_pairs()
            .any(|(k, v)| k == "id_token_hint" && v == "retained-id-token-secret"));
        assert!(!url.as_str().contains("agent_name_hint"));
        assert!(!format!("{:?}", hint).contains("retained-id-token-secret"));
    }

    #[test]
    fn scopes_and_refresh_time_are_parsed_without_exposing_token_strings() {
        assert_eq!(
            split_scopes("openid email chatgpt.tokens.use.direct"),
            ["openid", "email", "chatgpt.tokens.use.direct"]
        );
        assert_eq!(
            timestamp_value(serde_json::json!(1_800_000_000)),
            Some(1_800_000_000)
        );
        assert_eq!(
            timestamp_value(serde_json::json!("2027-01-01T00:00:00Z")),
            Some(1_798_761_600)
        );
    }

    #[tokio::test]
    async fn callback_rejects_a_mismatched_state_and_oauth_denial() {
        let endpoints = OAuthEndpoints::production();
        let (listener, _, attempt) =
            begin_login(&endpoints, None, "urn:uuid:stable", None, None).unwrap();
        let port = listener.local_addr().unwrap().port();
        let sender = tokio::spawn(send_callback(
            port,
            "state=wrong&code=code-secret".to_string(),
        ));
        assert!(matches!(
            wait_for_callback(listener, &attempt.state).await,
            Err(AixError::AuthCallbackInvalid)
        ));
        let response = sender.await.unwrap();
        assert!(response.starts_with("HTTP/1.1 400 Bad Request"));
        assert!(response.contains("could not be verified"));

        let (listener, _, attempt) =
            begin_login(&endpoints, None, "urn:uuid:stable", None, None).unwrap();
        let port = listener.local_addr().unwrap().port();
        let sender = tokio::spawn(send_callback(
            port,
            format!("state={}&error=access_denied", attempt.state),
        ));
        assert!(matches!(
            wait_for_callback(listener, &attempt.state).await,
            Err(AixError::AuthAuthorizationDenied)
        ));
        let response = sender.await.unwrap();
        assert!(response.starts_with("HTTP/1.1 400 Bad Request"));

        let (listener, _, attempt) =
            begin_login(&endpoints, None, "urn:uuid:stable", None, None).unwrap();
        let port = listener.local_addr().unwrap().port();
        let sender = tokio::spawn(send_callback(
            port,
            format!("state={}&code=one-time-code", attempt.state),
        ));
        let callback = wait_for_callback(listener, &attempt.state).await.unwrap();
        assert_eq!(callback.code.expose_secret(), "one-time-code");
        assert!(sender.await.unwrap().starts_with("HTTP/1.1 200 OK"));
    }

    async fn send_callback(port: u16, query: String) -> String {
        let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)).await else {
            panic!("could not connect to loopback callback");
        };
        let request =
            format!("GET {CALLBACK_PATH}?{query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut response).await;
        String::from_utf8(response).unwrap()
    }
}
