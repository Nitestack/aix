use crate::cache::{Cache, CacheSlot};
use crate::commands::env::resolve_profile;
use crate::commands::spend::report::{format_unix, CachePolicy, SpendCacheData, SpendSource};
use crate::commands::GatewayRequestOptions;
use crate::config;
use crate::error::AixError;
use crate::gateway::catalog::{CatalogClient, CatalogModel, CatalogSources, ModelCatalog};
use crate::gateway::OpenAiClient;
use crate::output;
use color_eyre::Result;
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;

pub async fn run(
    options: GatewayRequestOptions,
    config_path: Option<PathBuf>,
    json: bool,
    filter: Option<String>,
    detail: Option<CachePolicy>,
) -> Result<()> {
    let GatewayRequestOptions { selection, timeout } = options;
    let path = config::find_config_path(config_path.as_deref())?.ok_or(AixError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;
    config::load_env_files(&cfg)?;

    let profile_name = resolve_profile(selection, &cfg)?;
    let profile = cfg
        .profiles
        .get(&profile_name)
        .ok_or_else(|| AixError::ProfileNotFound {
            name: profile_name.clone(),
            available_hint: config::format_available_profiles(&cfg),
        })?;

    let base_url = config::resolve_base_url(profile, &cfg.endpoint)?;
    let api_key = profile.resolve_api_key()?;
    if let Some(policy) = detail {
        let cache = Cache::from_config(&cfg.cache);
        let (catalog, cache_data) = lookup_catalog(
            &cache,
            base_url.expose_secret(),
            api_key.expose_secret(),
            policy,
            timeout,
        )
        .await?;
        return print_detailed(catalog, cache_data, filter, json);
    }

    let client =
        OpenAiClient::with_timeout(base_url.expose_secret(), api_key.expose_secret(), timeout);
    let response = client.models().await?;
    let mut models = extract_models(response)?;

    if let Some(filter) = &filter {
        let lowercase_filter = filter.to_lowercase();
        models.retain(|model| model.id.to_lowercase().contains(&lowercase_filter));
    }

    if json {
        output::print_json("models", ModelsOutput { models, filter })?;
    } else {
        for model in models {
            println!("{}", model.id);
        }
    }

    Ok(())
}

/// Read the model catalog from the cache or the gateway; a live result is
/// written back.
async fn lookup_catalog(
    cache: &Cache,
    base_url: &str,
    api_key: &str,
    policy: CachePolicy,
    timeout: std::time::Duration,
) -> Result<(ModelCatalog, SpendCacheData), AixError> {
    if let CachePolicy::Use(freshness) = policy {
        let cached = cache.get_slot(CacheSlot::ModelCatalog, base_url, api_key, freshness);
        if let Some((data, fetched_at)) = cached {
            if let Some(catalog) = ModelCatalog::from_cache_value(&data) {
                let cache_data = SpendCacheData {
                    source: SpendSource::Cache,
                    age_seconds: Some(crate::cache::now_secs().saturating_sub(fetched_at)),
                    fetched_at: format_unix(fetched_at),
                };
                return Ok((catalog, cache_data));
            }
        }
    }

    let catalog = CatalogClient::with_timeout(base_url, api_key, timeout)
        .fetch()
        .await?;
    cache.put_slot(
        CacheSlot::ModelCatalog,
        base_url,
        api_key,
        &catalog.to_cache_value(),
    );
    let cache_data = SpendCacheData {
        source: SpendSource::Live,
        age_seconds: None,
        fetched_at: format_unix(crate::cache::now_secs()),
    };
    Ok((catalog, cache_data))
}

#[derive(Serialize)]
struct DetailedModelsOutput {
    models: Vec<CatalogModel>,
    filter: Option<String>,
    wildcard_routes: Vec<String>,
    sources: CatalogSources,
    cache: SpendCacheData,
}

fn print_detailed(
    catalog: ModelCatalog,
    cache: SpendCacheData,
    filter: Option<String>,
    json: bool,
) -> Result<()> {
    let ModelCatalog {
        mut models,
        wildcard_routes,
        sources,
        ..
    } = catalog;
    if let Some(filter) = &filter {
        let lowercase_filter = filter.to_lowercase();
        models.retain(|model| model.id.to_lowercase().contains(&lowercase_filter));
    }

    if json {
        return output::print_json(
            "models",
            DetailedModelsOutput {
                models,
                filter,
                wildcard_routes,
                sources,
                cache,
            },
        );
    }

    for model in &models {
        println!("{}", detail_line(model));
    }
    if !sources.model_info {
        eprintln!("note: this gateway has no /v2/model/info; only model IDs are known");
    }
    Ok(())
}

/// `id  mode · 1M ctx · 128K out · reasoning low..max · $4.00/$20.00 per 1M`
fn detail_line(model: &CatalogModel) -> String {
    let mut parts = Vec::new();
    if let Some(mode) = &model.mode {
        parts.push(mode.clone());
    }
    if let Some(context) = model.context_tokens {
        parts.push(format!("{} ctx", compact_tokens(context)));
    }
    if let Some(output) = model.max_output_tokens {
        parts.push(format!("{} out", compact_tokens(output)));
    }
    if let Some(reasoning) = &model.reasoning {
        parts.push(match reasoning.efforts.as_slice() {
            [] => "reasoning".to_string(),
            [only] => format!("reasoning {only}"),
            [first, .., last] => format!("reasoning {first}..{last}"),
        });
    }
    if let Some(pricing) = &model.pricing {
        if let (Some(input), Some(output)) = (pricing.input_per_million, pricing.output_per_million)
        {
            parts.push(format!("${input:.2}/${output:.2} per 1M"));
        }
    }
    if parts.is_empty() {
        model.id.clone()
    } else {
        format!("{}  {}", model.id, parts.join(" · "))
    }
}

fn compact_tokens(tokens: u64) -> String {
    match tokens {
        t if t >= 1_000_000 && t % 1_000_000 == 0 => format!("{}M", t / 1_000_000),
        t if t >= 1_000 => format!("{}K", t / 1_000),
        t => t.to_string(),
    }
}

#[derive(Serialize)]
pub(crate) struct ModelEntry {
    pub(crate) id: String,
}

#[derive(Serialize)]
struct ModelsOutput {
    models: Vec<ModelEntry>,
    filter: Option<String>,
}

pub(crate) fn extract_models(response: Value) -> Result<Vec<ModelEntry>, AixError> {
    let data =
        response
            .get("data")
            .and_then(Value::as_array)
            .ok_or(AixError::GatewayProtocolError(
                "malformed models response: missing model list",
            ))?;

    let mut models = data
        .iter()
        .map(|entry| {
            entry
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(|id| ModelEntry { id: id.to_string() })
                .ok_or(AixError::GatewayProtocolError(
                    "malformed models response: model entry has no non-empty ID",
                ))
        })
        .collect::<Result<Vec<_>, _>>()?;

    models.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    Ok(models)
}

pub(crate) fn validate_response(response: Value) -> Result<(), AixError> {
    extract_models(response).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_and_sorts_model_ids_without_preserving_other_metadata() {
        let models = extract_models(json!({
            "data": [
                { "id": "z-model", "owned_by": "private-provider" },
                { "id": "a-model", "created": 123 }
            ]
        }))
        .unwrap();

        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "a-model");
        assert_eq!(models[1].id, "z-model");
    }

    #[test]
    fn rejects_invalid_model_list_shapes() {
        for response in [
            json!({}),
            json!({ "data": {} }),
            json!({ "data": [{ "id": 42 }] }),
            json!({ "data": [{ "id": "" }] }),
        ] {
            assert!(matches!(
                extract_models(response),
                Err(AixError::GatewayProtocolError(_))
            ));
        }
    }
}
