use crate::error::{AixError, DoctorFailureCategory};

pub(super) struct CheckFailure {
    pub(super) message: String,
    pub(super) hint: &'static str,
    pub(super) category: DoctorFailureCategory,
}

impl CheckFailure {
    pub(super) fn new(
        message: impl Into<String>,
        hint: &'static str,
        category: DoctorFailureCategory,
    ) -> Self {
        Self {
            message: message.into(),
            hint,
            category,
        }
    }
}

pub(super) fn safe_load_failure(error: &AixError) -> CheckFailure {
    match error {
        AixError::UnknownFormat { .. } => CheckFailure::new(
            "config file has an unsupported format",
            "check the config file format, syntax, and read permissions",
            DoctorFailureCategory::Config,
        ),
        _ => CheckFailure::new(
            "config file could not be read or parsed",
            "check the config file format, syntax, and read permissions",
            DoctorFailureCategory::Config,
        ),
    }
}

pub(super) fn safe_validation_failure(error: &AixError) -> CheckFailure {
    let (message, hint) = match error {
        AixError::NoProfilesConfigured => (
            "config defines no profiles",
            "define at least one profile under [profiles]",
        ),
        AixError::UnknownDefaultProfile(_) => (
            "default_profile does not name a configured profile",
            "set default_profile to a name defined under [profiles]",
        ),
        AixError::DuplicateLabel { .. } => (
            "profile labels must be unique",
            "give every profile a distinct label",
        ),
        AixError::EmptyModelDefault { .. } => (
            "a configured model default is empty",
            "configure a non-empty model default or remove it",
        ),
        AixError::EmptyModelAliasName { .. } => (
            "a configured model alias name is empty",
            "give each model alias a non-empty name",
        ),
        AixError::EmptyModelAliasTarget { .. } => (
            "a configured model alias target is empty",
            "set each model alias to a non-empty model ID",
        ),
        AixError::InvalidEnvironmentVariableName { .. }
        | AixError::InvalidToolEnvironmentVariableName { .. } => (
            "a configured environment variable name is invalid",
            "use a valid environment variable name",
        ),
        AixError::EmptyToolName => (
            "a configured tool name is empty",
            "set a non-empty name under [tools.<name>]",
        ),
        AixError::EmptyToolCommand { .. } => (
            "a configured tool command is empty",
            "set a non-empty command for the configured tool",
        ),
        AixError::SecretMissingEnvVar { .. }
        | AixError::SecretFileRead { .. }
        | AixError::SecretCommandFailed { .. }
        | AixError::SecretCommandSpawn { .. }
        | AixError::SecretCommandEncoding { .. } => (
            "a configured dynamic profile label could not be resolved",
            "check the configured profile label source",
        ),
        _ => (
            "config failed structural validation",
            "run `aix config validate` and correct the reported configuration issue",
        ),
    };

    CheckFailure::new(message, hint, DoctorFailureCategory::Config)
}

pub(super) fn gateway_failure(error: &AixError) -> CheckFailure {
    if let Some(status) = http_status(error) {
        return match status {
            401 | 403 => CheckFailure::new(
                format!("gateway rejected authentication (HTTP {status})"),
                "verify the API key and its gateway permissions",
                DoctorFailureCategory::Authentication,
            ),
            402 => CheckFailure::new(
                "gateway rejected the request because a budget limit was reached (HTTP 402)",
                "review the key's effective remaining budget before retrying",
                DoctorFailureCategory::Budget,
            ),
            _ => CheckFailure::new(
                format!("gateway returned HTTP {status}"),
                "verify gateway availability and endpoint configuration",
                DoctorFailureCategory::Network,
            ),
        };
    }

    if is_timeout(error) {
        CheckFailure::new(
            "gateway request timed out",
            "verify gateway availability and network connectivity",
            DoctorFailureCategory::Network,
        )
    } else {
        CheckFailure::new(
            "gateway request failed",
            "verify endpoint.base_url and network access",
            DoctorFailureCategory::Network,
        )
    }
}

