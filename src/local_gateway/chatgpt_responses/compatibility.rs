use serde_json::Value;

pub(crate) const UNSUPPORTED_RESPONSE_FIELDS: &[&str] = &[
    "background",
    "conversation",
    "max_output_tokens",
    "max_tool_calls",
    "metadata",
    "moderation",
    "multi_agent",
    "prompt",
    "prompt_cache_retention",
    "safety_identifier",
    "temperature",
    "top_logprobs",
    "top_p",
    "truncation",
    "user",
    "previous_response_id",
];

const UNSUPPORTED_HOSTED_TOOLS: &[&str] = &[
    "code_interpreter",
    "file_search",
    "computer",
    "computer_use_preview",
    "image_generation",
    "mcp",
    "tool_search",
    "programmatic_tool_calling",
];

const UNSUPPORTED_INPUT_ITEM_TYPES: &[&str] = &[
    "code_interpreter_call",
    "computer_call",
    "computer_call_output",
    "file_search_call",
    "file_search_call_output",
    "image_generation_call",
    "mcp_call",
    "tool_search_call",
];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum CompatibilityError {
    #[error("Responses request must be a JSON object")]
    InvalidRequest,
    #[error("system input must contain only text content")]
    InvalidSystemContent,
    #[error("Responses tools must use supported function/custom or web search forms")]
    UnsupportedTool,
}

pub(crate) fn normalize_response_request(request: &mut Value) -> Result<Value, CompatibilityError> {
    let object = request
        .as_object_mut()
        .ok_or(CompatibilityError::InvalidRequest)?;

    for field in UNSUPPORTED_RESPONSE_FIELDS {
        object.remove(*field);
    }

    if let Some(tools) = object.get("tools") {
        validate_tool_list(tools)?;
    }
    if let Some(tools) = object.get("additional_tools") {
        validate_tool_list(tools)?;
    }

    let mut system_texts = Vec::new();
    if let Some(input) = object.get_mut("input") {
        if let Some(items) = input.as_array_mut() {
            let mut retained = Vec::with_capacity(items.len());
            for item in items.drain(..) {
                if item
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| UNSUPPORTED_INPUT_ITEM_TYPES.contains(&kind))
                {
                    return Err(CompatibilityError::UnsupportedTool);
                }
                if item.get("role").and_then(Value::as_str) == Some("system") {
                    let content = item
                        .get("content")
                        .ok_or(CompatibilityError::InvalidSystemContent)?;
                    system_texts.push(extract_system_text(content)?);
                } else {
                    retained.push(item);
                }
            }
            *items = retained;
        }
    }

    if !system_texts.is_empty() {
        let system_context = system_texts.join("\n\n");
        let existing = object
            .remove("instructions")
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or(CompatibilityError::InvalidSystemContent)
            })
            .transpose()?;
        object.insert(
            "instructions".to_string(),
            Value::String(match existing.filter(|text| !text.is_empty()) {
                Some(existing) => format!("{existing}\n\n{system_context}"),
                None => system_context,
            }),
        );
    }

    object.insert("store".to_string(), Value::Bool(false));
    object.insert("stream".to_string(), Value::Bool(true));
    Ok(request.clone())
}

fn validate_tool_list(tools: &Value) -> Result<(), CompatibilityError> {
    let tools = tools
        .as_array()
        .ok_or(CompatibilityError::UnsupportedTool)?;
    for tool in tools {
        let kind = tool
            .get("type")
            .and_then(Value::as_str)
            .ok_or(CompatibilityError::UnsupportedTool)?;
        if UNSUPPORTED_HOSTED_TOOLS.contains(&kind)
            || !matches!(
                kind,
                "function" | "custom" | "web_search" | "web_search_preview"
            )
        {
            return Err(CompatibilityError::UnsupportedTool);
        }
    }
    Ok(())
}

fn extract_system_text(content: &Value) -> Result<String, CompatibilityError> {
    if let Some(text) = content.as_str() {
        return Ok(text.to_owned());
    }
    let parts = content
        .as_array()
        .ok_or(CompatibilityError::InvalidSystemContent)?;
    let text = parts
        .iter()
        .map(|part| {
            let kind = part.get("type").and_then(Value::as_str);
            if !matches!(kind, None | Some("text") | Some("input_text")) {
                return Err(CompatibilityError::InvalidSystemContent);
            }
            part.get("text")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or(CompatibilityError::InvalidSystemContent)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(text.join("\n"))
}
