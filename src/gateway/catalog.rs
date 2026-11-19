//! Detailed model metadata from LiteLLM-compatible gateways.
//!
//! Model IDs come from `GET /v1/models?return_wildcard_routes=true`; limits,
//! pricing, and reasoning support come from the paginated `GET /v2/model/info`;
//! group reasoning efforts come from the optional `GET /model_group/info`. Only
//! the normalized fields below leave this module: deployment parameters
//! (`litellm_params`), which can hold upstream credentials, are read for the
//! routed model name only.

use super::transport::{GatewayTransport, TransportError};
use crate::error::AixError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

const MODELS_PATH: &str = "/v1/models";
const MODEL_INFO_PATH: &str = "/v2/model/info";
const MODEL_GROUP_INFO_PATH: &str = "/model_group/info";
const PAGE_SIZE: u64 = 100;
const MAX_PAGES: u64 = 100;
/// Bumped when the cached catalog shape changes; older records are refetched.
const CATALOG_FORMAT: u32 = 1;

/// Reasoning efforts in increasing order.
const REASONING_EFFORTS: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ModelCatalog {
    format: u32,
    pub(crate) models: Vec<CatalogModel>,
    /// Listed routes that do not name one selectable model, such as `*`,
    /// `openai/*`, or a wildcard that substitutes a different model.
    pub(crate) wildcard_routes: Vec<String>,
    pub(crate) sources: CatalogSources,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CatalogSources {
    /// `/v2/model/info` answered; without it only IDs are known.
    pub(crate) model_info: bool,
    /// `/model_group/info` answered with group reasoning efforts.
    pub(crate) model_group_info: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct CatalogModel {
    /// The ID to send as `model`.
    pub(crate) id: String,
    /// The gateway route when it differs from `id` (a passthrough wildcard
    /// such as `claude-opus-5-5*`).
    pub(crate) route: Option<String>,
    /// `chat`, `responses`, `embedding`, `image_generation`, ... as reported.
    pub(crate) mode: Option<String>,
    /// Upstream provider reported by the gateway, lowercased.
    pub(crate) provider: Option<String>,
    pub(crate) display_name: Option<String>,
    /// Number of gateway deployments behind this model; 0 without model info.
    pub(crate) deployments: usize,
    /// Smallest input limit across deployments.
    pub(crate) context_tokens: Option<u64>,
    /// Smallest output limit across deployments.
    pub(crate) max_output_tokens: Option<u64>,
    /// `supports_*` flags (prefix removed) on which every deployment agrees.
    pub(crate) capabilities: BTreeMap<String, bool>,
    pub(crate) reasoning: Option<Reasoning>,
    pub(crate) pricing: Option<Pricing>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Reasoning {
    /// Supported efforts, ordered from `none` to `max`; empty when the model
    /// reasons but does not take an effort setting.
    pub(crate) efforts: Vec<String>,
    pub(crate) default_effort: Option<String>,
}

/// USD per million tokens, present only when every deployment agrees.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Pricing {
    pub(crate) input_per_million: Option<f64>,
    pub(crate) output_per_million: Option<f64>,
    pub(crate) cache_read_per_million: Option<f64>,
    pub(crate) cache_write_per_million: Option<f64>,
}

impl ModelCatalog {
    pub(crate) fn to_cache_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub(crate) fn from_cache_value(value: &Value) -> Option<Self> {
        let catalog: Self = serde_json::from_value(value.clone()).ok()?;
        (catalog.format == CATALOG_FORMAT).then_some(catalog)
    }
}

pub(crate) struct CatalogClient {
    transport: GatewayTransport,
}

impl CatalogClient {
    pub(crate) fn with_timeout(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            transport: GatewayTransport::with_timeout(base_url, api_key, timeout),
        }
    }

    pub(crate) async fn fetch(&self) -> Result<ModelCatalog, AixError> {
        let ids = self.model_ids().await?;
        let entries = self.model_info().await?;
        let groups = match &entries {
            Some(_) => self.group_efforts().await,
            None => None,
        };
        Ok(normalize(&ids, entries.as_deref(), groups.as_ref()))
    }

    async fn model_ids(&self) -> Result<Vec<String>, AixError> {
        let body = self
            .get(
                MODELS_PATH,
                &[("return_wildcard_routes", "true".to_string())],
            )
            .await?;
        let ids = crate::commands::models::extract_models(body)?;
        Ok(ids.into_iter().map(|model| model.id).collect())
    }

    /// All `/v2/model/info` entries, or `None` when the gateway lacks the
    /// endpoint.
    async fn model_info(&self) -> Result<Option<Vec<Value>>, AixError> {
        let inconsistent = || {
            AixError::GatewayProtocolError("model info pagination is incomplete or inconsistent")
        };
        let mut entries = Vec::new();
        let mut expected_pages: Option<u64> = None;
        for page in 1..=MAX_PAGES {
            let query = [("page", page.to_string()), ("size", PAGE_SIZE.to_string())];
            let body = match self.get(MODEL_INFO_PATH, &query).await {
                Ok(body) => body,
                Err(AixError::GatewayError {
                    status: 404 | 405 | 501,
                    ..
                }) if page == 1 => return Ok(None),
                Err(error) => return Err(error),
            };
            let data = body.get("data").and_then(Value::as_array).ok_or(
                AixError::GatewayProtocolError("malformed model info response: missing data"),
            )?;
            let total_pages = match body.get("total_pages") {
                None | Some(Value::Null) => None,
                Some(value) => Some(value.as_u64().ok_or_else(inconsistent)?),
            };
            if let Some(total) = total_pages {
                let shrank = total < page && !(page == 1 && total == 0);
                let changed = expected_pages.is_some_and(|expected| expected != total);
                if total > MAX_PAGES || shrank || changed {
                    return Err(inconsistent());
                }
                expected_pages = Some(total);
            }
            entries.extend(data.iter().cloned());
            match expected_pages {
                Some(0) if !entries.is_empty() => return Err(inconsistent()),
                Some(0) => break,
                Some(total) => {
                    if data.is_empty() && (page > 1 || total > 1) {
                        return Err(inconsistent());
                    }
                    if page >= total {
                        break;
                    }
                }
                None if (data.len() as u64) < PAGE_SIZE => break,
                None => {}
            }
            if page == MAX_PAGES {
                return Err(inconsistent());
            }
        }
        Ok(Some(entries))
    }

    /// Group reasoning efforts; any failure means "not available".
    async fn group_efforts(&self) -> Option<HashMap<String, Vec<String>>> {
        let body = self.get(MODEL_GROUP_INFO_PATH, &[]).await.ok()?;
        let mut groups: HashMap<String, Vec<String>> = HashMap::new();
        for entry in body.get("data")?.as_array()? {
            let Some(record) = entry.as_object() else {
                continue;
            };
            let (Some(name), Some(efforts)) = (
                string_at(record, "model_group"),
                strings_at(record, "supported_reasoning_efforts"),
            ) else {
                continue;
            };
            groups
                .entry(name)
                .and_modify(|previous| previous.retain(|effort| efforts.contains(effort)))
                .or_insert(efforts);
        }
        Some(groups)
    }

    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value, AixError> {
        self.transport
            .get_json(path, query)
            .await
            .map_err(TransportError::into_aix_error)
    }
}

struct Deployment<'a> {
    name: String,
    routed_model: Option<String>,
    info: &'a Map<String, Value>,
}

