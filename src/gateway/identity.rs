//! Non-secret identity of a gateway endpoint and key: which kind of gateway
//! aix is talking to, and a stable fingerprint that lets callers group
//! profiles sharing one credential without ever holding the key.

use crate::config::{Endpoint, Gateway, KnownGateway, KnownProvider, Provider};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Domain separator so the fingerprint cannot be confused with other hashes
/// of the same key (for example the cache's key identity).
const FINGERPRINT_DOMAIN: &str = "aix-key-fingerprint-v1";

/// Hex SHA-256 over a domain-separated `(base_url, api_key)` pair.
///
/// The base URL is normalized by trimming trailing slashes, so `https://gw/`
/// and `https://gw` fingerprint the same. The value is one-way: it identifies
/// a credential without revealing it.
pub(crate) fn key_fingerprint(base_url: &str, api_key: &str) -> String {
    let mut hasher = Sha256::new();
    for part in [FINGERPRINT_DOMAIN, base_url.trim_end_matches('/'), api_key] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DetectedFrom {
    /// `endpoint.gateway` in the aix config.
    Config,
    /// Reported by the gateway (for example the budget response's `source`).
    Response,
    /// Recognized from the base URL host.
    BaseUrl,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct GatewayIdentity {
    /// Gateway kind such as `litellm`, `aihub`, `anthropic`, or `openai`.
    pub(crate) gateway: Option<String>,
    /// Upstream provider, when the endpoint serves a single one.
    pub(crate) provider: Option<String>,
    /// Where `gateway` came from; `null` when it is unknown.
    pub(crate) detected_from: Option<DetectedFrom>,
}

/// Resolve the gateway kind: configured metadata wins, then what the gateway
/// reported, then well-known first-party API hosts.
pub(crate) fn identify(
    endpoint: &Endpoint,
    base_url: &str,
    reported_gateway: Option<&str>,
) -> GatewayIdentity {
    let host_kind = first_party_host_kind(base_url);
    let (gateway, detected_from) = if let Some(gateway) = &endpoint.gateway {
        (
            Some(gateway_name(gateway).to_owned()),
            Some(DetectedFrom::Config),
        )
    } else if let Some(reported) = reported_gateway {
        (Some(reported.to_owned()), Some(DetectedFrom::Response))
    } else if let Some(kind) = host_kind {
        (Some(kind.to_owned()), Some(DetectedFrom::BaseUrl))
    } else {
        (None, None)
    };
    let provider = endpoint
        .provider
        .as_ref()
        .map(|provider| provider_name(provider).to_owned())
        .or_else(|| host_kind.map(str::to_owned));
    GatewayIdentity {
        gateway,
        provider,
        detected_from,
    }
}

fn first_party_host_kind(base_url: &str) -> Option<&'static str> {
    let url = url::Url::parse(base_url).ok()?;
    match url.host_str()? {
        "api.anthropic.com" => Some("anthropic"),
        "api.openai.com" => Some("openai"),
        _ => None,
    }
}

pub(crate) fn gateway_name(gateway: &Gateway) -> &str {
    match gateway {
        Gateway::Known(KnownGateway::Litellm) => "litellm",
        Gateway::Custom(name) => name,
    }
}

pub(crate) fn provider_name(provider: &Provider) -> &str {
    match provider {
        Provider::Known(KnownProvider::LiteLlm) => "litellm",
        Provider::Custom(name) => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(gateway: Option<Gateway>, provider: Option<Provider>) -> Endpoint {
        Endpoint {
            base_url: None,
            provider,
            gateway,
        }
    }

    #[test]
    fn fingerprint_is_stable_normalized_and_does_not_contain_the_key() {
        let key = "sk-fingerprint-secret-1234";
        let first = key_fingerprint("https://gw.example.com/", key);
        assert_eq!(first, key_fingerprint("https://gw.example.com", key));
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(!first.contains("1234"));
        assert_ne!(first, key_fingerprint("https://other.example.com", key));
        assert_ne!(first, key_fingerprint("https://gw.example.com", "sk-other"));
        assert_ne!(first, crate::cache::key_identity(key));
    }

    #[test]
    fn fingerprint_parts_cannot_be_shifted_between_fields() {
        assert_ne!(
            key_fingerprint("https://a", "bc"),
            key_fingerprint("https://ab", "c")
        );
    }

    #[test]
    fn configured_gateway_wins_over_reported_and_host() {
        let identity = identify(
            &endpoint(Some(Gateway::Known(KnownGateway::Litellm)), None),
            "https://api.anthropic.com",
            Some("aihub"),
        );
        assert_eq!(identity.gateway.as_deref(), Some("litellm"));
        assert_eq!(identity.detected_from, Some(DetectedFrom::Config));
        assert_eq!(identity.provider.as_deref(), Some("anthropic"));
    }

    #[test]
    fn reported_gateway_is_used_without_config() {
        let identity = identify(
            &endpoint(None, None),
            "https://gw.example.com",
            Some("aihub"),
        );
        assert_eq!(identity.gateway.as_deref(), Some("aihub"));
        assert_eq!(identity.detected_from, Some(DetectedFrom::Response));
        assert_eq!(identity.provider, None);
    }

    #[test]
    fn first_party_hosts_are_recognized_and_unknown_hosts_stay_null() {
        let anthropic = identify(&endpoint(None, None), "https://api.anthropic.com/", None);
        assert_eq!(anthropic.gateway.as_deref(), Some("anthropic"));
        assert_eq!(anthropic.provider.as_deref(), Some("anthropic"));
        assert_eq!(anthropic.detected_from, Some(DetectedFrom::BaseUrl));

        let unknown = identify(&endpoint(None, None), "https://gw.example.com", None);
        assert_eq!(unknown.gateway, None);
        assert_eq!(unknown.provider, None);
        assert_eq!(unknown.detected_from, None);
    }
}
