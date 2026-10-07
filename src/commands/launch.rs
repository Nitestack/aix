use crate::commands::env::{collect_profile_vars_excluding, collect_vars, resolve_profile};
use crate::commands::ProfileSelection;
use crate::config;
use crate::error::AixError;
use crate::local_gateway::{
    ApiKeyGatewayHandle, ChatGptResponsesHandle, LaunchContext as LocalGatewayContext, ServerHandle,
};
use crate::secrets::SecretString;
use color_eyre::Result;
use directories::BaseDirs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

#[path = "launch/chatgpt.rs"]
mod chatgpt;
#[path = "launch/opencode.rs"]
mod opencode;

pub struct LaunchEnv {
    pub vars: Vec<(String, String)>,
    pub auth_vars: Vec<(String, SecretString)>,
    pub(crate) display_only_vars: Vec<String>,
    pub clear_vars: Vec<String>,
    pub remove_vars: Vec<String>,
    pub profile_name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interruption {
    CtrlC,
    Termination,
}

pub(crate) struct ChildRunOutcome {
    pub(crate) status: std::process::ExitStatus,
    pub(crate) duration_expired: bool,
}

impl Interruption {
    pub fn exit_code(self) -> i32 {
        match self {
            Self::CtrlC => 130,
            Self::Termination => 143,
        }
    }
}

pub struct ResolvedRunLaunch {
    pub env: LaunchEnv,
    pub program: String,
    pub logical_tool_name: Option<String>,
    pub parent_gateway: Option<ParentGatewayCredentials>,
    pub allowed_models: Vec<String>,
    pub policy: Option<ResolvedRunPolicy>,
    pub prepend_args: Vec<String>,
    pub codex_provider_overrides: Vec<String>,
    codex_config: Option<CodexConfigSelection>,
    sidecar_plan: Option<LaunchSidecarPlan>,
}

enum LaunchSidecarPlan {
    OpenCodeSiwc,
    CodexChatGptResponses { access_token_env: String },
    ApiKey { api_format: config::ApiFormat },
}

pub(crate) struct StartedLaunchSidecar(ServerHandle);

impl StartedLaunchSidecar {
    async fn stop(self) {
        self.0.stop().await;
    }

    pub(crate) async fn run_child<T>(self, run_child: impl FnOnce() -> T) -> T {
        let result = run_child();
        self.stop().await;
        result
    }
}

pub(crate) struct ParentGatewayCredentials {
    pub base_url: SecretString,
    pub api_key: SecretString,
}

#[derive(Clone, Debug)]
pub struct ResolvedRunPolicy {
    pub name: String,
    pub max_budget: Option<f64>,
    pub max_duration: String,
    pub tags: Vec<String>,
}

struct LaunchResolution {
    env: LaunchEnv,
    program: String,
    logical_tool_name: Option<String>,
    parent_gateway: Option<ParentGatewayCredentials>,
    allowed_models: Vec<String>,
    policy: Option<ResolvedRunPolicy>,
    prepend_args: Vec<String>,
    codex_provider_overrides: Vec<String>,
    sidecar_plan: Option<LaunchSidecarPlan>,
    codex_config: Option<CodexConfigSelection>,
}

struct LaunchContext {
    cfg: config::Config,
    profile_name: String,
    config_path: PathBuf,
}

impl LaunchContext {
    fn profile(&self) -> Result<&config::Profile, AixError> {
        self.cfg
            .profiles
            .get(&self.profile_name)
            .ok_or_else(|| AixError::ProfileNotFound {
                name: self.profile_name.clone(),
                available_hint: config::format_available_profiles(&self.cfg),
            })
    }
}

struct LaunchRequest<'a> {
    selection: ProfileSelection,
    explicit_profile: Option<String>,
    config_path: Option<PathBuf>,
    format_override: Option<config::ApiFormat>,
    configured_tool_name: Option<&'a str>,
    tool_env_mode: ToolEnvMode,
    allowed_models: &'a [String],
    policy_name: Option<&'a str>,
    require_litellm: bool,
    codex_config: Option<CodexConfigSelection>,
    native_args: &'a [String],
}

#[derive(Clone, Debug)]
struct CodexConfigSelection {
    profile_name: String,
    config_path: PathBuf,
    requested_dir: PathBuf,
    effective_dir: PathBuf,
    unavailable_reason: Option<&'static str>,
}

impl CodexConfigSelection {
    fn is_active(&self) -> bool {
        self.unavailable_reason.is_none()
    }

    fn warn_selected_state_root(&self) {
        eprintln!(
            "warning: profile '{}' tool 'codex' config_dir '{}' from aix config '{}' sets CODEX_HOME, which also selects Codex persistent state; the standard Codex home's sessions, history, and saved authentication are not automatically shared (tested with codex-cli 0.157.0; other versions are unverified)",
            self.profile_name,
            self.requested_dir.display(),
            self.config_path.display(),
        );
    }

    fn warn_fallback(&self) {
        let reason = self
            .unavailable_reason
            .expect("fallback warning requires an unavailable directory");
        eprintln!(
            "warning: profile '{}' tool 'codex' config_dir '{}' from aix config '{}' {}; using standard Codex configuration",
            self.profile_name,
            self.requested_dir.display(),
            self.config_path.display(),
            reason,
        );
    }

    fn print_dry_run(&self) {
        if self.is_active() {
            eprintln!(
                "Codex configuration: selected '{}' for profile '{}' (tool 'codex'; source '{}'); CODEX_HOME also selects persistent state; codex-cli 0.157.0 tested, other versions unverified",
                self.effective_dir.display(),
                self.profile_name,
                self.config_path.display(),
            );
        } else {
            eprintln!(
                "Codex configuration: '{}' {}; would use standard Codex configuration for profile '{}' (tool 'codex'; source '{}')",
                self.requested_dir.display(),
                self.unavailable_reason.unwrap_or("is unavailable"),
                self.profile_name,
                self.config_path.display(),
            );
        }
    }
}

#[derive(Clone, Copy)]
enum ToolEnvMode {
    Resolve,
    NamesOnly,
}

pub fn resolve_launch_env(
    selection: ProfileSelection,
    config_path: Option<PathBuf>,
    format_override: Option<config::ApiFormat>,
) -> Result<LaunchEnv> {
    let resolution = resolve_launch_env_inner(LaunchRequest {
        selection,
        explicit_profile: None,
        config_path,
        format_override,
        configured_tool_name: None,
        tool_env_mode: ToolEnvMode::Resolve,
        allowed_models: &[],
        policy_name: None,
        require_litellm: false,
        codex_config: None,
        native_args: &[],
    })?;
    Ok(resolution.env)
}

