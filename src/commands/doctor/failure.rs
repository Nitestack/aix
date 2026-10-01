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
        return if is_auth_status(status) {
            CheckFailure::new(
                format!("gateway rejected authentication (HTTP {status})"),
                "verify the API key and its gateway permissions",
                DoctorFailureCategory::Authentication,
            )
        } else {
            CheckFailure::new(
                format!("gateway returned HTTP {status}"),
                "verify gateway availability and endpoint configuration",
                DoctorFailureCategory::Network,
            )
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

pub(super) fn admin_failure(error: &AixError) -> CheckFailure {
    if let Some(status) = http_status(error) {
        return match status {
            401 | 403 => CheckFailure::new(
                format!("LiteLLM management request was rejected (HTTP {status})"),
                "verify that the key is authorized for LiteLLM management endpoints",
                DoctorFailureCategory::Authentication,
            ),
            404 | 405 | 501 => CheckFailure::new(
                "LiteLLM management endpoint is unavailable",
                "verify the gateway exposes the LiteLLM /key/info endpoint",
                DoctorFailureCategory::Network,
            ),
            _ => CheckFailure::new(
                format!("LiteLLM management request returned HTTP {status}"),
                "verify LiteLLM management endpoint availability",
                DoctorFailureCategory::Network,
            ),
        };
    }

    if is_timeout(error) {
        CheckFailure::new(
            "LiteLLM management request timed out",
            "verify gateway availability and network connectivity",
            DoctorFailureCategory::Network,
        )
    } else {
        CheckFailure::new(
            "LiteLLM management request failed",
            "verify the gateway exposes LiteLLM management endpoints",
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

fn is_auth_status(status: u16) -> bool {
    matches!(status, 401 | 403)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
