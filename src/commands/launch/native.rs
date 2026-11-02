use super::{
    append_configured_env_excluding, validate_executable, LaunchContext, LaunchEnv, LaunchRequest,
    LaunchResolution, ToolEnvMode,
};
use crate::config;
use crate::error::AixError;
use color_eyre::Result;

/// Gateway credential variables removed from native launches so an inherited
/// shell key cannot override the tool's own sign-in.
const CLEARED_CREDENTIAL_VARS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "LITELLM_API_KEY",
    "LITELLM_BASE_URL",
];

pub(super) fn resolve_tool_launch(
    request: LaunchRequest<'_>,
    context: &LaunchContext,
    profile: &config::Profile,
) -> Result<LaunchResolution> {
    let LaunchRequest {
        configured_tool_name,
        tool_env_mode,
        policy_name,
        require_litellm,
        allowed_models,
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
        return Err(AixError::NativeAuthUnsupported.into());
    }
    let restricts_models =
        !allowed_models.is_empty() || policy.is_some_and(|policy| policy.allowed_models.is_some());
    if restricts_models {
        return Err(AixError::RunPolicyEnforcementUnavailable {
            policy: policy_name.unwrap_or_default().to_string(),
            constraint: "allowed_models",
        }
        .into());
    }

    let configured_tool = configured_tool_name.and_then(|name| cfg.tools.get(name));
    if configured_tool.is_some_and(|tool| tool.local_gateway) {
        return Err(AixError::NativeAuthUnsupported.into());
    }
    let program = configured_tool
        .and_then(|tool| tool.command.as_deref())
        .or(configured_tool_name)
        .unwrap_or_default()
        .to_string();
    if matches!(tool_env_mode, ToolEnvMode::Resolve) && configured_tool_name.is_some() {
        validate_executable(&program)?;
    }

    let mut vars = vec![("AIX_PROFILE".to_string(), profile_name.clone())];
    let mut display_only_vars = Vec::new();
    append_configured_env_excluding(
        &profile.env,
        tool_env_mode,
        &mut vars,
        &mut display_only_vars,
        &[],
    )?;
    if let Some(tool) = configured_tool {
        append_configured_env_excluding(
            &tool.env,
            tool_env_mode,
            &mut vars,
            &mut display_only_vars,
            &[],
        )?;
    }

    Ok(LaunchResolution {
        env: LaunchEnv {
            vars,
            auth_vars: Vec::new(),
            display_only_vars,
            clear_vars: CLEARED_CREDENTIAL_VARS
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            remove_vars: Vec::new(),
            profile_name: profile_name.clone(),
        },
        program,
        logical_tool_name: configured_tool.and(configured_tool_name.map(str::to_string)),
        parent_gateway: None,
        allowed_models: Vec::new(),
        policy: policy.map(|policy| super::ResolvedRunPolicy {
            name: policy_name.unwrap_or_default().to_string(),
            max_budget: policy.max_budget,
            max_duration: policy.max_duration.clone(),
            tags: policy.tags.clone(),
        }),
        prepend_args: Vec::new(),
        codex_provider_overrides: Vec::new(),
        sidecar_plan: None,
        codex_config: None,
    })
}
