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
    let uses_opencode_bridge = tool_name == "opencode";
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
    let resolved_allowed_models = super::resolve_allowed_models(
        policy.and_then(|policy| policy.allowed_models.as_ref()),
        allowed_models.to_vec(),
        cfg,
        profile,
    )?;
    if !resolved_allowed_models.is_empty() && !uses_opencode_bridge {
        return Err(AixError::RunPolicyEnforcementUnavailable {
            policy: policy_name.unwrap_or_default().to_string(),
            constraint: "allowed_models",
        }
        .into());
    }

    let mut auth_vars = Vec::new();
    let mut display_only_vars = Vec::new();
    let mut sidecar_plan = None;
    if matches!(tool_env_mode, ToolEnvMode::NamesOnly) {
        if uses_opencode_bridge {
            display_only_vars.push(super::opencode::BRIDGE_TOKEN_ENV.to_string());
            display_only_vars.push(super::opencode::OPENCODE_CONFIG_ENV.to_string());
            sidecar_plan = Some(super::LaunchSidecarPlan::OpenCodeSiwc);
        } else {
            display_only_vars.push(binding.access_token_env.clone());
        }
    } else if uses_opencode_bridge {
        validate_executable(&program)?;
        sidecar_plan = Some(super::LaunchSidecarPlan::OpenCodeSiwc);
    } else {
        let service = crate::auth::AuthService::new(timeout)?;
        let access_token = service.access_token(profile_name).await?;
        auth_vars.push((binding.access_token_env.clone(), access_token));
        validate_executable(&program)?;
    }

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
    if uses_opencode_bridge {
        clear_vars.push(binding.access_token_env.clone());
    }
    clear_vars.sort_unstable();
    clear_vars.dedup();

    if uses_opencode_bridge {
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
