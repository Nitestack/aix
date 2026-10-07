use super::{
    append_configured_env_excluding, validate_executable, LaunchContext, LaunchEnv, LaunchRequest,
    LaunchResolution, ToolEnvMode,
};
use crate::config;
use crate::error::AixError;
use color_eyre::Result;
use std::time::Duration;

pub(super) async fn resolve_tool_launch(
    request: LaunchRequest<'_>,
    context: &LaunchContext,
    profile: &config::Profile,
    timeout: Duration,
) -> Result<LaunchResolution> {
    let LaunchRequest {
        configured_tool_name,
        tool_env_mode,
        policy_name,
        require_litellm,
        allowed_models,
        codex_config,
        ..
    } = request;

    let LaunchContext {
        cfg, profile_name, ..
    } = context;
    let policy = super::resolve_run_policy(cfg, policy_name)?;
    if policy.is_some_and(|policy| policy.max_budget.is_some()) {
        return Err(AixError::RunPolicyBudgetUnavailable.into());
    }
    if require_litellm {
        return Err(AixError::ChatGptRunLeaseUnsupported.into());
    }

    let tool_name = configured_tool_name.unwrap_or_default();
    let tool = cfg
        .tools
        .get(tool_name)
        .ok_or_else(|| AixError::ChatGptToolNotConfigured {
            tool: tool_name.to_string(),
        })?;
    let responses_adapter = tool.chatgpt_responses_adapter(tool_name);
    let uses_opencode_bridge = responses_adapter == Some(config::ChatGptResponsesAdapter::OpenCode);
    if tool.local_gateway && !uses_opencode_bridge {
        return Err(AixError::ChatGptAuthUnsupported.into());
    }
    let binding = tool
        .chatgpt
        .as_ref()
        .ok_or_else(|| AixError::ChatGptToolNotConfigured {
            tool: tool_name.to_string(),
        })?;
    if binding.access_token_env == "CODEX_HOME"
        && codex_config
            .as_ref()
            .is_some_and(|selection| selection.is_active())
    {
        return Err(AixError::CodexConfigEnvironmentConflict.into());
    }
    let program = tool.command.as_deref().unwrap_or(tool_name).to_string();
    let uses_responses_gateway = responses_adapter.is_some();

    let resolved_allowed_models = super::resolve_allowed_models(
        policy.and_then(|policy| policy.allowed_models.as_ref()),
        allowed_models.to_vec(),
        cfg,
        profile,
    )?;
    if !resolved_allowed_models.is_empty() && !uses_responses_gateway {
        return Err(AixError::RunPolicyEnforcementUnavailable {
            policy: policy_name.unwrap_or_default().to_string(),
            constraint: "allowed_models",
        }
        .into());
    }
    let (sidecar_plan, names_only_vars) = match responses_adapter {
        Some(config::ChatGptResponsesAdapter::OpenCode) => (
            Some(super::LaunchSidecarPlan::OpenCodeSiwc),
            vec![
                super::opencode::BRIDGE_TOKEN_ENV.to_string(),
                super::opencode::OPENCODE_CONFIG_ENV.to_string(),
            ],
        ),
        Some(config::ChatGptResponsesAdapter::CodexAppServer) => (
            Some(super::LaunchSidecarPlan::CodexChatGptResponses {
                access_token_env: binding.access_token_env.clone(),
            }),
            vec![binding.access_token_env.clone()],
        ),
        None => (None, vec![binding.access_token_env.clone()]),
    };

    let mut auth_vars = Vec::new();
    let mut display_only_vars = if matches!(tool_env_mode, ToolEnvMode::NamesOnly) {
        names_only_vars
    } else if uses_responses_gateway {
        validate_executable(&program)?;
        Vec::new()
    } else {
        let service = crate::auth::AuthService::new(timeout)?;
        let access_token = service.access_token(profile_name).await?;
        auth_vars.push((binding.access_token_env.clone(), access_token));
        validate_executable(&program)?;
        Vec::new()
    };

    let mut vars = vec![("AIX_PROFILE".to_string(), profile_name.clone())];
    let skipped_env_names: &[&str] = if codex_config
        .as_ref()
        .is_some_and(|selection| selection.is_active())
    {
        &["CODEX_HOME"]
    } else {
        &[]
    };
    append_configured_env_excluding(
        &profile.env,
        tool_env_mode,
        &mut vars,
        &mut display_only_vars,
        skipped_env_names,
    )?;
    append_configured_env_excluding(
        &tool.env,
        tool_env_mode,
        &mut vars,
        &mut display_only_vars,
        skipped_env_names,
    )?;

    let mut clear_vars = vec![
        "ANTHROPIC_API_KEY".to_string(),
        "ANTHROPIC_BASE_URL".to_string(),
        "OPENAI_API_KEY".to_string(),
        "OPENAI_BASE_URL".to_string(),
        "LITELLM_API_KEY".to_string(),
        "LITELLM_BASE_URL".to_string(),
    ];
    clear_vars.extend(binding.clear_env.iter().cloned());
    if uses_responses_gateway {
        clear_vars.push(binding.access_token_env.clone());
    }
    clear_vars.sort_unstable();
    clear_vars.dedup();

    if uses_responses_gateway {
        vars.retain(|(name, _)| {
            name != &binding.access_token_env && !clear_vars.iter().any(|clear| clear == name)
        });
    }

    Ok(LaunchResolution {
        env: LaunchEnv {
            vars,
            auth_vars,
            display_only_vars,
            clear_vars,
            remove_vars: Vec::new(),
            profile_name: profile_name.clone(),
        },
        program,
        logical_tool_name: Some(tool_name.to_string()),
        parent_gateway: None,
        allowed_models: resolved_allowed_models,
        policy: policy.map(|policy| super::ResolvedRunPolicy {
            name: policy_name.unwrap_or_default().to_string(),
            max_budget: policy.max_budget,
            max_duration: policy.max_duration.clone(),
            tags: policy.tags.clone(),
        }),
        prepend_args: binding.prepend_args.clone(),
        codex_provider_overrides: Vec::new(),
        sidecar_plan,
        codex_config: None,
    })
}