fn parse_deployment(entry: &Value) -> Option<Deployment<'_>> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    let record = entry.as_object()?;
    Some(Deployment {
        name: string_at(record, "model_name")?,
        // Read only the routed model name; other parameters may be secret.
        routed_model: record
            .get("litellm_params")
            .and_then(Value::as_object)
            .and_then(|params| string_at(params, "model")),
        info: record
            .get("model_info")
            .and_then(Value::as_object)
            .unwrap_or_else(|| EMPTY.get_or_init(Map::new)),
    })
}

/// A model name without its provider prefix, lowercased.
fn model_name(id: &str) -> String {
    id.rsplit('/').next().unwrap_or(id).to_lowercase()
}

fn is_generic_wildcard(name: &str) -> bool {
    name == "*" || name.ends_with("/*")
}

/// `claude-opus-5-5*` names the selectable `claude-opus-5-5`; family routes
/// such as `claude-*` or `gpt-image*` do not.
fn concrete_wildcard_id(name: &str) -> Option<&str> {
    let id = name.strip_suffix('*')?;
    let selectable = !id.contains('*')
        && !is_generic_wildcard(name)
        && id.chars().last().is_some_and(|c| c.is_ascii_alphanumeric())
        && model_name(id).chars().any(|c| c.is_ascii_digit());
    selectable.then_some(id)
}

