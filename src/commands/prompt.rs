use crate::config;
use crate::error::AixError;
use crate::inference;
use crate::output;
use color_eyre::Result;
use serde::Serialize;
use std::path::PathBuf;

pub async fn run(
    selected_profile: Option<String>,
    config_path: Option<PathBuf>,
    json_output: bool,
    name: Option<String>,
    list: bool,
    requested_model: Option<String>,
    files: Vec<PathBuf>,
) -> Result<()> {
    let cfg = inference::load_config(config_path.as_deref())?;
    if list {
        return list_prompts(&cfg, json_output);
    }

    let Some(name) = name else {
        return Err(AixError::PromptNameRequired.into());
    };
    let preset = cfg
        .prompts
        .get(&name)
        .ok_or_else(|| AixError::PromptNotFound {
            name: name.clone(),
            available_hint: available_prompts(&cfg),
        })?;
    let messages =
        inference::collect_messages(preset.system.clone(), Some(preset.prompt.clone()), &files)?;
    let model = requested_model.as_deref().or(preset.model.as_deref());
    let result = inference::execute(&cfg, selected_profile, model, messages).await?;
    inference::print_result("prompt", json_output, result)
}

fn list_prompts(cfg: &config::Config, json_output: bool) -> Result<()> {
    let names = config::sorted_prompt_names(cfg);

    if json_output {
        let prompts: Vec<_> = names
            .iter()
            .map(|name| PromptSummary {
                name,
                model: cfg.prompts[*name].model.as_deref(),
                has_system: cfg.prompts[*name].system.is_some(),
            })
            .collect();
        output::print_json("prompt", prompts)?;
    } else {
        for name in names {
            match cfg.prompts[name].model.as_deref() {
                Some(model) => println!("{name} (model: {model})"),
                None => println!("{name}"),
            }
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct PromptSummary<'a> {
    name: &'a str,
    model: Option<&'a str>,
    has_system: bool,
}

fn available_prompts(cfg: &config::Config) -> String {
    let names = config::sorted_prompt_names(cfg);
    if names.is_empty() {
        return "  (no prompts defined)".to_string();
    }
    names
        .into_iter()
        .map(|name| format!("  {name}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_prompt_hint_is_sorted_and_empty_lists_are_actionable() {
        let cfg: config::Config = toml::from_str(
            r#"
[endpoint]
base_url = "https://example.com"
[profiles.work]
api_key = "test-key"
[prompts.z-last]
prompt = "last"
[prompts.a-first]
prompt = "first"
"#,
        )
        .unwrap();
        assert_eq!(available_prompts(&cfg), "  a-first\n  z-last");

        let empty: config::Config = toml::from_str(
            r#"
[endpoint]
base_url = "https://example.com"
[profiles.work]
api_key = "test-key"
"#,
        )
        .unwrap();
        assert_eq!(available_prompts(&empty), "  (no prompts defined)");
    }
}
