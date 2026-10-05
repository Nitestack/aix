use super::{
    append_configured_env, validate_executable, LaunchContext, LaunchEnv, LaunchRequest,
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
        ..
    } = request;

    let LaunchContext { cfg, profile_name } = context;
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
    append_configured_env(
        &profile.env,
        tool_env_mode,
        &mut vars,
        &mut display_only_vars,
    )?;
    append_configured_env(&tool.env, tool_env_mode, &mut vars, &mut display_only_vars)?;

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
        sidecar_plan,
    })
}

pub(super) fn codex_provider_overrides(base_url: &str, access_token_env: &str) -> Vec<String> {
    // Avoid merging process-local auth/base URL settings with a provider table
    // already present in the user's Codex config.
    const PROVIDER: &str = "aix_chatgpt_plan";
    let mut args = Vec::with_capacity(14);
    let mut add_override = |key: &str, value: String| {
        args.push("-c".to_string());
        args.push(format!("{key}={value}"));
    };
    let toml_string = |value: &str| {
        serde_json::to_string(value).expect("Codex provider strings are serializable")
    };

    add_override("model_provider", toml_string(PROVIDER));
    add_override(
        &format!("model_providers.{PROVIDER}.name"),
        toml_string("ChatGPT plan via aix"),
    );
    add_override(
        &format!("model_providers.{PROVIDER}.base_url"),
        toml_string(base_url),
    );
    add_override(
        &format!("model_providers.{PROVIDER}.env_key"),
        toml_string(access_token_env),
    );
    add_override(
        &format!("model_providers.{PROVIDER}.wire_api"),
        toml_string("responses"),
    );
    add_override(
        &format!("model_providers.{PROVIDER}.requires_openai_auth"),
        "false".to_string(),
    );
    add_override(
        &format!("model_providers.{PROVIDER}.supports_websockets"),
        "false".to_string(),
    );
    args
}
