use crate::commands::env::resolve_profile;
use crate::commands::GatewayRequestOptions;
use crate::config;
use crate::error::AixError;
use crate::gateway::openai::OpenAiClient;
use crate::output;
use color_eyre::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

#[derive(Serialize)]
pub(crate) struct InferenceOutput {
    model: String,
    content: String,
    usage: Usage,
}

#[derive(Default, Deserialize, Serialize)]
struct Usage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

impl Usage {
    fn from_response(response: &Value) -> Self {
        response
            .get("usage")
            .cloned()
            .and_then(|usage| serde_json::from_value(usage).ok())
            .unwrap_or_default()
    }
}

pub(crate) fn load_config(config_path: Option<&Path>) -> Result<config::Config> {
    let path = config::find_config_path(config_path)?.ok_or(AixError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;
    Ok(cfg)
}

pub(crate) fn prepare_request_options(
    cfg: &config::Config,
    mut options: GatewayRequestOptions,
) -> Result<GatewayRequestOptions> {
    let profile_name = resolve_profile(options.selection.clone(), cfg)?;
    if !cfg.profiles.contains_key(&profile_name) {
        return Err(AixError::ProfileNotFound {
            name: profile_name.clone(),
            available_hint: config::format_available_profiles(cfg),
        }
        .into());
    }
    options.selection.profile = Some(profile_name);
    Ok(options)
}

pub(crate) async fn execute(
    cfg: &config::Config,
    options: GatewayRequestOptions,
    requested_model: Option<&str>,
    messages: Vec<Value>,
) -> Result<InferenceOutput> {
    config::load_env_files(cfg)?;
    let GatewayRequestOptions { selection, timeout } = options;
    let profile_name = resolve_profile(selection, cfg)?;
    let profile = cfg
        .profiles
        .get(&profile_name)
        .ok_or_else(|| AixError::ProfileNotFound {
            name: profile_name.clone(),
            available_hint: config::format_available_profiles(cfg),
        })?;
    let resolved_model = config::resolve_model(requested_model, cfg, profile)?;
    let base_url = config::resolve_base_url(profile, &cfg.endpoint)?;
    let api_key = profile.api_key.resolve()?;

    let request = json!({ "model": resolved_model.clone(), "messages": messages });
    let client =
        OpenAiClient::with_timeout(base_url.expose_secret(), api_key.expose_secret(), timeout);
    let response = client
        .chat_completions(&request)
        .await
        .map_err(|error| match error {
            AixError::GatewayError { status, .. } => AixError::GatewayRequestFailed { status },
            error => error,
        })?;

    Ok(InferenceOutput {
        model: resolved_model,
        content: assistant_content(&response)?,
        usage: Usage::from_response(&response),
    })
}

pub(crate) fn print_result(
    command: &str,
    json_output: bool,
    result: InferenceOutput,
) -> Result<()> {
    if json_output {
        output::print_json(command, result)?;
    } else {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(result.content.as_bytes())?;
        if !result.content.ends_with('\n') {
            stdout.write_all(b"\n")?;
        }
    }
    Ok(())
}

pub(crate) fn collect_messages(
    system: Option<String>,
    prompt: Option<String>,
    files: &[PathBuf],
) -> Result<Vec<Value>, AixError> {
    let stdin = std::io::stdin();
    let stdin_is_terminal = stdin.is_terminal();
    validate_input_shape(prompt.is_some(), !files.is_empty(), stdin_is_terminal)?;

    let stdin_context = if stdin_is_terminal {
        None
    } else {
        let mut contents = String::new();
        stdin
            .lock()
            .read_to_string(&mut contents)
            .map_err(|source| AixError::AskStdinRead { source })?;
        (!contents.is_empty()).then_some(contents)
    };

    if prompt.is_none() && stdin_context.is_none() {
        return Err(AixError::AskInputRequired);
    }

    let mut messages = Vec::new();
    if let Some(system) = system {
        messages.push(json!({ "role": "system", "content": system }));
    }
    if let Some(prompt) = prompt {
        messages.push(json!({ "role": "user", "content": prompt }));
        if let Some(context) = stdin_context {
            messages.push(json!({
                "role": "user",
                "content": format!("Context from stdin:\n{context}")
            }));
        }
    } else if let Some(context) = stdin_context {
        messages.push(json!({ "role": "user", "content": context }));
    }
    for path in files {
        let contents = std::fs::read_to_string(path).map_err(|source| AixError::AskFileRead {
            path: path.clone(),
            source,
        })?;
        let name = path.display();
        messages.push(json!({
            "role": "user",
            "content": format!("--- Begin file: {name} ---\n{contents}\n--- End file: {name} ---")
        }));
    }

    Ok(messages)
}

pub(crate) fn validate_input_shape(
    has_prompt: bool,
    has_files: bool,
    stdin_is_terminal: bool,
) -> Result<(), AixError> {
    if !has_prompt && has_files {
        return Err(AixError::AskInstructionRequired);
    }
    if !has_prompt && stdin_is_terminal {
        return Err(AixError::AskInputRequired);
    }
    Ok(())
}

fn assistant_content(response: &Value) -> Result<String> {
    response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .filter(|content| !content.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            AixError::GatewayProtocolError("response contained no assistant text").into()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_instruction_on_terminal_fails_with_usage_guidance() {
        assert!(matches!(
            validate_input_shape(false, false, true),
            Err(AixError::AskInputRequired)
        ));
    }

    #[test]
    fn files_without_instruction_require_one_even_if_stdin_is_piped() {
        assert!(matches!(
            validate_input_shape(false, true, false),
            Err(AixError::AskInstructionRequired)
        ));
    }
}