/// A wildcard route that is not a passthrough to its own concrete model.
fn is_unresolved_wildcard(deployment: &Deployment<'_>) -> bool {
    if !deployment.name.contains('*') {
        return false;
    }
    let passthrough = concrete_wildcard_id(&deployment.name).is_some()
        && deployment
            .routed_model
            .as_deref()
            .is_some_and(|model| model_name(model) == model_name(&deployment.name));
    !passthrough
}

pub(crate) fn normalize(
    listed_ids: &[String],
    entries: Option<&[Value]>,
    groups: Option<&HashMap<String, Vec<String>>>,
) -> ModelCatalog {
    let mut by_id: BTreeMap<String, Vec<Deployment<'_>>> = BTreeMap::new();
    let mut unresolved = BTreeSet::new();
    for deployment in entries
        .unwrap_or_default()
        .iter()
        .filter_map(parse_deployment)
    {
        if is_unresolved_wildcard(&deployment) {
            unresolved.insert(deployment.name.clone());
            continue;
        }
        let id = concrete_wildcard_id(&deployment.name)
            .unwrap_or(&deployment.name)
            .to_string();
        by_id.entry(id).or_default().push(deployment);
    }

    let mut models = Vec::new();
    let mut wildcard_routes = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for listed in listed_ids {
        let resolved = by_id
            .get_key_value(concrete_wildcard_id(listed).unwrap_or(listed))
            .filter(|(_, deployments)| deployments.iter().any(|d| &d.name == listed));
        let (id, deployments) = match resolved {
            Some((id, deployments)) => (id.clone(), deployments.as_slice()),
            None if listed.contains('*') || unresolved.contains(listed) => {
                wildcard_routes.insert(listed.clone());
                continue;
            }
            None => match by_id.get(listed) {
                Some(deployments) => (listed.clone(), deployments.as_slice()),
                None => (listed.clone(), &[][..]),
            },
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        let group_efforts = groups.and_then(|groups| {
            groups
                .get(&id)
                .or_else(|| (listed != &id).then(|| groups.get(listed)).flatten())
        });
        models.push(to_model(id, listed, deployments, group_efforts));
    }
    models.sort_by(|left, right| left.id.cmp(&right.id));

    ModelCatalog {
        format: CATALOG_FORMAT,
        models,
        wildcard_routes: wildcard_routes.into_iter().collect(),
        sources: CatalogSources {
            model_info: entries.is_some(),
            model_group_info: groups.is_some(),
        },
    }
}

fn to_model(
    id: String,
    listed: &str,
    deployments: &[Deployment<'_>],
    group_efforts: Option<&Vec<String>>,
) -> CatalogModel {
    let infos: Vec<&Map<String, Value>> = deployments.iter().map(|d| d.info).collect();
    CatalogModel {
        route: (listed != id).then(|| listed.to_string()),
        mode: agreed(
            infos
                .iter()
                .map(|info| string_at(info, "mode").map(|m| m.to_lowercase())),
        ),
        provider: agreed(deployments.iter().map(provider_of)),
        display_name: agreed(infos.iter().map(|info| string_at(info, "display_name"))),
        deployments: deployments.len(),
        context_tokens: smallest(
            infos
                .iter()
                .map(|info| positive_at(info, "max_input_tokens")),
        ),
        max_output_tokens: smallest(
            infos
                .iter()
                .map(|info| positive_at(info, "max_output_tokens")),
        ),
        capabilities: capabilities_of(&infos),
        reasoning: reasoning_of_all(&infos, group_efforts),
        pricing: pricing_of(&infos),
        id,
    }
}

fn provider_of(deployment: &Deployment<'_>) -> Option<String> {
    string_at(deployment.info, "litellm_provider")
        .or_else(|| {
            deployment
                .routed_model
                .as_deref()
                .and_then(|model| model.split_once('/'))
                .map(|(provider, _)| provider.to_string())
        })
        .map(|provider| provider.to_lowercase())
}

fn capabilities_of(infos: &[&Map<String, Value>]) -> BTreeMap<String, bool> {
    let Some(first) = infos.first() else {
        return BTreeMap::new();
    };
    first
        .iter()
        .filter_map(|(key, value)| {
            let name = key.strip_prefix("supports_")?;
            if name == "reasoning" || name.ends_with("_reasoning_effort") {
                return None;
            }
            let value = value.as_bool()?;
            infos
                .iter()
                .all(|info| info.get(key).and_then(Value::as_bool) == Some(value))
                .then(|| (name.to_string(), value))
        })
        .collect()
}

fn ordered_efforts(values: &[String]) -> Vec<String> {
    REASONING_EFFORTS
        .iter()
        .filter(|effort| values.iter().any(|value| value == *effort))
        .map(|effort| effort.to_string())
        .collect()
}

/// One deployment's reasoning support.
fn reasoning_of(info: &Map<String, Value>) -> Option<Vec<String>> {
    let supports = info.get("supports_reasoning").and_then(Value::as_bool);
    if supports == Some(false) {
        return None;
    }
    if let Some(explicit) = strings_at(info, "reasoning_effort_levels") {
        return Some(ordered_efforts(&explicit));
    }
    let flags: Vec<(&str, Option<bool>)> = REASONING_EFFORTS
        .iter()
        .map(|effort| {
            let key = format!("supports_{effort}_reasoning_effort");
            (*effort, info.get(&key).and_then(Value::as_bool))
        })
        .collect();
    if flags.iter().all(|(_, flag)| flag.is_none()) {
        return (supports == Some(true)).then(Vec::new);
    }
    Some(
        flags
            .into_iter()
            .filter(|(_, flag)| *flag == Some(true))
            .map(|(effort, _)| effort.to_string())
            .collect(),
    )
}

fn reasoning_of_all(
    infos: &[&Map<String, Value>],
    group_efforts: Option<&Vec<String>>,
) -> Option<Reasoning> {
    let each: Vec<Option<Vec<String>>> = infos.iter().map(|info| reasoning_of(info)).collect();
    let default = agreed(
        infos
            .iter()
            .map(|info| string_at(info, "default_reasoning_effort")),
    );
    let efforts = if let Some(group) = group_efforts {
        let efforts = ordered_efforts(group);
        if efforts.is_empty() && each.iter().all(Option::is_none) {
            return None;
        }
        efforts
    } else {
        if each.is_empty() || each.iter().any(Option::is_none) {
            return None;
        }
        REASONING_EFFORTS
            .iter()
            .filter(|effort| {
                each.iter()
                    .flatten()
                    .all(|efforts| efforts.iter().any(|value| value == *effort))
            })
            .map(|effort| effort.to_string())
            .collect()
    };
    let default_effort = default.filter(|effort| efforts.contains(effort));
    Some(Reasoning {
        efforts,
        default_effort,
    })
}

fn pricing_of(infos: &[&Map<String, Value>]) -> Option<Pricing> {
    if infos.is_empty() {
        return None;
    }
    let price = |field: &str| {
        let first = per_million(infos[0], field);
        infos
            .iter()
            .all(|info| per_million(info, field) == first)
            .then_some(first)
            .flatten()
    };
    let pricing = Pricing {
        input_per_million: price("input_cost_per_token"),
        output_per_million: price("output_cost_per_token"),
        cache_read_per_million: price("cache_read_input_token_cost"),
        cache_write_per_million: price("cache_creation_input_token_cost"),
    };
    let any = pricing.input_per_million.is_some()
        || pricing.output_per_million.is_some()
        || pricing.cache_read_per_million.is_some()
        || pricing.cache_write_per_million.is_some();
    any.then_some(pricing)
}

/// USD per token as USD per million tokens, rounded to 6 decimal places to
/// remove binary floating-point noise.
fn per_million(info: &Map<String, Value>, field: &str) -> Option<f64> {
    let value = info
        .get(field)?
        .as_f64()
        .filter(|v| v.is_finite() && *v >= 0.0)?;
    let converted = value * 1_000_000.0;
    let rounded = (converted * 1_000_000.0).round() / 1_000_000.0;
    Some(if rounded.is_finite() {
        rounded
    } else {
        converted
    })
}

fn agreed(values: impl Iterator<Item = Option<String>>) -> Option<String> {
    let mut values = values.peekable();
    let first = values.next()??;
    values
        .all(|value| value.as_ref() == Some(&first))
        .then_some(first)
}

fn smallest(values: impl Iterator<Item = Option<u64>>) -> Option<u64> {
    values.collect::<Option<Vec<_>>>()?.into_iter().min()
}

fn string_at(record: &Map<String, Value>, key: &str) -> Option<String> {
    let value = record.get(key)?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn strings_at(record: &Map<String, Value>, key: &str) -> Option<Vec<String>> {
    record
        .get(key)?
        .as_array()?
        .iter()
        .map(|value| value.as_str().map(str::to_string))
        .collect()
}

fn positive_at(record: &Map<String, Value>, key: &str) -> Option<u64> {
    record.get(key)?.as_u64().filter(|value| *value > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|id| id.to_string()).collect()
    }

    fn deployment(name: &str, model: &str, info: Value) -> Value {
        json!({
            "model_name": name,
            "litellm_params": { "model": model, "api_key": "upstream-secret" },
            "model_info": info
        })
    }

    #[test]
    fn passthrough_wildcards_become_selectable_ids() {
        let entries = [
            deployment(
                "claude-opus-5-5*",
                "claude-opus-5-5*",
                json!({ "mode": "chat", "litellm_provider": "Google", "max_input_tokens": 1000000 }),
            ),
            deployment("claude-*", "claude-opus-5-5", json!({ "mode": "chat" })),
            deployment("openai/*", "openai/*", json!({})),
        ];
        let catalog = normalize(
            &ids(&["claude-opus-5-5*", "claude-*", "openai/*"]),
            Some(&entries),
            None,
        );

        assert_eq!(catalog.models.len(), 1);
        let model = &catalog.models[0];
        assert_eq!(model.id, "claude-opus-5-5");
        assert_eq!(model.route.as_deref(), Some("claude-opus-5-5*"));
        assert_eq!(model.provider.as_deref(), Some("google"));
        assert_eq!(model.context_tokens, Some(1_000_000));
        assert_eq!(catalog.wildcard_routes, ids(&["claude-*", "openai/*"]));
    }

    #[test]
    fn a_wildcard_that_substitutes_another_model_is_not_selectable() {
        let entries = [deployment(
            "claude-opus-5-5*",
            "anthropic/claude-sonnet-5",
            json!({ "mode": "chat" }),
        )];
        let catalog = normalize(&ids(&["claude-opus-5-5*"]), Some(&entries), None);
        assert!(catalog.models.is_empty());
        assert_eq!(catalog.wildcard_routes, ids(&["claude-opus-5-5*"]));
    }

    #[test]
    fn deployments_merge_conservatively() {
        let entries = [
            deployment(
                "gpt-6-sol",
                "azure/gpt-6-sol",
                json!({
                    "mode": "chat",
                    "litellm_provider": "Azure",
                    "max_input_tokens": 922000,
                    "max_output_tokens": 128000,
                    "input_cost_per_token": 0.0000024,
                    "output_cost_per_token": 0.000012,
                    "supports_vision": true,
                    "supports_pdf_input": true,
                    "reasoning_effort_levels": ["max", "low", "none", "high"],
                    "default_reasoning_effort": "high"
                }),
            ),
            deployment(
                "gpt-6-sol",
                "azure/gpt-6-sol-eu",
                json!({
                    "mode": "chat",
                    "litellm_provider": "Azure",
                    "max_input_tokens": 400000,
                    "max_output_tokens": 128000,
                    "input_cost_per_token": 0.0000024,
                    "output_cost_per_token": 0.000013,
                    "supports_vision": true,
                    "supports_pdf_input": false,
                    "supports_none_reasoning_effort": false,
                    "supports_low_reasoning_effort": true,
                    "supports_high_reasoning_effort": true,
                    "default_reasoning_effort": "high"
                }),
            ),
        ];
        let catalog = normalize(&ids(&["gpt-6-sol"]), Some(&entries), None);
        let model = &catalog.models[0];

        assert_eq!(model.deployments, 2);
        assert_eq!(model.mode.as_deref(), Some("chat"));
        assert_eq!(model.context_tokens, Some(400_000));
        assert_eq!(model.max_output_tokens, Some(128_000));
        assert_eq!(
            model.capabilities,
            BTreeMap::from([("vision".to_string(), true)])
        );
        let pricing = model.pricing.as_ref().unwrap();
        assert_eq!(pricing.input_per_million, Some(2.4));
        assert_eq!(pricing.output_per_million, None);
        let reasoning = model.reasoning.as_ref().unwrap();
        assert_eq!(reasoning.efforts, ids(&["low", "high"]));
        assert_eq!(reasoning.default_effort.as_deref(), Some("high"));
    }

    #[test]
    fn group_efforts_take_precedence() {
        let entries = [deployment(
            "m-1",
            "m-1",
            json!({ "supports_reasoning": true }),
        )];
        let groups = HashMap::from([("m-1".to_string(), ids(&["high", "low"]))]);
        let catalog = normalize(&ids(&["m-1"]), Some(&entries), Some(&groups));
        assert_eq!(
            catalog.models[0].reasoning.as_ref().unwrap().efforts,
            ids(&["low", "high"])
        );
        assert!(catalog.sources.model_group_info);
    }

    #[test]
    fn listed_models_without_info_are_kept_and_unlisted_info_is_dropped() {
        let entries = [deployment("hidden", "hidden", json!({ "mode": "chat" }))];
        let catalog = normalize(&ids(&["plain"]), Some(&entries), None);
        assert_eq!(catalog.models.len(), 1);
        assert_eq!(catalog.models[0].id, "plain");
        assert_eq!(catalog.models[0].deployments, 0);
        assert!(catalog.models[0].pricing.is_none());
    }

    #[test]
    fn non_reasoning_models_report_no_reasoning() {
        let entries = [deployment(
            "embed-1",
            "embed-1",
            json!({ "mode": "embedding", "supports_reasoning": false }),
        )];
        let catalog = normalize(&ids(&["embed-1"]), Some(&entries), None);
        assert!(catalog.models[0].reasoning.is_none());
        assert_eq!(catalog.models[0].mode.as_deref(), Some("embedding"));
    }

    #[test]
    fn cache_round_trip_and_secrets_are_not_kept() {
        let entries = [deployment("m-1", "m-1", json!({ "mode": "chat" }))];
        let catalog = normalize(&ids(&["m-1"]), Some(&entries), None);
        let value = catalog.to_cache_value();
        assert!(!value.to_string().contains("upstream-secret"));
        assert_eq!(ModelCatalog::from_cache_value(&value), Some(catalog));

        let mut old = value;
        old["format"] = json!(0);
        assert_eq!(ModelCatalog::from_cache_value(&old), None);
    }
}
