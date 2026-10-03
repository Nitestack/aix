use crate::commands::env::{collect_profile_vars, resolve_profile};
use crate::commands::ProfileSelection;
use crate::config;
use crate::error::AixError;
use crate::secrets::SecretString;
use color_eyre::Result;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[path = "launch/chatgpt.rs"]
mod chatgpt;

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
}

pub(crate) struct ParentGatewayCredentials {
    pub base_url: SecretString,
    pub api_key: SecretString,
}

#[derive(Clone, Debug)]
pub struct ResolvedRunPolicy {
    pub name: String,
    pub max_budget: f64,
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
}

struct LaunchContext {
    cfg: config::Config,
    profile_name: String,
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
        },
        timeout,
    )
    .await
}

async fn resolve_tool_launch(
    request: LaunchRequest<'_>,
    timeout: Duration,
) -> Result<ResolvedRunLaunch> {
    let context = load_launch_context(&request)?;
    let profile = context.profile()?;
    let is_chatgpt = profile.auth.is_chatgpt();
    let resolution = if is_chatgpt {
        chatgpt::resolve_tool_launch(request, &context, profile, timeout).await?
    } else {
        resolve_api_key_launch(request, &context, profile)?
    };
    Ok(ResolvedRunLaunch {
        env: resolution.env,
        program: resolution.program,
        logical_tool_name: resolution.logical_tool_name,
        parent_gateway: resolution.parent_gateway,
        allowed_models: resolution.allowed_models,
        policy: resolution.policy,
        prepend_args: resolution.prepend_args,
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
    if request.require_litellm && explicit_non_litellm_gateway {
        return Err(AixError::LeaseNotLiteLlm.into());
    }

    let profile_name = resolve_profile(
        ProfileSelection {
            profile: policy
                .and_then(|policy| policy.profile.clone())
                .or_else(|| request.selection.profile.clone()),
            non_interactive: request.selection.non_interactive,
        },
        &cfg,
    )?;

    Ok(LaunchContext { cfg, profile_name })
}

fn resolve_run_policy<'a>(
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
        allowed_models,
        policy_name,
        ..
    } = request;
    let LaunchContext { cfg, profile_name } = context;
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
    let api_key = profile_entry.resolve_api_key()?;
    let base_url = config::resolve_base_url(profile_entry, &cfg.endpoint)?;
    let configured_tool = configured_tool_name.and_then(|name| cfg.tools.get(name));
    let api_format = configured_tool
        .map(|tool| tool.api_format)
        .or(format_override)
        .unwrap_or(config::ApiFormat::Both);
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

    let mut vars = collect_profile_vars(
        profile_name,
        api_key.expose_secret(),
        base_url.expose_secret(),
        &api_format,
        profile_entry,
    )?;
    let mut display_only_vars = Vec::new();
    if let Some(tool) = configured_tool {
        append_configured_env(&tool.env, tool_env_mode, &mut vars, &mut display_only_vars)?;
    }

    let logical_tool_name = configured_tool.map(|_| configured_tool_name.unwrap().to_string());
    let effective_program = configured_tool
        .and_then(|tool| tool.command.as_deref())
        .or(configured_tool_name)
        .unwrap_or_default()
        .to_string();

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
        parent_gateway: Some(ParentGatewayCredentials { base_url, api_key }),
        allowed_models: resolved_allowed_models,
        policy: policy.map(|policy| ResolvedRunPolicy {
            name: policy_name.unwrap_or_default().to_string(),
            max_budget: policy.max_budget,
            max_duration: policy.max_duration.clone(),
            tags: policy.tags.clone(),
        }),
        prepend_args: Vec::new(),
    })
}

fn append_configured_env(
    values: &std::collections::HashMap<String, crate::secrets::SecretSource>,
    mode: ToolEnvMode,
    vars: &mut Vec<(String, String)>,
    display_only_vars: &mut Vec<String>,
) -> Result<()> {
    let mut values: Vec<_> = values.iter().collect();
    values.sort_unstable_by_key(|(key, _)| key.as_str());
    for (key, value) in values {
        match mode {
            ToolEnvMode::Resolve => {
                vars.push((key.clone(), value.resolve()?.expose_secret().to_string()));
            }
            ToolEnvMode::NamesOnly => display_only_vars.push(key.clone()),
        }
    }
    Ok(())
}

fn resolve_allowed_models(
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
            parent_key.is_empty() || !value.contains(parent_key)
        }
    });

    if !parent_key.is_empty() {
        for (name, value) in std::env::vars_os() {
            if value.to_string_lossy().contains(parent_key) {
                let name = name.to_string_lossy().into_owned();
                if !env
                    .vars
                    .iter()
                    .any(|(configured_name, _)| configured_name == &name)
                {
                    env.remove_vars.push(name);
                }
            }
        }
        env.remove_vars.sort_unstable();
        env.remove_vars.dedup();
    }
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
    let resolved = resolve_tool_launch(
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
        },
        timeout,
    )
    .await?;
    let args = resolved
        .prepend_args
        .iter()
        .cloned()
        .chain(args)
        .collect::<Vec<_>>();
    run_command(&resolved.program, &args, &resolved.env, dry_run)
}

pub fn detect_shell() -> String {
    detect_shell_impl()
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
    loop {
        if let Some(status) = child.try_wait().map_err(|source| AixError::ProcessWait {
            program: program.to_string(),
            source,
        })? {
            return Ok(status);
        }

        if let Some(interruption) = interruption_reason(interrupt_requested, terminated) {
            return terminate_child(&mut child, program, interruption);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn terminate_child(
    child: &mut std::process::Child,
    program: &str,
    interruption: Interruption,
) -> Result<std::process::ExitStatus, AixError> {
    #[cfg(unix)]
    {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;

        let child_pid = Pid::from_raw(child.id() as i32);
        let signal = if interruption == Interruption::Termination {
            Signal::SIGTERM
        } else {
            Signal::SIGINT
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