pub(super) fn budget_failure(error: &AixError) -> CheckFailure {
    if let Some(status) = http_status(error) {
        return match status {
            401 | 403 => CheckFailure::new(
                format!("budget information request was rejected (HTTP {status})"),
                "verify that the API key can read its own budget information",
                DoctorFailureCategory::Authentication,
            ),
            402 => CheckFailure::new(
                "GET /v1/key/info was rejected because a budget limit was reached (HTTP 402)",
                "review the key's effective remaining budget before retrying",
                DoctorFailureCategory::Budget,
            ),
            _ => CheckFailure::new(
                format!("GET /v1/key/info returned HTTP {status}"),
                "verify the gateway's native budget information endpoint",
                DoctorFailureCategory::Network,
            ),
        };
    }

    if is_timeout(error) {
        CheckFailure::new(
            "GET /v1/key/info timed out",
            "verify gateway availability and network connectivity",
            DoctorFailureCategory::Network,
        )
    } else if matches!(error, AixError::GatewayProtocolError(_)) {
        CheckFailure::new(
            "GET /v1/key/info returned malformed budget information",
            "ensure the gateway returns the documented native budget fields",
            DoctorFailureCategory::Network,
        )
    } else {
        CheckFailure::new(
            "budget information request failed",
            "verify the gateway's native budget information endpoint",
            DoctorFailureCategory::Network,
        )
    }
}

fn http_status(error: &AixError) -> Option<u16> {
    match error {
        AixError::GatewayError { status, .. } | AixError::GatewayRequestFailed { status } => {
            Some(*status)
        }
        _ => None,
    }
}

fn is_timeout(error: &AixError) -> bool {
    matches!(error, AixError::HttpError(error) if error.is_timeout())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn gateway_error(status: u16) -> AixError {
        AixError::GatewayError {
            status,
            body: "upstream-body-private-detail".to_string(),
        }
    }

    fn assert_budget_exit(failure: &CheckFailure) {
        assert!(matches!(failure.category, DoctorFailureCategory::Budget));
        let error = AixError::DoctorChecksFailed {
            category: failure.category,
        };
        assert_eq!(error.exit_code(), 6);
        assert!(!failure.message.contains("upstream-body-private-detail"));
        assert!(!failure.hint.contains("upstream-body-private-detail"));
    }

    #[test]
    fn gateway_402_is_a_safe_budget_failure_with_exit_code_six() {
        let failure = gateway_failure(&gateway_error(402));

        assert_budget_exit(&failure);
        assert_eq!(
            failure.message,
            "gateway rejected the request because a budget limit was reached (HTTP 402)"
        );
    }

    #[test]
    fn budget_endpoint_402_is_a_safe_budget_failure_with_exit_code_six() {
        let failure = budget_failure(&gateway_error(402));

        assert_budget_exit(&failure);
        assert_eq!(
            failure.message,
            "GET /v1/key/info was rejected because a budget limit was reached (HTTP 402)"
        );
    }

    #[test]
    fn gateway_and_budget_endpoint_keep_auth_network_and_protocol_classes() {
        assert!(matches!(
            gateway_failure(&gateway_error(403)).category,
            DoctorFailureCategory::Authentication
        ));
        assert!(matches!(
            budget_failure(&gateway_error(403)).category,
            DoctorFailureCategory::Authentication
        ));
        assert!(matches!(
            gateway_failure(&gateway_error(503)).category,
            DoctorFailureCategory::Network
        ));

        let malformed = budget_failure(&AixError::GatewayProtocolError(
            "gateway returned malformed native budget information",
        ));
        assert!(matches!(malformed.category, DoctorFailureCategory::Network));
        assert!(malformed.message.contains("malformed budget information"));
    }

    #[tokio::test]
    async fn gateway_timeout_has_a_safe_network_diagnostic() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/slow"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(500)))
            .mount(&server)
            .await;

        let error = reqwest::Client::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .unwrap()
            .get(format!("{}/slow", server.uri()))
            .send()
            .await
            .unwrap_err();
        let failure = gateway_failure(&AixError::HttpError(error));

        assert_eq!(failure.message, "gateway request timed out");
        assert_eq!(
            failure.hint,
            "verify gateway availability and network connectivity"
        );
        assert!(matches!(failure.category, DoctorFailureCategory::Network));
    }
}