pub(crate) struct RunLaunchRequest<'a> {
    pub selection: ProfileSelection,
    pub explicit_profile: Option<String>,
    pub config_path: Option<PathBuf>,
    pub program: &'a str,
    pub allowed_models: &'a [String],
    pub policy_name: Option<&'a str>,
    pub require_litellm: bool,
    pub dry_run: bool,
    pub timeout: Duration,
    pub native_args: &'a [String],
}

pub(crate) async fn resolve_run_launch(request: RunLaunchRequest<'_>) -> Result<ResolvedRunLaunch> {
    let RunLaunchRequest {
        selection,
        explicit_profile,
        config_path,
        program,
        allowed_models,
        policy_name,
        require_litellm,
        dry_run,
        timeout,
        native_args,
    } = request;
    resolve_tool_launch(
        LaunchRequest {
            selection,
            explicit_profile,
            config_path,
            format_override: None,
            configured_tool_name: Some(program),
            tool_env_mode: if dry_run {
                ToolEnvMode::NamesOnly
            } else {
                ToolEnvMode::Resolve
            },
            allowed_models,
            policy_name,
            require_litellm,
            codex_config: None,
            native_args,
        },
        timeout,
    )
    .await
}

async fn resolve_tool_launch(
    mut request: LaunchRequest<'_>,
    timeout: Duration,
) -> Result<ResolvedRunLaunch> {
    let context = load_launch_context(&request)?;
    let profile = context.profile()?;
    let codex_config = match request.configured_tool_name {
        Some("codex") => profile.tool_configs.codex.as_ref().map(|tool_config| {
            resolve_codex_config_selection(
                &context.profile_name,
                &context.config_path,
                &tool_config.config_dir,
            )
        }),
        _ => None,
    };
    validate_codex_config_selection_args(codex_config.as_ref(), request.native_args)?;
    request.codex_config = codex_config.clone();
    let tool_env_mode = request.tool_env_mode;
    let is_chatgpt = profile.auth.is_chatgpt();
    let mut resolution = if is_chatgpt {
        chatgpt::resolve_tool_launch(request, &context, profile, timeout).await?
    } else {
        resolve_api_key_launch(request, &context, profile)?
    };
    if let Some(codex_config) = codex_config {
        if codex_config.is_active() {
            resolution.env.vars.retain(|(name, _)| name != "CODEX_HOME");
            resolution
                .env
                .display_only_vars
                .retain(|name| name != "CODEX_HOME");
            resolution.env.vars.push((
                "CODEX_HOME".to_string(),
                codex_config.effective_dir.to_string_lossy().into_owned(),
            ));
            if matches!(tool_env_mode, ToolEnvMode::Resolve) {
                codex_config.warn_selected_state_root();
            }
        } else if matches!(tool_env_mode, ToolEnvMode::Resolve) {
            codex_config.warn_fallback();
        }
        resolution.codex_config = Some(codex_config);
    }
    Ok(ResolvedRunLaunch {
        env: resolution.env,
        program: resolution.program,
        logical_tool_name: resolution.logical_tool_name,
        parent_gateway: resolution.parent_gateway,
        allowed_models: resolution.allowed_models,
        policy: resolution.policy,
        prepend_args: resolution.prepend_args,
        codex_provider_overrides: resolution.codex_provider_overrides,
        codex_config: resolution.codex_config,
        sidecar_plan: resolution.sidecar_plan,
    })
}

fn resolve_launch_env_inner(request: LaunchRequest<'_>) -> Result<LaunchResolution> {
    let context = load_launch_context(&request)?;
    let profile = context.profile()?;
    resolve_api_key_launch(request, &context, profile)
}

fn load_launch_context(request: &LaunchRequest<'_>) -> Result<LaunchContext> {
    let path =
        config::find_config_path(request.config_path.as_deref())?.ok_or(AixError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;
    config::load_env_files(&cfg)?;

    let policy = resolve_run_policy(&cfg, request.policy_name)?;
    if let Some(policy_profile) = policy.and_then(|policy| policy.profile.as_deref()) {
        if request
            .explicit_profile
            .as_deref()
            .is_some_and(|selected| selected != policy_profile)
        {
            return Err(AixError::RunPolicyProfileConflict.into());
        }
    }

    let explicit_non_litellm_gateway = matches!(
        cfg.endpoint.gateway.as_ref(),
        Some(config::Gateway::Custom(_))
    );
    let profile_name = resolve_profile(
        ProfileSelection {
            profile: policy
                .and_then(|policy| policy.profile.clone())
                .or_else(|| request.selection.profile.clone()),
            non_interactive: request.selection.non_interactive,
        },
        &cfg,
    )?;

    let profile = cfg
        .profiles
        .get(&profile_name)
        .ok_or_else(|| AixError::ProfileNotFound {
            name: profile_name.clone(),
            available_hint: config::format_available_profiles(&cfg),
        })?;
    let budget_requested = policy.is_some_and(|policy| policy.max_budget.is_some());
    if budget_requested && profile.auth.is_chatgpt() {
        return Err(AixError::RunPolicyBudgetUnavailable.into());
    }
    if (request.require_litellm || budget_requested) && explicit_non_litellm_gateway {
        return Err(AixError::LeaseNotLiteLlm.into());
    }

    Ok(LaunchContext {
        cfg,
        profile_name,
        config_path: path,
    })
}

fn resolve_codex_config_selection(
    profile_name: &str,
    config_path: &Path,
    requested_dir: &Path,
) -> CodexConfigSelection {
    let (effective_dir, unavailable_reason) =
        match expand_codex_config_dir(config_path, requested_dir) {
            Ok(path) => match check_codex_config_dir(&path) {
                Ok(()) => (path, None),
                Err(reason) => (path, Some(reason)),
            },
            Err(reason) => (requested_dir.to_path_buf(), Some(reason)),
        };
    CodexConfigSelection {
        profile_name: profile_name.to_string(),
        config_path: config_path.to_path_buf(),
        requested_dir: requested_dir.to_path_buf(),
        effective_dir,
        unavailable_reason,
    }
}

fn expand_codex_config_dir(
    config_path: &Path,
    requested_dir: &Path,
) -> Result<PathBuf, &'static str> {
    if requested_dir.as_os_str().is_empty() {
        return Err("is empty");
    }
    let requested = requested_dir.to_string_lossy();
    let expanded = if requested == "~" {
        BaseDirs::new()
            .map(|dirs| dirs.home_dir().to_path_buf())
            .ok_or("home-directory shorthand could not be expanded")?
    } else if let Some(suffix) = requested
        .strip_prefix("~/")
        .or_else(|| requested.strip_prefix("~\\"))
    {
        BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(suffix))
            .ok_or("home-directory shorthand could not be expanded")?
    } else {
        requested_dir.to_path_buf()
    };

    if expanded.is_absolute() {
        return Ok(expanded);
    }
    let config_dir = config_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    Ok(config_dir.join(expanded))
}

