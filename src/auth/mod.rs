#![allow(dead_code)] // Token access is consumed by the tool adapter in #22.

mod protocol;
mod store;
#[cfg(test)]
mod test_fixtures;

use crate::config::ProfileAuth;
use crate::error::AixError;
use crate::secrets::SecretString;
use protocol::OAuthEndpoints;
use serde::Serialize;
use std::time::Duration;

const REFRESH_SAFETY_MARGIN_SECS: u64 = 120;

fn manual_authorization_url(authorize_url: &url::Url) -> url::Url {
    let query_pairs = authorize_url
        .query_pairs()
        // Keep stored account hints in the auto-opened URL, but not in terminal output.
        .filter(|(name, _)| name != "id_token_hint" && name != "login_hint")
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    let mut manual_url = authorize_url.clone();
    manual_url
        .query_pairs_mut()
        .clear()
        .extend_pairs(query_pairs);
    manual_url
}

fn authorization_url_instructions(authorize_url: &url::Url) -> String {
    format!(
        "Opening ChatGPT sign-in in your browser. If it doesn't open, copy and paste this URL:\n{authorize_url}"
    )
}

pub(crate) struct AuthService {
    store: store::AuthStore,
    client: reqwest::Client,
    endpoints: OAuthEndpoints,
}

#[derive(Debug, Serialize)]
pub(crate) struct AuthStatus {
    pub(crate) profile: String,
    pub(crate) auth_type: String,
    pub(crate) state: &'static str,
    pub(crate) connected: bool,
    pub(crate) chatgpt_plan_usage_enabled: Option<bool>,
    pub(crate) access_token_expires_at: Option<u64>,
    pub(crate) email: Option<String>,
}

pub(crate) struct LogoutOutcome {
    pub was_connected: bool,
    pub remote_revocation_confirmed: bool,
}

impl AuthService {
    pub(crate) fn new(timeout: Duration) -> Result<Self, AixError> {
        Self::with_store_and_endpoints(
            store::AuthStore::from_environment()?,
            OAuthEndpoints::production(),
            timeout,
        )
    }

