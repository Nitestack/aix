use crate::commands::env::resolve_profile;
use crate::config;
use crate::error::AixError;
use crate::gateway::openai::OpenAiClient;
use crate::output;
use color_eyre::Result;
use serde::Serialize;
use serde_json::{json, Value};
use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;

#[derive(Serialize)]
struct AskOutput {
    model: String,
    content: String,
    usage: Usage,
}

#[derive(Serialize)]
struct Usage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

impl Usage {
    fn from_response(response: &Value) -> Self {
        let usage = response.get("usage");
        Self {
            prompt_tokens: usage
                .and_then(|usage| usage.get("prompt_tokens"))
                .and_then(Value::as_u64),
            completion_tokens: usage
                .and_then(|usage| usage.get("completion_tokens"))
                .and_then(Value::as_u64),
            total_tokens: usage
                .and_then(|usage| usage.get("total_tokens"))
                .and_then(Value::as_u64),
        }
    }
}

pub async fn run(
    selected_profile: Option<String>,
    config_path: Option<PathBuf>,
    json_output: bool,
    requested_model: Option<String>,
    system: Option<String>,
    files: Vec<PathBuf>,
    prompt: Option<String>,
) -> Result<()> {
    let messages = collect_messages(system, prompt, &files)?;

    let path = config::find_config_path(config_path.as_deref())?.ok_or(AixError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;
    config::load_env_files(&cfg)?;

    let profile_name = resolve_profile(selected_profile, &cfg)?;
    let profile = cfg
        .profiles
        .get(&profile_name)
        .ok_or_else(|| AixError::ProfileNotFound {
            name: profile_name.clone(),
            available_hint: config::format_available_profiles(&cfg),
        })?;
    let resolved_model = config::resolve_model(requested_model.as_deref(), &cfg, profile)?;
    let base_url = config::resolve_base_url(profile, &cfg.endpoint)?;
    let api_key = profile.api_key.resolve()?;

    let request = json!({ "model": resolved_model.clone(), "messages": messages });
    let client = OpenAiClient::new(base_url.expose_secret(), api_key.expose_secret());
    let response = client
        .chat_completions(&request)
        .await
        .map_err(|error| match error {
            AixError::GatewayError { status, .. } => AixError::GatewayRequestFailed { status },
            error => error,
        })?;

    let content = assistant_content(&response)?;
    let result = AskOutput {
        model: resolved_model,
        content,
        usage: Usage::from_response(&response),
    };

    if json_output {
        output::print_json("ask", result)?;
    } else {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(result.content.as_bytes())?;
        if !result.content.ends_with('\n') {
            stdout.write_all(b"\n")?;
        }
    }
    Ok(())
}

fn collect_messages(
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

fn validate_input_shape(
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
    fn missing_prompt_on_terminal_fails_with_usage_guidance() {
        assert!(matches!(
            validate_input_shape(false, false, true),
            Err(AixError::AskInputRequired)
        ));
    }

    #[test]
    fn files_without_prompt_require_an_instruction_even_if_stdin_is_piped() {
        assert!(matches!(
            validate_input_shape(false, true, false),
            Err(AixError::AskInstructionRequired)
        ));
    }
}