pub(super) fn codex_provider_overrides(base_url: &str, access_token_env: &str) -> Vec<String> {
    // Avoid merging process-local auth/base URL settings with a provider table
    // already present in the user's Codex config.
    codex_provider_config_overrides(
        "aix_chatgpt_plan",
        "ChatGPT plan via aix",
        base_url,
        access_token_env,
        false,
    )
}

pub(super) fn codex_api_key_provider_overrides(base_url: &str) -> Vec<String> {
    codex_provider_config_overrides(
        "aix_api_key_gateway",
        "API key via aix",
        base_url,
        "OPENAI_API_KEY",
        true,
    )
}

fn codex_provider_config_overrides(
    provider: &str,
    name: &str,
    base_url: &str,
    env_key: &str,
    requires_openai_auth: bool,
) -> Vec<String> {
    let mut args = Vec::with_capacity(14);
    let mut add_override = |key: &str, value: String| {
        args.push("-c".to_string());
        args.push(format!("{key}={value}"));
    };
    let toml_string = |value: &str| {
        serde_json::to_string(value).expect("Codex provider strings are serializable")
    };

    add_override("model_provider", toml_string(provider));
    add_override(
        &format!("model_providers.{provider}.name"),
        toml_string(name),
    );
    add_override(
        &format!("model_providers.{provider}.base_url"),
        toml_string(base_url),
    );
    add_override(
        &format!("model_providers.{provider}.env_key"),
        toml_string(env_key),
    );
    add_override(
        &format!("model_providers.{provider}.wire_api"),
        toml_string("responses"),
    );
    add_override(
        &format!("model_providers.{provider}.requires_openai_auth"),
        requires_openai_auth.to_string(),
    );
    add_override(
        &format!("model_providers.{provider}.supports_websockets"),
        "false".to_string(),
    );
    args
}