    fn with_store_and_endpoints(
        store: store::AuthStore,
        endpoints: OAuthEndpoints,
        timeout: Duration,
    ) -> Result<Self, AixError> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("aix/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| AixError::AuthNetwork {
                operation: "HTTP client setup",
            })?;
        Ok(Self {
            store,
            client,
            endpoints,
        })
    }

    #[cfg(test)]
    pub(crate) fn with_test_profile_token(
        store_root: std::path::PathBuf,
        profile_name: &str,
        access_token: &str,
        refresh_token: &str,
        expires_at: u64,
        token_endpoint: url::Url,
    ) -> Result<Self, AixError> {
        let store = store::AuthStore::new(store_root);
        store.save(&store::RegistrationRecord {
            version: store::RECORD_VERSION,
            profile: profile_name.to_owned(),
            client_id: "test-client".to_owned(),
            subject: "test-subject".to_owned(),
            email: None,
            scopes: vec![protocol::DIRECT_SCOPE.to_owned()],
            id_token: None,
            access_token: Some(SecretString::new(access_token.to_owned())),
            refresh_token: Some(SecretString::new(refresh_token.to_owned())),
            expires_at: Some(expires_at),
            earliest_refresh_at: None,
        })?;
        let mut endpoints = OAuthEndpoints::production();
        endpoints.token = token_endpoint;
        Self::with_store_and_endpoints(store, endpoints, Duration::from_secs(5))
    }

    #[cfg(test)]
    pub(crate) fn expire_access_token_for_test(&self, profile_name: &str) -> Result<(), AixError> {
        let mut record = self
            .store
            .load(profile_name)?
            .ok_or(AixError::AuthRefreshTokenUnavailable)?;
        record.expires_at = Some(0);
        self.store.save(&record)
    }

    pub(crate) fn status(
        &self,
        profile_name: &str,
        auth: &ProfileAuth,
    ) -> Result<AuthStatus, AixError> {
        match auth {
            ProfileAuth::ApiKey { .. } => Ok(AuthStatus {
                profile: profile_name.to_owned(),
                auth_type: auth.auth_type().to_owned(),
                state: "configured",
                connected: true,
                chatgpt_plan_usage_enabled: None,
                access_token_expires_at: None,
                email: None,
            }),
            ProfileAuth::ChatGpt => {
                let record = self.store.load(profile_name)?;
                let connected = record.as_ref().is_some_and(|record| {
                    record.access_token.is_some() || record.refresh_token.is_some()
                });
                Ok(AuthStatus {
                    profile: profile_name.to_owned(),
                    auth_type: auth.auth_type().to_owned(),
                    state: if connected { "connected" } else { "signed_out" },
                    connected,
                    chatgpt_plan_usage_enabled: record.as_ref().map(|record| {
                        record
                            .scopes
                            .iter()
                            .any(|scope| scope == protocol::DIRECT_SCOPE)
                    }),
                    access_token_expires_at: record
                        .as_ref()
                        .and_then(|record| record.access_token.as_ref().and(record.expires_at)),
                    email: record.and_then(|record| record.email),
                })
            }
        }
    }

    pub(crate) async fn login(&self, profile_name: &str) -> Result<(), AixError> {
        let host_id = self.store.get_or_create_host_id_async().await?;
        let _profile_lock = self.store.lock_profile_async(profile_name).await?;
        let existing = self.store.load(profile_name)?;
        let (listener, mut authorize_url, attempt) = protocol::begin_login(
            &self.endpoints,
            existing.as_ref().map(|record| record.client_id.as_str()),
            &host_id,
            existing
                .as_ref()
                .and_then(|record| record.id_token.as_ref()),
            existing.as_ref().and_then(|record| record.email.as_deref()),
        )?;

        eprintln!(
            "{}",
            authorization_url_instructions(&manual_authorization_url(&authorize_url))
        );
        let opened = webbrowser::open(authorize_url.as_str()).is_ok();
        authorize_url.set_query(None);
        if !opened {
            eprintln!(
                "Could not open a browser automatically; waiting for you to open the URL above."
            );
        }

        let callback = protocol::wait_for_callback(listener, &attempt.state).await?;
        self.complete_login(profile_name, existing, &attempt, callback)
            .await
    }

    async fn complete_login(
        &self,
        profile_name: &str,
        existing: Option<store::RegistrationRecord>,
        attempt: &protocol::LoginAttempt,
        callback: protocol::Callback,
    ) -> Result<(), AixError> {
        let client_id = match existing.as_ref() {
            Some(record) => {
                if callback
                    .client_id
                    .as_ref()
                    .is_some_and(|client_id| client_id != &record.client_id)
                {
                    return Err(AixError::AuthCallbackInvalid);
                }
                record.client_id.clone()
            }
            None => callback
                .client_id
                .clone()
                .filter(|client_id| !client_id.is_empty() && client_id != "dynamic_agent_client")
                .ok_or(AixError::AuthCallbackInvalid)?,
        };

        let tokens = protocol::exchange_code(
            &self.client,
            &self.endpoints,
            &callback,
            attempt,
            &client_id,
        )
        .await?;
        let id_token = tokens.id_token.as_ref().ok_or(AixError::AuthProtocol)?;
        let identity = protocol::validate_id_token(
            &self.client,
            &self.endpoints,
            id_token,
            &client_id,
            &attempt.nonce,
        )
        .await?;
        if existing
            .as_ref()
            .is_some_and(|record| record.subject != identity.subject)
        {
            return Err(AixError::AuthIdentityMismatch);
        }

        let scopes = tokens.scopes.unwrap_or_else(|| {
            callback
                .scope
                .as_deref()
                .map(protocol::split_scopes)
                .unwrap_or_default()
        });
        let record = store::RegistrationRecord {
            version: store::RECORD_VERSION,
            profile: profile_name.to_owned(),
            client_id,
            subject: identity.subject,
            email: identity.email,
            scopes,
            id_token: tokens.id_token,
            access_token: Some(tokens.access_token),
            refresh_token: tokens.refresh_token,
            expires_at: Some(tokens.expires_at),
            earliest_refresh_at: tokens.earliest_refresh_at,
        };
        self.store.save(&record)?;
        Ok(())
    }

    pub(crate) async fn logout(&self, profile_name: &str) -> Result<LogoutOutcome, AixError> {
        let _profile_lock = self.store.lock_profile_async(profile_name).await?;
        let Some(mut record) = self.store.load(profile_name)? else {
            return Ok(LogoutOutcome {
                was_connected: false,
                remote_revocation_confirmed: true,
            });
        };
        let was_connected = record.access_token.is_some() || record.refresh_token.is_some();
        let remote_revocation_confirmed = match record.refresh_token.as_ref() {
            Some(refresh_token) => {
                protocol::revoke(
                    &self.client,
                    &self.endpoints,
                    &record.client_id,
                    refresh_token,
                )
                .await
            }
            None => !was_connected,
        };
        record.id_token = None;
        record.access_token = None;
        record.refresh_token = None;
        record.expires_at = None;
        record.earliest_refresh_at = None;
        self.store.save(&record)?;
        Ok(LogoutOutcome {
            was_connected,
            remote_revocation_confirmed,
        })
    }

    pub(crate) async fn access_token(&self, profile_name: &str) -> Result<SecretString, AixError> {
        let record = self
            .store
            .load(profile_name)?
            .ok_or(AixError::AuthRefreshTokenUnavailable)?;
        ensure_plan_scope(&record.scopes)?;
        let now = protocol::unix_time();
        if let Some(token) = usable_token(&record, now, false) {
            return Ok(token);
        }
        if let Some(token) = usable_token(&record, now, true) {
            return Ok(token);
        }
        if record
            .earliest_refresh_at
            .is_some_and(|earliest| now < earliest)
        {
            return Err(AixError::AuthRefreshNotYetAllowed);
        }

        let _profile_lock = self.store.lock_profile_async(profile_name).await?;
        let mut record = self
            .store
            .load(profile_name)?
            .ok_or(AixError::AuthRefreshTokenUnavailable)?;
        ensure_plan_scope(&record.scopes)?;
        let now = protocol::unix_time();
        if let Some(token) = usable_token(&record, now, false) {
            return Ok(token);
        }
        if let Some(token) = usable_token(&record, now, true) {
            return Ok(token);
        }
        if record
            .earliest_refresh_at
            .is_some_and(|earliest| now < earliest)
        {
            return Err(AixError::AuthRefreshNotYetAllowed);
        }
        if record.refresh_token.is_none() {
            if let Some(token) = unexpired_token(&record, now) {
                return Ok(token);
            }
            return Err(AixError::AuthRefreshTokenUnavailable);
        }

        let refresh_token = record
            .refresh_token
            .as_ref()
            .ok_or(AixError::AuthRefreshTokenUnavailable)?;
        let tokens = match protocol::refresh_tokens(
            &self.client,
            &self.endpoints,
            &record.client_id,
            refresh_token,
        )
        .await
        {
            Ok(tokens) => tokens,
            Err(error @ AixError::AuthOAuthRejected { terminal: true, .. }) => {
                clear_tokens(&mut record);
                self.store.save(&record)?;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        record.access_token = Some(tokens.access_token);
        record.refresh_token = tokens.refresh_token;
        record.expires_at = Some(tokens.expires_at);
        record.earliest_refresh_at = tokens.earliest_refresh_at.or(record.earliest_refresh_at);
        if let Some(scopes) = tokens.scopes {
            record.scopes = scopes;
        }
        let access_token = record
            .access_token
            .as_ref()
            .map(|token| SecretString::new(token.expose_secret().to_owned()))
            .ok_or(AixError::AuthProtocol)?;
        self.store.save(&record)?;
        ensure_plan_scope(&record.scopes)?;
        Ok(access_token)
    }
}

fn ensure_plan_scope(scopes: &[String]) -> Result<(), AixError> {
    if scopes.iter().any(|scope| scope == protocol::DIRECT_SCOPE) {
        Ok(())
    } else {
        Err(AixError::AuthPlanUsageDisabled)
    }
}

fn usable_token(
    record: &store::RegistrationRecord,
    now: u64,
    respect_earliest_refresh: bool,
) -> Option<SecretString> {
    let expires_at = record.expires_at?;
    let access_token = record.access_token.as_ref()?;
    if expires_at <= now {
        return None;
    }
    let refresh_not_yet_allowed = respect_earliest_refresh
        && record
            .earliest_refresh_at
            .is_some_and(|earliest| now < earliest);
    if expires_at > now.saturating_add(REFRESH_SAFETY_MARGIN_SECS) || refresh_not_yet_allowed {
        Some(SecretString::new(access_token.expose_secret().to_owned()))
    } else {
        None
    }
}

fn unexpired_token(record: &store::RegistrationRecord, now: u64) -> Option<SecretString> {
    if record.expires_at? <= now {
        return None;
    }
    record
        .access_token
        .as_ref()
        .map(|token| SecretString::new(token.expose_secret().to_owned()))
}

fn clear_tokens(record: &mut store::RegistrationRecord) {
    record.id_token = None;
    record.access_token = None;
    record.refresh_token = None;
    record.expires_at = None;
    record.earliest_refresh_at = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::store::{AuthStore, RegistrationRecord};
    use crate::auth::test_fixtures::{JWKS, PRIVATE_KEY};
    use assert_fs::TempDir;
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    use serde::Serialize;
    use serde_json::{json, Value};
    use std::sync::Arc;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn connected_record(
        profile: &str,
        expires_at: u64,
        earliest_refresh_at: Option<u64>,
    ) -> RegistrationRecord {
        RegistrationRecord {
            version: store::RECORD_VERSION,
            profile: profile.into(),
            client_id: "issued-client".into(),
            subject: "verified-subject".into(),
            email: Some("user@example.test".into()),
            scopes: vec![protocol::DIRECT_SCOPE.into()],
            id_token: Some(SecretString::new("id-token".into())),
            access_token: Some(SecretString::new("access-token".into())),
            refresh_token: Some(SecretString::new("refresh-token".into())),
            expires_at: Some(expires_at),
            earliest_refresh_at,
        }
    }

    fn service(directory: &TempDir) -> AuthService {
        AuthService::with_store_and_endpoints(
            AuthStore::new(directory.path().to_path_buf()),
            OAuthEndpoints::production(),
            Duration::from_secs(1),
        )
        .unwrap()
    }

    fn service_with_endpoints(
        directory: &TempDir,
        endpoints: protocol::OAuthEndpoints,
    ) -> AuthService {
        AuthService::with_store_and_endpoints(
            AuthStore::new(directory.path().to_path_buf()),
            endpoints,
            Duration::from_secs(3),
        )
        .unwrap()
    }

    #[test]
    fn login_instructions_include_the_manual_authorization_url() {
        let authorize_url = url::Url::parse(
            "https://auth.example.test/authorize?client_id=example&state=one-time-state&nonce=one-time-nonce&login_hint=person%40example.test&id_token_hint=id-token-secret",
        )
        .unwrap();

        let manual_url = manual_authorization_url(&authorize_url);
        let instructions = authorization_url_instructions(&manual_url);
        let query = manual_url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();

        assert!(instructions.contains("copy and paste this URL"));
        assert!(instructions.contains(manual_url.as_str()));
        assert_eq!(
            query.get("state").map(|value| value.as_ref()),
            Some("one-time-state")
        );
        assert_eq!(
            query.get("nonce").map(|value| value.as_ref()),
            Some("one-time-nonce")
        );
        assert!(!query.contains_key("login_hint"));
        assert!(!query.contains_key("id_token_hint"));
        assert!(!instructions.contains("person@example.test"));
        assert!(!instructions.contains("id-token-secret"));
    }

    async fn mock_openid(server: &MockServer) -> protocol::OAuthEndpoints {
        let base = server.uri();
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "issuer": protocol::ISSUER,
                "jwks_uri": format!("{base}/jwks"),
                "revocation_endpoint": format!("{base}/revoke")
            })))
            .mount(server)
            .await;
        let jwks: Value = serde_json::from_str(JWKS).unwrap();
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(jwks))
            .mount(server)
            .await;
        protocol::OAuthEndpoints {
            authorize: url::Url::parse(&format!("{base}/authorize")).unwrap(),
            token: url::Url::parse(&format!("{base}/token")).unwrap(),
            discovery: url::Url::parse(&format!("{base}/.well-known/openid-configuration"))
                .unwrap(),
        }
    }

    #[derive(Serialize)]
    struct TestClaims<'a> {
        iss: &'a str,
        aud: &'a str,
        sub: &'a str,
        exp: u64,
        iat: u64,
        nonce: &'a str,
        email: &'a str,
    }

    fn sign_test_id_token(
        subject: &str,
        client_id: &str,
        nonce: &str,
        issuer: &str,
        exp: u64,
    ) -> String {
        let now = protocol::unix_time();
        let claims = TestClaims {
            iss: issuer,
            aud: client_id,
            sub: subject,
            exp,
            iat: now,
            nonce,
            email: "user@example.test",
        };
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("aix-test-key".to_owned());
        encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(PRIVATE_KEY).unwrap(),
        )
        .unwrap()
    }

    fn login_attempt(nonce: &str) -> protocol::LoginAttempt {
        protocol::LoginAttempt {
            state: "state-value".into(),
            nonce: nonce.into(),
            verifier: SecretString::new("pkce-verifier".into()),
            redirect_uri: "http://127.0.0.1:54321/auth/callback".into(),
        }
    }

    fn token_response(id_token: String, scopes: &str) -> Value {
        json!({
            "access_token": "access-token-new",
            "refresh_token": "refresh-token-new",
            "id_token": id_token,
            "token_type": "Bearer",
            "expires_in": 3600,
            "scope": scopes,
            "earliest_refresh_at": protocol::unix_time()
        })
    }

    #[test]
    fn status_is_offline_and_does_not_include_token_values() {
        let dir = TempDir::new().unwrap();
        let service = service(&dir);
        service
            .store
            .save(&connected_record("personal", 1_900_000_000, None))
            .unwrap();
        let status = service.status("personal", &ProfileAuth::ChatGpt).unwrap();
        let json = serde_json::to_string(&status).unwrap();
        assert!(status.connected);
        assert!(status.chatgpt_plan_usage_enabled.unwrap());
        assert!(!json.contains("access-token"));
        assert!(!json.contains("refresh-token"));
        assert!(!json.contains("id-token"));
    }

    #[test]
    fn an_earliest_refresh_time_preserves_a_still_valid_access_token() {
        let now = protocol::unix_time();
        let record = connected_record("personal", now + 30, Some(now + 600));
        assert!(usable_token(&record, now, true).is_some());
        assert!(usable_token(&record, now, false).is_none());
    }

    #[tokio::test]
    async fn initial_registration_exchanges_with_the_issued_client_id_and_stores_verified_identity()
    {
        let server = MockServer::start().await;
        let endpoints = mock_openid(&server).await;
        let id_token = sign_test_id_token(
            "verified-subject",
            "issued-oai-client",
            "login-nonce",
            protocol::ISSUER,
            protocol::unix_time() + 3600,
        );
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=authorization_code"))
            .and(body_string_contains("client_id=issued-oai-client"))
            .and(body_string_contains("code_verifier=pkce-verifier"))
            .and(body_string_contains(
                "resource=https%3A%2F%2Fapi.openai.com%2Fv1",
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(token_response(id_token, protocol::REQUIRED_SCOPES)),
            )
            .expect(1)
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let service = service_with_endpoints(&dir, endpoints);
        service
            .complete_login(
                "personal",
                None,
                &login_attempt("login-nonce"),
                protocol::Callback {
                    code: SecretString::new("one-time-code-secret".into()),
                    client_id: Some("issued-oai-client".into()),
                    scope: None,
                },
            )
            .await
            .unwrap();

        let record = service.store.load("personal").unwrap().unwrap();
        assert_eq!(record.client_id, "issued-oai-client");
        assert_eq!(record.subject, "verified-subject");
        assert_eq!(record.email.as_deref(), Some("user@example.test"));
        assert!(record
            .scopes
            .iter()
            .any(|scope| scope == protocol::DIRECT_SCOPE));
        assert_eq!(
            record.access_token.unwrap().expose_secret(),
            "access-token-new"
        );
        assert_eq!(
            record.refresh_token.unwrap().expose_secret(),
            "refresh-token-new"
        );
    }

    #[tokio::test]
    async fn new_registration_requires_an_issued_client_id_and_reauthorization_preserves_identity()
    {
        let dir = TempDir::new().unwrap();
        let service = service(&dir);
        let missing_id = service
            .complete_login(
                "personal",
                None,
                &login_attempt("nonce"),
                protocol::Callback {
                    code: SecretString::new("code-secret".into()),
                    client_id: None,
                    scope: None,
                },
            )
            .await;
        assert!(matches!(missing_id, Err(AixError::AuthCallbackInvalid)));
        assert!(service.store.load("personal").unwrap().is_none());

        let server = MockServer::start().await;
        let endpoints = mock_openid(&server).await;
        let replacement_token = sign_test_id_token(
            "different-subject",
            "issued-client",
            "return-nonce",
            protocol::ISSUER,
            protocol::unix_time() + 3600,
        );
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(token_response(replacement_token, protocol::REQUIRED_SCOPES)),
            )
            .mount(&server)
            .await;
        let prior = connected_record("personal", protocol::unix_time() + 7200, None);
        service.store.save(&prior).unwrap();
        let service = service_with_endpoints(&dir, endpoints);
        let changed_subject = service
            .complete_login(
                "personal",
                service.store.load("personal").unwrap(),
                &login_attempt("return-nonce"),
                protocol::Callback {
                    code: SecretString::new("code-secret".into()),
                    client_id: None,
                    scope: None,
                },
            )
            .await;
        assert!(matches!(
            changed_subject,
            Err(AixError::AuthIdentityMismatch)
        ));
        let retained = service.store.load("personal").unwrap().unwrap();
        assert_eq!(retained.subject, prior.subject);
        assert_eq!(
            retained.access_token.unwrap().expose_secret(),
            "access-token"
        );

        let mismatched_client = service
            .complete_login(
                "personal",
                service.store.load("personal").unwrap(),
                &login_attempt("return-nonce"),
                protocol::Callback {
                    code: SecretString::new("code-secret".into()),
                    client_id: Some("another-issued-client".into()),
                    scope: None,
                },
            )
            .await;
        assert!(matches!(
            mismatched_client,
            Err(AixError::AuthCallbackInvalid)
        ));
    }

    #[tokio::test]
    async fn id_token_requires_signature_issuer_audience_expiry_and_nonce() {
        let server = MockServer::start().await;
        let endpoints = mock_openid(&server).await;
        let client = reqwest::Client::new();
        let valid_token = sign_test_id_token(
            "subject",
            "issued-client",
            "expected-nonce",
            protocol::ISSUER,
            protocol::unix_time() + 3600,
        );
        let identity = protocol::validate_id_token(
            &client,
            &endpoints,
            &SecretString::new(valid_token.clone()),
            "issued-client",
            "expected-nonce",
        )
        .await
        .unwrap();
        assert_eq!(identity.subject, "subject");
        assert_eq!(identity.email.as_deref(), Some("user@example.test"));

        let invalid_tokens = [
            sign_test_id_token(
                "subject",
                "issued-client",
                "wrong-nonce",
                protocol::ISSUER,
                protocol::unix_time() + 3600,
            ),
            sign_test_id_token(
                "subject",
                "another-client",
                "expected-nonce",
                protocol::ISSUER,
                protocol::unix_time() + 3600,
            ),
            sign_test_id_token(
                "subject",
                "issued-client",
                "expected-nonce",
                "https://wrong-issuer.example",
                protocol::unix_time() + 3600,
            ),
            sign_test_id_token(
                "subject",
                "issued-client",
                "expected-nonce",
                protocol::ISSUER,
                protocol::unix_time().saturating_sub(3600),
            ),
            "not-a-signed-token".to_owned(),
        ];
        for token in invalid_tokens {
            assert!(matches!(
                protocol::validate_id_token(
                    &client,
                    &endpoints,
                    &SecretString::new(token),
                    "issued-client",
                    "expected-nonce",
                )
                .await,
                Err(AixError::AuthIdentityInvalid)
            ));
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_refreshes_serialize_and_observe_rotated_credentials() {
        let server = MockServer::start().await;
        let endpoints = protocol::OAuthEndpoints {
            authorize: url::Url::parse(&format!("{}/authorize", server.uri())).unwrap(),
            token: url::Url::parse(&format!("{}/token", server.uri())).unwrap(),
            discovery: url::Url::parse(&format!(
                "{}/.well-known/openid-configuration",
                server.uri()
            ))
            .unwrap(),
        };
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .and(body_string_contains("client_id=issued-client"))
            .and(body_string_contains("refresh_token=refresh-token"))
            .and(body_string_contains(
                "resource=https%3A%2F%2Fapi.openai.com%2Fv1",
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(100))
                    .set_body_json(json!({
                        "access_token": "access-token-rotated",
                        "refresh_token": "refresh-token-rotated",
                        "expires_in": 3600,
                        "scope": protocol::REQUIRED_SCOPES
                    })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let service = Arc::new(service_with_endpoints(&dir, endpoints));
        service
            .store
            .save(&connected_record(
                "personal",
                protocol::unix_time().saturating_sub(1),
                None,
            ))
            .unwrap();
        let (first, second) = tokio::join!(
            service.access_token("personal"),
            service.access_token("personal")
        );
        assert_eq!(first.unwrap().expose_secret(), "access-token-rotated");
        assert_eq!(second.unwrap().expose_secret(), "access-token-rotated");
        let saved = service.store.load("personal").unwrap().unwrap();
        assert_eq!(
            saved.refresh_token.unwrap().expose_secret(),
            "refresh-token-rotated"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn transient_refresh_failure_preserves_tokens_but_terminal_failure_clears_them() {
        let server = MockServer::start().await;
        let endpoints = protocol::OAuthEndpoints {
            authorize: url::Url::parse(&format!("{}/authorize", server.uri())).unwrap(),
            token: url::Url::parse(&format!("{}/token", server.uri())).unwrap(),
            discovery: url::Url::parse(&format!(
                "{}/.well-known/openid-configuration",
                server.uri()
            ))
            .unwrap(),
        };
        let dir = TempDir::new().unwrap();
        let service = service_with_endpoints(&dir, endpoints);
        service
            .store
            .save(&connected_record("personal", 0, None))
            .unwrap();
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        assert!(matches!(
            service.access_token("personal").await,
            Err(AixError::AuthNetwork { .. })
        ));
        let preserved = service.store.load("personal").unwrap().unwrap();
        assert_eq!(
            preserved.refresh_token.unwrap().expose_secret(),
            "refresh-token"
        );
        assert_eq!(
            preserved.access_token.unwrap().expose_secret(),
            "access-token"
        );

        server.reset().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(
                ResponseTemplate::new(400).set_body_json(json!({"error":"invalid_grant"})),
            )
            .mount(&server)
            .await;
        assert!(matches!(
            service.access_token("personal").await,
            Err(AixError::AuthOAuthRejected { terminal: true, .. })
        ));
        let cleared = service.store.load("personal").unwrap().unwrap();
        assert_eq!(cleared.client_id, "issued-client");
        assert_eq!(cleared.subject, "verified-subject");
        assert!(cleared.id_token.is_none());
        assert!(cleared.access_token.is_none());
        assert!(cleared.refresh_token.is_none());
    }

    #[tokio::test]
    async fn logout_clears_local_tokens_even_when_revocation_is_unconfirmed() {
        let server = MockServer::start().await;
        let endpoints = mock_openid(&server).await;
        Mock::given(method("POST"))
            .and(path("/revoke"))
            .and(body_string_contains("token_type_hint=refresh_token"))
            .and(body_string_contains("client_id=issued-client"))
            .respond_with(ResponseTemplate::new(503))
            .expect(3)
            .mount(&server)
            .await;
        let dir = TempDir::new().unwrap();
        let service = service_with_endpoints(&dir, endpoints);
        service
            .store
            .save(&connected_record(
                "personal",
                protocol::unix_time() + 3600,
                None,
            ))
            .unwrap();
        let outcome = service.logout("personal").await.unwrap();
        assert!(outcome.was_connected);
        assert!(!outcome.remote_revocation_confirmed);
        let record = service.store.load("personal").unwrap().unwrap();
        assert_eq!(record.client_id, "issued-client");
        assert_eq!(record.subject, "verified-subject");
        assert!(record.id_token.is_none());
        assert!(record.access_token.is_none());
        assert!(record.refresh_token.is_none());
        assert_eq!(
            service
                .status("personal", &ProfileAuth::ChatGpt)
                .unwrap()
                .state,
            "signed_out"
        );
    }

    #[tokio::test]
    async fn logout_confirms_revocation_and_clears_local_tokens() {
        let server = MockServer::start().await;
        let endpoints = mock_openid(&server).await;
        Mock::given(method("POST"))
            .and(path("/revoke"))
            .and(body_string_contains("token_type_hint=refresh_token"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let dir = TempDir::new().unwrap();
        let service = service_with_endpoints(&dir, endpoints);
        service
            .store
            .save(&connected_record(
                "personal",
                protocol::unix_time() + 3600,
                None,
            ))
            .unwrap();

        let outcome = service.logout("personal").await.unwrap();
        assert!(outcome.was_connected);
        assert!(outcome.remote_revocation_confirmed);
        let record = service.store.load("personal").unwrap().unwrap();
        assert!(record.id_token.is_none());
        assert!(record.access_token.is_none());
        assert!(record.refresh_token.is_none());
    }

    #[tokio::test]
    async fn a_valid_identity_without_direct_scope_is_saved_but_cannot_supply_a_token() {
        let server = MockServer::start().await;
        let endpoints = mock_openid(&server).await;
        let id_token = sign_test_id_token(
            "verified-subject",
            "issued-client",
            "nonce",
            protocol::ISSUER,
            protocol::unix_time() + 3600,
        );
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_response(
                id_token,
                "openid profile email offline_access",
            )))
            .mount(&server)
            .await;
        let dir = TempDir::new().unwrap();
        let service = service_with_endpoints(&dir, endpoints);
        service
            .complete_login(
                "personal",
                None,
                &login_attempt("nonce"),
                protocol::Callback {
                    code: SecretString::new("authorization-code".into()),
                    client_id: Some("issued-client".into()),
                    scope: None,
                },
            )
            .await
            .unwrap();
        let status = service.status("personal", &ProfileAuth::ChatGpt).unwrap();
        assert!(status.connected);
        assert_eq!(status.chatgpt_plan_usage_enabled, Some(false));
        assert!(matches!(
            service.access_token("personal").await,
            Err(AixError::AuthPlanUsageDisabled)
        ));
    }
}
