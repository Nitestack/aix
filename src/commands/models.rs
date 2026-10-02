use crate::commands::env::resolve_profile;
use crate::commands::GatewayRequestOptions;
use crate::config;
use crate::error::AixError;
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
    let api_key = profile.api_key.resolve()?;
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

#[derive(Serialize)]
struct ModelEntry {
    id: String,
}

#[derive(Serialize)]
struct ModelsOutput {
    models: Vec<ModelEntry>,
    filter: Option<String>,
}

fn extract_models(response: Value) -> Result<Vec<ModelEntry>, AixError> {
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