fn check_codex_config_dir(path: &Path) -> Result<(), &'static str> {
    let metadata = std::fs::metadata(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => "does not exist",
        _ => "cannot be accessed",
    })?;
    if !metadata.is_dir() {
        return Err("is not a directory");
    }
    let mut entries = std::fs::read_dir(path).map_err(|_| "is not readable")?;
    if entries.next().is_some_and(|entry| entry.is_err()) {
        return Err("is not readable");
    }
    Ok(())
}

pub(super) fn resolve_run_policy<'a>(
    cfg: &'a config::Config,
    policy_name: Option<&str>,
) -> Result<Option<&'a config::RunPolicy>> {
    Ok(policy_name
        .map(|name| {
            cfg.run_policies
                .get(name)
                .ok_or_else(|| AixError::RunPolicyNotFound {
                    name: name.to_string(),
                    available_hint: config::sorted_run_policy_names(cfg)
                        .into_iter()
                        .map(|name| format!("  {name}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                })
        })
        .transpose()?)
}

fn resolve_api_key_launch(
    request: LaunchRequest<'_>,
    context: &LaunchContext,
    profile_entry: &config::Profile,
) -> Result<LaunchResolution> {
    let LaunchRequest {
        format_override,
        configured_tool_name,
        tool_env_mode,
        codex_config,
        allowed_models,
        policy_name,
        ..
    } = request;
    let LaunchContext {
        cfg, profile_name, ..
    } = context;
    let policy = resolve_run_policy(cfg, policy_name)?;

    let resolved_requested_models = allowed_models
        .iter()
        .map(|model| config::resolve_model(Some(model), cfg, profile_entry))
        .collect::<Result<Vec<_>, _>>()?;
    let resolved_allowed_models = resolve_allowed_models(
        policy.and_then(|policy| policy.allowed_models.as_ref()),
        resolved_requested_models,
        cfg,
        profile_entry,
    )?;
    let configured_tool = configured_tool_name.and_then(|name| cfg.tools.get(name));
    let api_format = configured_tool
        .map(|tool| tool.api_format)
        .or(format_override)
        .unwrap_or(config::ApiFormat::Both);
    let local_gateway = configured_tool.is_some_and(|tool| tool.local_gateway);
    if policy.is_some_and(|policy| policy.max_budget.is_none())
        && !resolved_allowed_models.is_empty()
        && !local_gateway
    {
        return Err(AixError::RunPolicyEnforcementUnavailable {
            policy: policy_name.unwrap_or_default().to_string(),
            constraint: "allowed_models",
        }
        .into());
    }
    let effective_program = configured_tool
        .and_then(|tool| tool.command.as_deref())
        .or(configured_tool_name)
        .unwrap_or_default()
        .to_string();
    if local_gateway {
        validate_executable(&effective_program)?;
    }

    let dry_run = matches!(tool_env_mode, ToolEnvMode::NamesOnly);
    let local_gateway_dry_run = local_gateway && dry_run;
    let codex_config_dry_run = codex_config.is_some() && dry_run;
    let codex_config_active = codex_config
        .as_ref()
        .is_some_and(|selection| selection.is_active());
    let skipped_env_names: &[&str] = if codex_config_active {
        &["CODEX_HOME"]
    } else {
        &[]
    };
    let (mut vars, mut display_only_vars, parent_gateway) =
        if local_gateway_dry_run || codex_config_dry_run {
            let mut vars: Vec<_> = collect_vars(profile_name, "", "", &api_format)
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect();
            if local_gateway_dry_run {
                vars.retain(|(name, _)| !is_gateway_managed_env_var(name));
            }
            let mut display_only_vars = Vec::new();
            append_configured_env_excluding(
                &profile_entry.env,
                ToolEnvMode::NamesOnly,
                &mut vars,
                &mut display_only_vars,
                skipped_env_names,
            )?;
            if local_gateway_dry_run {
                display_only_vars.extend(ApiKeyGatewayHandle::dry_run_variable_names(api_format));
            }
            (vars, display_only_vars, None)
        } else {
            let api_key = profile_entry.resolve_api_key()?;
            let base_url = config::resolve_base_url(profile_entry, &cfg.endpoint)?;
            let vars = collect_profile_vars_excluding(
                profile_name,
                api_key.expose_secret(),
                base_url.expose_secret(),
                &api_format,
                profile_entry,
                skipped_env_names,
            )?;
            (
                vars,
                Vec::new(),
                Some(ParentGatewayCredentials { base_url, api_key }),
            )
        };
    let clear_vars = if configured_tool.is_some() {
        match api_format {
            config::ApiFormat::Anthropic => {
                vec!["OPENAI_API_KEY".to_string(), "OPENAI_BASE_URL".to_string()]
            }
            config::ApiFormat::OpenAi => vec![
                "ANTHROPIC_API_KEY".to_string(),
                "ANTHROPIC_BASE_URL".to_string(),
            ],
            config::ApiFormat::Both => Vec::new(),
        }
    } else {
        Vec::new()
    };
    if let Some(tool) = configured_tool {
        append_configured_env_excluding(
            &tool.env,
            tool_env_mode,
            &mut vars,
            &mut display_only_vars,
            skipped_env_names,
        )?;
    }

    let logical_tool_name = configured_tool
        .map(|_| configured_tool_name.unwrap().to_string())
        .or_else(|| {
            (configured_tool_name == Some("codex") && codex_config_active)
                .then(|| "codex".to_string())
        });
    let sidecar_plan = local_gateway.then_some(LaunchSidecarPlan::ApiKey { api_format });
    let codex_provider_overrides = if codex_config_active
        && configured_tool_name == Some("codex")
        && api_format.supports_openai()
        && !local_gateway
    {
        vars.iter()
            .rev()
            .find(|(name, _)| name == "OPENAI_BASE_URL")
            .map(|(_, base_url)| chatgpt::codex_api_key_provider_overrides(base_url))
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    Ok(LaunchResolution {
        env: LaunchEnv {
            vars,
            auth_vars: Vec::new(),
            display_only_vars,
            clear_vars,
            remove_vars: Vec::new(),
            profile_name: profile_name.clone(),
        },
        program: effective_program,
        logical_tool_name,
        parent_gateway,
        allowed_models: resolved_allowed_models,
        policy: policy.map(|policy| ResolvedRunPolicy {
            name: policy_name.unwrap_or_default().to_string(),
            max_budget: policy.max_budget,
            max_duration: policy.max_duration.clone(),
            tags: policy.tags.clone(),
        }),
        prepend_args: Vec::new(),
        codex_provider_overrides,
        sidecar_plan,
        codex_config: None,
    })
}

fn is_gateway_managed_env_var(name: &str) -> bool {
    matches!(
        name,
        "ANTHROPIC_API_KEY"
            | "ANTHROPIC_BASE_URL"
            | "OPENAI_API_KEY"
            | "OPENAI_BASE_URL"
            | "LITELLM_API_KEY"
            | "LITELLM_BASE_URL"
    )
}

fn append_configured_env_excluding(
    values: &std::collections::HashMap<String, crate::secrets::SecretSource>,
    mode: ToolEnvMode,
    vars: &mut Vec<(String, String)>,
    display_only_vars: &mut Vec<String>,
    excluded_names: &[&str],
) -> Result<()> {
    let mut values: Vec<_> = values.iter().collect();
    values.sort_unstable_by_key(|(key, _)| key.as_str());
    for (key, value) in values {
        if excluded_names.contains(&key.as_str()) {
            continue;
        }
        match mode {
            ToolEnvMode::Resolve => {
                vars.push((key.clone(), value.resolve()?.expose_secret().to_string()));
            }
            ToolEnvMode::NamesOnly => display_only_vars.push(key.clone()),
        }
    }
    Ok(())
}

pub(super) fn resolve_allowed_models(
    policy_models: Option<&Vec<String>>,
    requested_models: Vec<String>,
    cfg: &config::Config,
    profile: &config::Profile,
) -> Result<Vec<String>, AixError> {
    let Some(policy_models) = policy_models else {
        return Ok(requested_models);
    };
    let resolved_policy_models = policy_models
        .iter()
        .map(|model| config::resolve_model(Some(model), cfg, profile))
        .collect::<Result<Vec<_>, _>>()?;
    let policy_model_set: std::collections::BTreeSet<_> =
        resolved_policy_models.iter().map(String::as_str).collect();
    for model in &requested_models {
        if !policy_model_set.contains(model.as_str()) {
            return Err(AixError::RunPolicyModelNotAllowed);
        }
    }

    if requested_models.is_empty() {
        Ok(deduplicate_models(resolved_policy_models))
    } else {
        Ok(deduplicate_models(requested_models))
    }
}

fn deduplicate_models(models: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    models
        .into_iter()
        .filter(|model| seen.insert(model.clone()))
        .collect()
}

pub fn apply_lease_credentials(
    env: &mut LaunchEnv,
    parent_key: &str,
    leased_key: &str,
    base_url: &str,
) {
    const API_KEY_VARS: [&str; 3] = ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "LITELLM_API_KEY"];
    const BASE_URL_VARS: [&str; 3] = ["ANTHROPIC_BASE_URL", "OPENAI_BASE_URL", "LITELLM_BASE_URL"];

    env.vars.retain_mut(|(name, value)| {
        if API_KEY_VARS.contains(&name.as_str()) {
            *value = leased_key.to_string();
            true
        } else if BASE_URL_VARS.contains(&name.as_str()) {
            *value = if name == "ANTHROPIC_BASE_URL" {
                base_url.to_string()
            } else {
                crate::commands::env::append_v1(base_url)
            };
            true
        } else {
            true
        }
    });

    scrub_parent_credential(env, parent_key);
}

pub(crate) fn scrub_parent_credential(env: &mut LaunchEnv, parent_key: &str) {
    if parent_key.is_empty() {
        return;
    }

    env.vars.retain(|(_, value)| !value.contains(parent_key));
    env.auth_vars
        .retain(|(_, value)| !value.expose_secret().contains(parent_key));

    for (name, value) in std::env::vars_os() {
        if value.to_string_lossy().contains(parent_key) {
            let name = name.to_string_lossy().into_owned();
            let overridden = env
                .vars
                .iter()
                .any(|(configured_name, _)| configured_name == &name)
                || env
                    .auth_vars
                    .iter()
                    .any(|(configured_name, _)| configured_name == &name)
                || env
                    .clear_vars
                    .iter()
                    .any(|cleared_name| cleared_name == &name);
            if !overridden {
                env.remove_vars.push(name);
            }
        }
    }

    env.remove_vars.sort_unstable();
    env.remove_vars.dedup();
}

pub async fn run_named_tool(
    name: &str,
    selection: ProfileSelection,
    config_path: Option<PathBuf>,
    dry_run: bool,
    args: Vec<String>,
    timeout: Duration,
) -> Result<()> {
    let fallback_format = if name == "claude" {
        config::ApiFormat::Anthropic
    } else {
        config::ApiFormat::OpenAi
    };
    let tool_env_mode = if dry_run {
        ToolEnvMode::NamesOnly
    } else {
        ToolEnvMode::Resolve
    };
    let mut resolved = resolve_tool_launch(
        LaunchRequest {
            selection,
            explicit_profile: None,
            config_path,
            format_override: Some(fallback_format),
            configured_tool_name: Some(name),
            tool_env_mode,
            allowed_models: &[],
            policy_name: None,
            require_litellm: false,
            codex_config: None,
            native_args: &args,
        },
        timeout,
    )
    .await?;
    let mut args = resolved
        .prepend_args
        .iter()
        .cloned()
        .chain(args)
        .collect::<Vec<_>>();
    validate_codex_config_args(&resolved, &args)?;
    if dry_run {
        print_tool_config_dry_run(&resolved);
        print_sidecar_dry_run(&resolved);
        return run_command(&resolved.program, &args, &resolved.env, true);
    }

    let Some(sidecar) = start_launch_sidecar(&mut resolved, timeout, None, None, None).await?
    else {
        args.extend(resolved.codex_provider_overrides.iter().cloned());
        return run_command(&resolved.program, &args, &resolved.env, false);
    };
    args.extend(resolved.codex_provider_overrides.iter().cloned());
    let (interrupt_requested, terminated) = install_interrupt_handlers()?;
    let child_result = sidecar
        .run_child(|| {
            run_command_status_interruptible(
                &resolved.program,
                &args,
                &resolved.env,
                &interrupt_requested,
                &terminated,
            )
        })
        .await;
    let status = child_result?;
    if let Some(interruption) = interruption_reason(&interrupt_requested, &terminated) {
        std::process::exit(interruption.exit_code());
    }
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

pub(crate) async fn start_launch_sidecar(
    resolved: &mut ResolvedRunLaunch,
    timeout: Duration,
    run_id: Option<Uuid>,
    upstream_api_key_override: Option<&SecretString>,
    deadline: Option<Instant>,
) -> Result<Option<StartedLaunchSidecar>> {
    let Some(plan) = resolved.sidecar_plan.take() else {
        return Ok(None);
    };
    let context = sidecar_context(resolved, run_id, deadline);
    let server = match plan {
        LaunchSidecarPlan::OpenCodeSiwc => {
            let bridge = opencode::BridgeHandle::start(context, timeout).await?;
            bridge.configure_env(&mut resolved.env);
            bridge.into_server()
        }
        LaunchSidecarPlan::CodexChatGptResponses { access_token_env } => {
            let bridge = ChatGptResponsesHandle::start(
                context,
                timeout,
                crate::local_gateway::AuthFailureResponse::new(
                    "aix_chatgpt_gateway_error",
                    "Invalid local ChatGPT credential",
                ),
            )
            .await?;
            let base_url = format!("http://127.0.0.1:{}/v1", bridge.port());
            resolved
                .env
                .auth_vars
                .push((access_token_env.clone(), bridge.child_token()));
            resolved
                .codex_provider_overrides
                .extend(chatgpt::codex_provider_overrides(
                    &base_url,
                    &access_token_env,
                ));
            bridge.into_server()
        }
        LaunchSidecarPlan::ApiKey { api_format } => {
            let parent_gateway = resolved
                .parent_gateway
                .as_ref()
                .ok_or(AixError::ChatGptAuthUnsupported)?;
            let upstream_api_key = upstream_api_key_override
                .unwrap_or(&parent_gateway.api_key)
                .expose_secret();
            let gateway = ApiKeyGatewayHandle::start(
                context,
                api_format,
                parent_gateway.base_url.expose_secret(),
                upstream_api_key,
            )
            .await?;
            gateway.configure_env(&mut resolved.env);
            if api_format.supports_openai()
                && resolved.logical_tool_name.as_deref() == Some("codex")
                && resolved
                    .codex_config
                    .as_ref()
                    .is_some_and(CodexConfigSelection::is_active)
            {
                if let Some(base_url) = resolved
                    .env
                    .vars
                    .iter()
                    .rev()
                    .find(|(name, _)| name == "OPENAI_BASE_URL")
                    .map(|(_, value)| value.clone())
                {
                    resolved
                        .codex_provider_overrides
                        .extend(chatgpt::codex_api_key_provider_overrides(&base_url));
                }
            }
            scrub_parent_credential(&mut resolved.env, parent_gateway.api_key.expose_secret());
            gateway.into_server()
        }
    };
    Ok(Some(StartedLaunchSidecar(server)))
}

fn sidecar_context(
    resolved: &ResolvedRunLaunch,
    run_id: Option<Uuid>,
    deadline: Option<Instant>,
) -> LocalGatewayContext {
    LocalGatewayContext::with_enforcement(
        resolved.env.profile_name.clone(),
        resolved
            .logical_tool_name
            .clone()
            .expect("a sidecar launch has a logical tool name"),
        run_id.map(|run_id| run_id.to_string()),
        resolved.policy.as_ref().map(|policy| policy.name.clone()),
        crate::local_gateway::RequestEnforcement {
            allowed_models: resolved.allowed_models.clone(),
            deadline,
        },
    )
}

pub(crate) fn print_sidecar_dry_run(resolved: &ResolvedRunLaunch) {
    if let Some(plan) = &resolved.sidecar_plan {
        match plan {
            LaunchSidecarPlan::OpenCodeSiwc => opencode::print_dry_run(),
            LaunchSidecarPlan::CodexChatGptResponses { access_token_env } => {
                eprintln!("transport: local_gateway");
                eprintln!(
                    "Would use a dynamic local Responses provider/base URL for Codex app-server"
                );
                eprintln!("Would set local bearer in {access_token_env}");
            }
            LaunchSidecarPlan::ApiKey { api_format } => {
                ApiKeyGatewayHandle::print_dry_run(*api_format)
            }
        }
    }
}

pub(crate) fn print_tool_config_dry_run(resolved: &ResolvedRunLaunch) {
    if let Some(selection) = &resolved.codex_config {
        selection.print_dry_run();
    }
}

pub(crate) fn validate_codex_config_args(
    resolved: &ResolvedRunLaunch,
    args: &[String],
) -> Result<(), AixError> {
    validate_codex_config_selection_args(resolved.codex_config.as_ref(), args)
}

fn validate_codex_config_selection_args(
    selection: Option<&CodexConfigSelection>,
    args: &[String],
) -> Result<(), AixError> {
    let conflicting_argument = selection
        .is_some_and(CodexConfigSelection::is_active)
        .then(|| {
            args.iter().find_map(|argument| match argument.as_str() {
                "--ignore-user-config" => Some("--ignore-user-config"),
                "--oss" => Some("--oss"),
                "--local-provider" => Some("--local-provider"),
                "--remote" => Some("--remote"),
                value if value.starts_with("--local-provider=") => Some("--local-provider"),
                value if value.starts_with("--remote=") => Some("--remote"),
                _ => None,
            })
        })
        .flatten();
    if let Some(argument) = conflicting_argument {
        return Err(AixError::CodexConfigArgumentConflict { argument });
    }
    Ok(())
}

fn install_interrupt_handlers() -> Result<(Arc<AtomicBool>, Arc<AtomicBool>), AixError> {
    let interrupt_requested = Arc::new(AtomicBool::new(false));
    let ctrlc_flag = Arc::clone(&interrupt_requested);
    ctrlc::set_handler(move || ctrlc_flag.store(true, Ordering::SeqCst))
        .map_err(|error| AixError::RunInterruptHandler(std::io::Error::other(error.to_string())))?;
    #[cfg(unix)]
    let terminated = {
        let flag = Arc::new(AtomicBool::new(false));
        signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&flag))
            .map_err(AixError::RunInterruptHandler)?;
        flag
    };
    #[cfg(not(unix))]
    let terminated = Arc::new(AtomicBool::new(false));
    Ok((interrupt_requested, terminated))
}

pub fn detect_shell() -> String {
    detect_shell_impl()
}

pub(crate) fn chatgpt_model_ids() -> &'static [&'static str] {
    opencode::CHATGPT_MODEL_IDS
}

pub fn run_command(program: &str, args: &[String], env: &LaunchEnv, dry_run: bool) -> Result<()> {
    if dry_run {
        let display_args: Vec<&str> = std::iter::once(program)
            .chain(args.iter().map(String::as_str))
            .collect();
        eprintln!("Would run: {}", display_args.join(" "));
        eprintln!("Would set:");
        for key in env
            .vars
            .iter()
            .map(|(key, _)| key)
            .chain(env.auth_vars.iter().map(|(key, _)| key))
            .chain(env.display_only_vars.iter())
        {
            eprintln!("  {key}");
        }
        return Ok(());
    }

    let status = run_command_status(program, args, env)?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

pub fn run_command_status(
    program: &str,
    args: &[String],
    env: &LaunchEnv,
) -> Result<std::process::ExitStatus, AixError> {
    let mut cmd = command_with_env(program, args, env)?;
    cmd.status().map_err(|source| AixError::ProcessSpawn {
        program: program.to_string(),
        source,
    })
}

pub fn run_command_status_interruptible(
    program: &str,
    args: &[String],
    env: &LaunchEnv,
    interrupt_requested: &AtomicBool,
    terminated: &AtomicBool,
) -> Result<std::process::ExitStatus, AixError> {
    run_command_status_interruptible_with_deadline(
        program,
        args,
        env,
        interrupt_requested,
        terminated,
        None,
    )
    .map(|outcome| outcome.status)
}

pub(crate) fn run_command_status_interruptible_with_deadline(
    program: &str,
    args: &[String],
    env: &LaunchEnv,
    interrupt_requested: &AtomicBool,
    terminated: &AtomicBool,
    deadline: Option<Instant>,
) -> Result<ChildRunOutcome, AixError> {
    let mut command = command_with_env(program, args, env)?;
    command
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    for key in &env.remove_vars {
        command.env_remove(key);
    }

    let mut child = command.spawn().map_err(|source| AixError::ProcessSpawn {
        program: program.to_string(),
        source,
    })?;
    supervise_child(
        &mut child,
        program,
        interrupt_requested,
        terminated,
        deadline,
    )
}

fn supervise_child(
    child: &mut std::process::Child,
    program: &str,
    interrupt_requested: &AtomicBool,
    terminated: &AtomicBool,
    deadline: Option<Instant>,
) -> Result<ChildRunOutcome, AixError> {
    loop {
        if let Some(status) = child.try_wait().map_err(|source| AixError::ProcessWait {
            program: program.to_string(),
            source,
        })? {
            return Ok(ChildRunOutcome {
                status,
                duration_expired: false,
            });
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Ok(ChildRunOutcome {
                status: terminate_child(child, program, None)?,
                duration_expired: true,
            });
        }

        if let Some(interruption) = interruption_reason(interrupt_requested, terminated) {
            return Ok(ChildRunOutcome {
                status: terminate_child(child, program, Some(interruption))?,
                duration_expired: false,
            });
        }
        let poll_interval = deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .map_or(Duration::from_millis(20), |remaining| {
                remaining.min(Duration::from_millis(20))
            });
        std::thread::sleep(poll_interval);
    }
}

fn terminate_child(
    child: &mut std::process::Child,
    program: &str,
    interruption: Option<Interruption>,
) -> Result<std::process::ExitStatus, AixError> {
    #[cfg(unix)]
    {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;

        let child_pid = Pid::from_raw(child.id() as i32);
        let signal = match interruption {
            Some(Interruption::CtrlC) => Signal::SIGINT,
            Some(Interruption::Termination) | None => Signal::SIGTERM,
        };
        let _ = kill(child_pid, signal);

        let grace_period = Instant::now() + Duration::from_millis(500);
        loop {
            if let Some(status) = child.try_wait().map_err(|source| AixError::ProcessWait {
                program: program.to_string(),
                source,
            })? {
                return Ok(status);
            }
            if Instant::now() >= grace_period {
                let _ = kill(child_pid, Signal::SIGKILL);
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = interruption;
        child.kill().map_err(|source| AixError::ProcessWait {
            program: program.to_string(),
            source,
        })?;
    }

    child.wait().map_err(|source| AixError::ProcessWait {
        program: program.to_string(),
        source,
    })
}

fn command_with_env(
    program: &str,
    args: &[String],
    env: &LaunchEnv,
) -> Result<std::process::Command, AixError> {
    let resolved = find_executable(program)?;
    let mut command = std::process::Command::new(resolved);
    command.args(args);
    for key in &env.clear_vars {
        command.env_remove(key);
    }
    for (key, value) in &env.vars {
        command.env(key, value);
    }
    for (key, value) in &env.auth_vars {
        command.env(key, value.expose_secret());
    }
    Ok(command)
}

pub fn interruption_reason(
    interrupt_requested: &AtomicBool,
    terminated: &AtomicBool,
) -> Option<Interruption> {
    if terminated.load(Ordering::SeqCst) {
        Some(Interruption::Termination)
    } else if interrupt_requested.load(Ordering::SeqCst) {
        Some(Interruption::CtrlC)
    } else {
        None
    }
}

fn find_executable(program: &str) -> Result<PathBuf, AixError> {
    let path = Path::new(program);
    if path.is_absolute() {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        return Err(AixError::ExecutableNotFound {
            program: program.to_string(),
        });
    }
    which::which(program).map_err(|_| AixError::ExecutableNotFound {
        program: program.to_string(),
    })
}

pub(crate) fn validate_executable(program: &str) -> Result<(), AixError> {
    find_executable(program).map(|_| ())
}

#[cfg(unix)]
fn detect_shell_impl() -> String {
    if let Ok(shell) = std::env::var("SHELL") {
        if !shell.is_empty() {
            return shell;
        }
    }
    if std::env::var("NU_VERSION").is_ok() && which::which("nu").is_ok() {
        return "nu".to_string();
    }
    "sh".to_string()
}

#[cfg(windows)]
fn detect_shell_impl() -> String {
    if let Ok(shell) = std::env::var("SHELL") {
        if !shell.is_empty() {
            return shell;
        }
    }
    for candidate in &["pwsh", "powershell"] {
        if which::which(candidate).is_ok() {
            return candidate.to_string();
        }
    }
    "cmd".to_string()
}

#[cfg(not(any(unix, windows)))]
fn detect_shell_impl() -> String {
    "sh".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_gateway::AuthFailureResponse;

    fn test_tool_command() -> String {
        serde_json::to_string(
            &std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .to_string(),
        )
        .unwrap()
    }

    fn chatgpt_opencode_config() -> config::Config {
        let config_text = format!(
            r#"
default_profile = "personal"

[profiles.personal]
auth = {{ type = "chatgpt" }}

[profiles.personal.env]
ACCESS_TOKEN = "profile-access-token"
OPENAI_API_KEY = "profile-openai-key"
KEEP_ME = "profile-setting"

[tools.opencode]
command = {}
api_format = "openai"

[tools.opencode.env]
ACCESS_TOKEN = "tool-access-token"
OPENAI_API_KEY = "tool-openai-key"
TOOL_SETTING = "kept"

[tools.opencode.chatgpt]
access_token_env = "ACCESS_TOKEN"
clear_env = ["CODEX_API_KEY"]
"#,
            test_tool_command()
        );
        let config: config::Config = toml::from_str(&config_text).unwrap();
        config::validate(&config).unwrap();
        config
    }

    fn test_launch_request<'a>(tool_env_mode: ToolEnvMode) -> LaunchRequest<'a> {
        LaunchRequest {
            selection: ProfileSelection {
                profile: Some("personal".to_string()),
                non_interactive: true,
            },
            explicit_profile: None,
            config_path: None,
            format_override: None,
            configured_tool_name: Some("opencode"),
            tool_env_mode,
            allowed_models: &[],
            policy_name: None,
            require_litellm: false,
            codex_config: None,
            native_args: &[],
        }
    }

    #[tokio::test]
    async fn chatgpt_opencode_needs_no_aix_model_default_and_only_plans_a_local_bridge() {
        let context = LaunchContext {
            cfg: chatgpt_opencode_config(),
            profile_name: "personal".to_string(),
            config_path: PathBuf::from("aix.toml"),
        };
        let profile = context.profile().unwrap();
        let resolved = chatgpt::resolve_tool_launch(
            test_launch_request(ToolEnvMode::Resolve),
            &context,
            profile,
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        assert!(matches!(
            resolved.sidecar_plan.as_ref().unwrap(),
            LaunchSidecarPlan::OpenCodeSiwc
        ));
        assert!(resolved.env.auth_vars.is_empty());
        assert!(!resolved.env.vars.iter().any(|(name, _)| {
            matches!(
                name.as_str(),
                "ACCESS_TOKEN" | "OPENAI_API_KEY" | "CODEX_API_KEY"
            )
        }));
        assert!(resolved
            .env
            .clear_vars
            .contains(&"ACCESS_TOKEN".to_string()));
        assert!(!resolved.env.vars.iter().any(|(name, _)| {
            name == opencode::OPENCODE_CONFIG_ENV || name == opencode::BRIDGE_TOKEN_ENV
        }));
    }

    #[tokio::test]
    async fn opencode_dry_run_resolves_no_token_and_reports_only_variable_names() {
        let context = LaunchContext {
            cfg: chatgpt_opencode_config(),
            profile_name: "personal".to_string(),
            config_path: PathBuf::from("aix.toml"),
        };
        let profile = context.profile().unwrap();
        let resolved = chatgpt::resolve_tool_launch(
            test_launch_request(ToolEnvMode::NamesOnly),
            &context,
            profile,
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        assert!(resolved.env.auth_vars.is_empty());
        assert_eq!(
            resolved.env.vars,
            [("AIX_PROFILE".to_string(), "personal".to_string())]
        );
        assert!(resolved
            .env
            .display_only_vars
            .contains(&opencode::BRIDGE_TOKEN_ENV.to_string()));
        assert!(resolved
            .env
            .display_only_vars
            .contains(&opencode::OPENCODE_CONFIG_ENV.to_string()));
        assert!(resolved
            .env
            .display_only_vars
            .contains(&"ACCESS_TOKEN".to_string()));
        assert!(resolved.sidecar_plan.is_some());
    }

    #[tokio::test]
    async fn chatgpt_opencode_does_not_require_an_effective_default_model() {
        let cfg = chatgpt_opencode_config();
        let context = LaunchContext {
            cfg,
            profile_name: "personal".to_string(),
            config_path: PathBuf::from("aix.toml"),
        };
        let profile = context.profile().unwrap();
        let resolved = chatgpt::resolve_tool_launch(
            test_launch_request(ToolEnvMode::NamesOnly),
            &context,
            profile,
            Duration::from_secs(5),
        )
        .await
        .expect("OpenCode selects models through its native picker");
        assert!(matches!(
            resolved.sidecar_plan,
            Some(LaunchSidecarPlan::OpenCodeSiwc)
        ));
    }

    #[test]
    fn api_key_opencode_launch_does_not_gain_a_chatgpt_sidecar() {
        let config: config::Config = toml::from_str(&format!(
            r#"
[endpoint]
base_url = "https://gateway.example/v1"

[profiles.work]
api_key = "api-key-profile"

[tools.opencode]
command = {}
api_format = "openai"
[tools.opencode.chatgpt]
access_token_env = "ACCESS_TOKEN"
"#,
            test_tool_command()
        ))
        .unwrap();
        config::validate(&config).unwrap();
        let context = LaunchContext {
            cfg: config,
            profile_name: "work".to_string(),
            config_path: PathBuf::from("aix.toml"),
        };
        let profile = context.profile().unwrap();
        let resolution = resolve_api_key_launch(
            LaunchRequest {
                selection: ProfileSelection {
                    profile: Some("work".to_string()),
                    non_interactive: true,
                },
                explicit_profile: None,
                config_path: None,
                format_override: None,
                configured_tool_name: Some("opencode"),
                tool_env_mode: ToolEnvMode::Resolve,
                allowed_models: &[],
                policy_name: None,
                require_litellm: false,
                codex_config: None,
                native_args: &[],
            },
            &context,
            profile,
        )
        .unwrap();

        assert!(resolution.parent_gateway.is_some());
        assert!(resolution.sidecar_plan.is_none());
        assert!(resolution
            .env
            .vars
            .iter()
            .any(|(name, _)| name == "OPENAI_API_KEY"));
    }

    #[test]
    fn sidecar_context_carries_run_metadata_without_inventing_named_tool_runs() {
        let mut resolved = ResolvedRunLaunch {
            env: LaunchEnv {
                vars: Vec::new(),
                auth_vars: Vec::new(),
                display_only_vars: Vec::new(),
                clear_vars: Vec::new(),
                remove_vars: Vec::new(),
                profile_name: "personal".to_string(),
            },
            program: "opencode".to_string(),
            logical_tool_name: Some("opencode".to_string()),
            parent_gateway: None,
            allowed_models: Vec::new(),
            policy: Some(ResolvedRunPolicy {
                name: "bounded".to_string(),
                max_budget: Some(10.0),
                max_duration: "1h".to_string(),
                tags: Vec::new(),
            }),
            prepend_args: Vec::new(),
            codex_provider_overrides: Vec::new(),
            codex_config: None,
            sidecar_plan: None,
        };
        let run_id = Uuid::parse_str("4b9a85df-51d9-49a4-9a17-69d7f0dc91f1").unwrap();
        let run_id_string = run_id.to_string();

        let run_context = sidecar_context(&resolved, Some(run_id), None);
        assert_eq!(run_context.profile, "personal");
        assert_eq!(run_context.logical_tool_name, "opencode");
        assert_eq!(run_context.run_id.as_deref(), Some(run_id_string.as_str()));
        assert_eq!(run_context.run_policy.as_deref(), Some("bounded"));

        resolved.logical_tool_name = Some("codex".to_string());
        let codex_context = sidecar_context(&resolved, Some(run_id), None);
        assert_eq!(codex_context.logical_tool_name, "codex");
        assert_eq!(
            codex_context.run_id.as_deref(),
            Some(run_id_string.as_str())
        );
        assert_eq!(codex_context.run_policy.as_deref(), Some("bounded"));

        resolved.policy = None;
        let named_tool_context = sidecar_context(&resolved, None, None);
        assert_eq!(named_tool_context.run_id, None);
        assert_eq!(named_tool_context.run_policy, None);
    }

    #[cfg(unix)]
    #[test]
    fn child_exit_is_observed_before_an_expired_deadline_is_classified() {
        let mut child = std::process::Command::new("sh")
            .args(["-c", "exit 7"])
            .spawn()
            .unwrap();
        let status = child.wait().unwrap();
        assert_eq!(status.code(), Some(7));

        let outcome = supervise_child(
            &mut child,
            "sh",
            &AtomicBool::new(false),
            &AtomicBool::new(false),
            Some(Instant::now() - Duration::from_millis(1)),
        )
        .unwrap();

        assert!(!outcome.duration_expired);
        assert_eq!(outcome.status.code(), Some(7));
    }

    #[tokio::test]
    async fn sidecar_stops_after_success_failure_interrupt_or_spawn_error() {
        let outcomes: [Result<&str, &str>; 4] = [
            Ok("success"),
            Ok("child failure"),
            Ok("interrupt"),
            Err("spawn failure"),
        ];
        let client = reqwest::Client::new();

        for outcome in outcomes {
            let app = axum::Router::new().route("/ready", axum::routing::get(|| async { "ready" }));
            let server = ServerHandle::start(
                app,
                LocalGatewayContext::new(
                    "personal".to_string(),
                    "opencode".to_string(),
                    None,
                    None,
                ),
                AuthFailureResponse::new("test_error", "Invalid test credential"),
            )
            .await
            .unwrap();
            let address = server.address();

            let result = StartedLaunchSidecar(server).run_child(|| outcome).await;

            assert_eq!(result, outcome);
            assert!(client
                .get(format!("http://{address}/ready"))
                .send()
                .await
                .is_err());
        }
    }

    #[test]
    #[cfg(unix)]
    fn detect_shell_returns_shell_env_when_set() {
        let prev = std::env::var("SHELL").ok();
        std::env::set_var("SHELL", "/usr/bin/fish");
        let result = detect_shell();
        match prev {
            Some(v) => std::env::set_var("SHELL", v),
            None => std::env::remove_var("SHELL"),
        }
        assert_eq!(result, "/usr/bin/fish");
    }

    #[test]
    #[cfg(unix)]
    fn detect_shell_falls_back_to_sh_when_shell_not_set() {
        let prev = std::env::var("SHELL").ok();
        let prev_nu = std::env::var("NU_VERSION").ok();
        std::env::remove_var("SHELL");
        std::env::remove_var("NU_VERSION");
        let result = detect_shell();
        match prev {
            Some(v) => std::env::set_var("SHELL", v),
            None => std::env::remove_var("SHELL"),
        }
        match prev_nu {
            Some(v) => std::env::set_var("NU_VERSION", v),
            None => std::env::remove_var("NU_VERSION"),
        }
        assert_eq!(result, "sh");
    }

    #[test]
    fn detect_shell_returns_a_non_empty_string() {
        let shell = detect_shell();
        assert!(!shell.is_empty());
    }

    #[test]
    fn launch_env_vars_include_required_keys() {
        use crate::commands::env::collect_vars;
        use crate::config::ApiFormat;
        let vars = collect_vars(
            "myprofile",
            "sk-test",
            "https://example.com",
            &ApiFormat::Anthropic,
        );
        let names: Vec<&str> = vars.iter().map(|(k, _)| *k).collect();
        assert!(names.contains(&"AIX_PROFILE"));
        assert!(names.contains(&"LITELLM_API_KEY"));
        assert!(!names.contains(&"AIX_API_KEY"));
        assert!(!names.contains(&"AIX_BASE_URL"));
    }
}
