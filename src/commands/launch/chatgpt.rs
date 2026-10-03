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
        ..
    } = request;
    if require_litellm || policy_name.is_some() {
        return Err(AixError::ChatGptRunLeaseUnsupported.into());
    }

    let LaunchContext { cfg, profile_name } = context;
    let tool_name = configured_tool_name.unwrap_or_default();
    let tool = cfg
        .tools
        .get(tool_name)
        .ok_or_else(|| AixError::ChatGptToolNotConfigured {
            tool: tool_name.to_string(),
        })?;
    let binding = tool
        .chatgpt
        .as_ref()
        .ok_or_else(|| AixError::ChatGptToolNotConfigured {
            tool: tool_name.to_string(),
        })?;
    let program = tool.command.as_deref().unwrap_or(tool_name).to_string();

    let mut auth_vars = Vec::new();
    let mut display_only_vars = Vec::new();
    if matches!(tool_env_mode, ToolEnvMode::NamesOnly) {
        display_only_vars.push(binding.access_token_env.clone());
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
    clear_vars.sort_unstable();
    clear_vars.dedup();

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
        allowed_models: Vec::new(),
        policy: None,
        prepend_args: binding.prepend_args.clone(),
    })
}
