#![allow(dead_code)]

use crate::error::AixError;
use crate::secrets::{DynamicValue, SecretSource};
use directories::BaseDirs;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub default_profile: Option<String>,
    /// Env files to load after the main config is parsed. Real process
    /// environment always wins — values from these files are only applied
    /// when the variable is not already set.
    #[serde(default)]
    pub env_files: Vec<PathBuf>,
    #[serde(default)]
    pub endpoint: Endpoint,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub profiles: HashMap<String, Profile>,
    #[serde(default)]
    pub models: ModelConfig,
    #[serde(default)]
    pub prompts: HashMap<String, PromptPreset>,
    #[serde(default)]
    pub tools: HashMap<String, Tool>,
    #[serde(default)]
    pub run_policies: HashMap<String, RunPolicy>,
}

#[derive(Debug, Deserialize, PartialEq)]
pub enum KnownProvider {
    #[serde(rename = "litellm")]
    LiteLlm,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Provider {
    Known(KnownProvider),
    Custom(String),
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum KnownGateway {
    Litellm,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Gateway {
    Known(KnownGateway),
    Custom(String),
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub enum ApiFormat {
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "both")]
    Both,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    pub command: Option<String>,
    pub api_format: ApiFormat,
    #[serde(default)]
    pub env: HashMap<String, SecretSource>,
    #[serde(default)]
    pub chatgpt: Option<ChatGptTool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatGptTool {
    pub access_token_env: String,
    #[serde(default)]
    pub prepend_args: Vec<String>,
    #[serde(default)]
    pub clear_env: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub base_url: Option<SecretSource>,
    pub provider: Option<Provider>,
    pub gateway: Option<Gateway>,
}

#[derive(Debug)]
pub struct Profile {
    pub label: Option<DynamicValue>,
    pub auth: ProfileAuth,
    /// Overrides the shared endpoint URL for this profile when configured.
    pub base_url: Option<SecretSource>,
    /// Additional environment variables injected when this profile is used.
    pub env: HashMap<String, SecretSource>,
    pub models: ModelConfig,
}

#[derive(Debug)]
pub enum ProfileAuth {
    ApiKey { api_key: SecretSource },
    ChatGpt,
}

impl ProfileAuth {
    pub fn is_chatgpt(&self) -> bool {
        matches!(self, Self::ChatGpt)
    }

    pub fn auth_type(&self) -> &'static str {
        match self {
            Self::ApiKey { .. } => "api_key",
            Self::ChatGpt => "chatgpt",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileConfig {
    #[serde(default)]
    label: Option<DynamicValue>,
    #[serde(default)]
    api_key: Option<SecretSource>,
    #[serde(default)]
    auth: Option<ProfileAuthConfig>,
    #[serde(default)]
    base_url: Option<SecretSource>,
    #[serde(default)]
    env: HashMap<String, SecretSource>,
    #[serde(default)]
    models: ModelConfig,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileAuthConfig {
    #[serde(rename = "type")]
    kind: ProfileAuthKind,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProfileAuthKind {
    Chatgpt,
}

impl<'de> Deserialize<'de> for Profile {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let config = ProfileConfig::deserialize(deserializer)?;
        let auth = match (config.api_key, config.auth) {
            (Some(api_key), None) => ProfileAuth::ApiKey { api_key },
            (
                None,
                Some(ProfileAuthConfig {
                    kind: ProfileAuthKind::Chatgpt,
                }),
            ) => ProfileAuth::ChatGpt,
            _ => {
                return Err(serde::de::Error::custom(
                    "profile must configure exactly one of api_key or ChatGPT auth",
                ))
            }
        };
        Ok(Self {
            label: config.label,
            auth,
            base_url: config.base_url,
            env: config.env,
            models: config.models,
        })
    }
}

impl Profile {
    pub fn resolve_api_key(&self) -> Result<crate::secrets::SecretString, AixError> {
        match &self.auth {
            ProfileAuth::ApiKey { api_key } => api_key.resolve(),
            ProfileAuth::ChatGpt => Err(AixError::ChatGptAuthUnsupported),
        }
    }
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    /// A raw model ID used when the caller does not specify one.
    pub default: Option<String>,
    /// Local names that resolve to raw model IDs. Alias targets are not resolved recursively.
    #[serde(default)]
    pub aliases: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPreset {
    pub prompt: String,
    pub system: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPolicy {
    pub profile: Option<String>,
    pub max_budget: f64,
    pub max_duration: String,
    pub allowed_models: Option<Vec<String>>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    #[serde(default = "default_cache_ttl")]
    pub ttl_secs: u64,
    #[serde(default)]
    pub disabled: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            ttl_secs: default_cache_ttl(),
            disabled: false,
        }
    }
}

fn default_cache_ttl() -> u64 {
    3600
}

pub fn find_config_path(explicit: Option<&Path>) -> Result<Option<PathBuf>, AixError> {
    if let Some(path) = explicit {
        return Ok(Some(path.to_path_buf()));
    }

    let Some(config_dir) = default_config_dir() else {
        return Ok(None);
    };

    for name in &["aix.toml", "aix.yaml", "aix.yml", "aix.json", "aix.json5"] {
        let candidate = config_dir.join(name);
        if candidate.exists() {
            return Ok(Some(candidate));
        }
    }

    Ok(None)
}

fn default_config_dir() -> Option<PathBuf> {
    BaseDirs::new().map(|dirs| dirs.config_dir().join("aix"))
}

pub fn load(path: &Path) -> Result<Config, AixError> {
    let content = std::fs::read_to_string(path).map_err(|e| AixError::ParseError {
        path: path.to_path_buf(),
        source: Box::new(e),
    })?;

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "toml" => toml::from_str(&content).map_err(|e| AixError::ParseError {
            path: path.to_path_buf(),
            source: Box::new(e),
        }),
        "yaml" | "yml" => serde_yaml::from_str(&content).map_err(|e| AixError::ParseError {
            path: path.to_path_buf(),
            source: Box::new(e),
        }),
        "json" => serde_json::from_str(&content).map_err(|e| AixError::ParseError {
            path: path.to_path_buf(),
            source: Box::new(e),
        }),
        "json5" => json5::from_str(&content).map_err(|e| AixError::ParseError {
            path: path.to_path_buf(),
            source: Box::new(e),
        }),
        _ => Err(AixError::UnknownFormat { ext }),
    }
}

/// Load env files listed in `config.env_files` into the process environment.
///
/// Real process environment wins: `dotenvy::from_path` only sets a variable
/// when it is not already present in the environment. `~` in paths is expanded.
pub fn load_env_files(config: &Config) -> Result<(), AixError> {
    for raw_path in &config.env_files {
        let expanded = shellexpand::tilde(&raw_path.to_string_lossy()).into_owned();
        let path = PathBuf::from(expanded);
        dotenvy::from_path(&path).map_err(|e| AixError::EnvFileLoad {
            path: path.clone(),
            source: Box::new(e),
        })?;
    }
    Ok(())
}

/// Resolve the profile's endpoint URL, falling back to the shared endpoint.
pub fn resolve_base_url(
    profile: &Profile,
    endpoint: &Endpoint,
) -> Result<crate::secrets::SecretString, AixError> {
    if profile.auth.is_chatgpt() {
        return Err(AixError::ChatGptAuthUnsupported);
    }
    match &profile.base_url {
        Some(base_url) => base_url.resolve(),
        None => endpoint
            .base_url
            .as_ref()
            .ok_or(AixError::MissingEndpointUrl)?
            .resolve(),
    }
}

/// Resolve an explicit model name or the selected profile's effective default.
pub fn resolve_model(
    requested: Option<&str>,
    config: &Config,
    profile: &Profile,
) -> Result<String, AixError> {
    if let Some(requested) = requested {
        let resolved = profile
            .models
            .aliases
            .get(requested)
            .or_else(|| config.models.aliases.get(requested))
            .map(String::as_str)
            .unwrap_or(requested);
        return Ok(resolved.to_string());
    }
    profile
        .models
        .default
        .as_ref()
        .or(config.models.default.as_ref())
        .cloned()
        .ok_or(AixError::NoModelConfigured)
}

pub fn validate(config: &Config) -> Result<(), AixError> {
    if config.profiles.is_empty() {
        return Err(AixError::NoProfilesConfigured);
    }

    if let Some(ref name) = config.default_profile {
        if !config.profiles.contains_key(name.as_str()) {
            return Err(AixError::UnknownDefaultProfile(name.clone()));
        }
    }

    validate_models(&config.models, "models")?;

    for name in sorted_prompt_names(config) {
        if name.trim().is_empty() {
            return Err(AixError::EmptyPromptName);
        }
        let preset = &config.prompts[name];
        if preset.prompt.trim().is_empty() {
            return Err(AixError::EmptyPromptField {
                name: name.to_string(),
                field: "prompt",
            });
        }
        if preset
            .system
            .as_deref()
            .is_some_and(|system| system.trim().is_empty())
        {
            return Err(AixError::EmptyPromptField {
                name: name.to_string(),
                field: "system",
            });
        }
        if preset
            .model
            .as_deref()
            .is_some_and(|model| model.trim().is_empty())
        {
            return Err(AixError::EmptyPromptField {
                name: name.to_string(),
                field: "model",
            });
        }
    }

    let mut policy_names: Vec<_> = config.run_policies.keys().collect();
    policy_names.sort_unstable();
    for name in policy_names {
        let policy = &config.run_policies[name];
        if name.trim().is_empty() {
            return Err(AixError::EmptyRunPolicyName);
        }
        if !policy.max_budget.is_finite() || policy.max_budget <= 0.0 {
            return Err(AixError::InvalidRunPolicyBudget { name: name.clone() });
        }
        if crate::duration::parse_litellm_duration(&policy.max_duration).is_none() {
            return Err(AixError::InvalidRunPolicyDuration { name: name.clone() });
        }
        if let Some(models) = &policy.allowed_models {
            if models.is_empty() {
                return Err(AixError::EmptyRunPolicyModels { name: name.clone() });
            }
            if models.iter().any(|model| model.trim().is_empty()) {
                return Err(AixError::EmptyRunPolicyModel { name: name.clone() });
            }
        }
        if policy.tags.iter().any(|tag| tag.trim().is_empty()) {
            return Err(AixError::EmptyRunPolicyTag { name: name.clone() });
        }
        if let Some(profile) = &policy.profile {
            if !config.profiles.contains_key(profile) {
                return Err(AixError::UnknownRunPolicyProfile {
                    name: name.clone(),
                    profile: profile.clone(),
                });
            }
        }
    }

    // Build the effective display label for every profile: explicit label or name as fallback.
    // The interactive selector uses this same set of labels; duplicates make selection ambiguous.
    let mut sorted_names: Vec<&str> = config.profiles.keys().map(String::as_str).collect();
    sorted_names.sort();
    let mut seen: std::collections::HashMap<String, &str> = std::collections::HashMap::new();
    for name in sorted_names {
        let profile = &config.profiles[name];
        validate_models(&profile.models, &format!("profiles.{name}.models"))?;
        match &profile.auth {
            ProfileAuth::ApiKey { .. }
                if profile.base_url.is_none() && config.endpoint.base_url.is_none() =>
            {
                return Err(AixError::MissingEndpointUrl);
            }
            ProfileAuth::ChatGpt if profile.base_url.is_some() => {
                return Err(AixError::ChatGptProfileBaseUrl {
                    name: name.to_string(),
                });
            }
            _ => {}
        }
        for env_name in profile.env.keys() {
            if !is_valid_env_name(env_name) {
                return Err(AixError::InvalidEnvironmentVariableName {
                    profile: name.to_string(),
                    name: env_name.clone(),
                });
            }
        }

        let effective_label: String = match &profile.label {
            None => name.to_string(),
            Some(dv) => dv.resolve()?,
        };
        if let Some(first) = seen.get(effective_label.as_str()) {
            return Err(AixError::DuplicateLabel {
                label: effective_label.clone(),
                first: first.to_string(),
                second: name.to_string(),
            });
        }
        seen.insert(effective_label, name);
    }

    let mut tool_names: Vec<_> = config.tools.keys().collect();
    tool_names.sort();
    for name in tool_names {
        let tool = &config.tools[name];
        if name.trim().is_empty() {
            return Err(AixError::EmptyToolName);
        }
        if tool
            .command
            .as_deref()
            .is_some_and(|command| command.trim().is_empty())
        {
            return Err(AixError::EmptyToolCommand { name: name.clone() });
        }
        for env_name in tool.env.keys() {
            if !is_valid_env_name(env_name) {
                return Err(AixError::InvalidToolEnvironmentVariableName {
                    tool: name.clone(),
                    name: env_name.clone(),
                });
            }
        }
        if let Some(chatgpt) = &tool.chatgpt {
            if !is_valid_env_name(&chatgpt.access_token_env) {
                return Err(AixError::InvalidToolEnvironmentVariableName {
                    tool: name.clone(),
                    name: chatgpt.access_token_env.clone(),
                });
            }
            for env_name in &chatgpt.clear_env {
                if !is_valid_env_name(env_name) {
                    return Err(AixError::InvalidToolEnvironmentVariableName {
                        tool: name.clone(),
                        name: env_name.clone(),
                    });
                }
            }
            if chatgpt
                .clear_env
                .iter()
                .any(|env_name| env_name == &chatgpt.access_token_env)
            {
                return Err(AixError::ChatGptAccessTokenEnvCleared {
                    tool: name.clone(),
                    name: chatgpt.access_token_env.clone(),
                });
            }
        }
    }

    Ok(())
}

fn validate_models(models: &ModelConfig, scope: &str) -> Result<(), AixError> {
    if models
        .default
        .as_deref()
        .is_some_and(|model| model.trim().is_empty())
    {
        return Err(AixError::EmptyModelDefault {
            scope: scope.to_string(),
        });
    }

    let mut aliases: Vec<_> = models.aliases.iter().collect();
    aliases.sort_by_key(|(alias, _)| *alias);
    for (alias, target) in aliases {
        if alias.trim().is_empty() {
            return Err(AixError::EmptyModelAliasName {
                scope: scope.to_string(),
            });
        }
        if target.trim().is_empty() {
            return Err(AixError::EmptyModelAliasTarget {
                scope: scope.to_string(),
                alias: alias.clone(),
            });
        }
    }

    Ok(())
}

/// Format a list of profile names (with optional label in parens) for use in error messages.
/// Shows the names users can type, e.g. "  swtb\n  work  (Work account)".
fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.bytes();
    matches!(chars.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && chars.all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_'))
}

pub fn format_available_profiles(cfg: &Config) -> String {
    let mut pairs: Vec<(&str, String)> = cfg
        .profiles
        .iter()
        .map(|(name, profile)| {
            let label = profile
                .label
                .as_ref()
                .and_then(|dv| dv.resolve().ok())
                .unwrap_or_else(|| name.to_string());
            (name.as_str(), label)
        })
        .collect();
    pairs.sort_by_key(|(name, _)| *name);
    if pairs.is_empty() {
        return "  (no profiles defined)".to_string();
    }
    pairs
        .iter()
        .map(|(name, label)| {
            if *name == label.as_str() {
                format!("  {name}")
            } else {
                format!("  {name} ({label})")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn sorted_profiles(cfg: &Config) -> Result<Vec<(&str, String)>, AixError> {
    let mut pairs: Vec<(&str, String)> = Vec::new();
    for (name, profile) in &cfg.profiles {
        let label = match &profile.label {
            None => name.to_string(),
            Some(dv) => dv.resolve()?,
        };
        pairs.push((name.as_str(), label));
    }
    pairs.sort_by_key(|(name, _)| *name);
    Ok(pairs)
}

pub fn sorted_prompt_names(cfg: &Config) -> Vec<&str> {
    let mut names: Vec<_> = cfg.prompts.keys().map(String::as_str).collect();
    names.sort_unstable();
    names
}

pub fn sorted_run_policy_names(cfg: &Config) -> Vec<&str> {
    let mut names: Vec<_> = cfg.run_policies.keys().map(String::as_str).collect();
    names.sort_unstable();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::SourceKind;

    const TOML: &str = r#"
default_profile = "work"

[models]
default = "global-model"

[models.aliases]
fast = "global-fast-model"

[endpoint]
base_url = { env = "AIX_BASE_URL" }
provider = "litellm"

[profiles.work]
label = "Work"
api_key = { env = "AIX_API_KEY" }

[profiles.work.models]
default = "work-model"

[profiles.work.models.aliases]
fast = "work-fast-model"

[profiles.local]
label = "Local"
api_key = "sk-local-key"
base_url = "https://local.example.com"
"#;

    const YAML: &str = r#"
default_profile: work
models:
  default: global-model
  aliases:
    fast: global-fast-model
endpoint:
  base_url:
    env: AIX_BASE_URL
  provider: litellm
profiles:
  work:
    label: Work
    api_key:
      env: AIX_API_KEY
    models:
      default: work-model
      aliases:
        fast: work-fast-model
  local:
    label: Local
    api_key: sk-local-key
    base_url: https://local.example.com
"#;

    const JSON: &str = r#"{
  "default_profile": "work",
  "models": {
    "default": "global-model",
    "aliases": { "fast": "global-fast-model" }
  },
  "endpoint": {
    "base_url": { "env": "AIX_BASE_URL" },
    "provider": "litellm"
  },
  "profiles": {
    "work": {
      "label": "Work",
      "api_key": { "env": "AIX_API_KEY" },
      "models": {
        "default": "work-model",
        "aliases": { "fast": "work-fast-model" }
      }
    },
    "local": { "label": "Local", "api_key": "sk-local-key", "base_url": "https://local.example.com" }
  }
}"#;

    const JSON5: &str = r#"{
  default_profile: "work",
  models: {
    default: "global-model",
    aliases: { fast: "global-fast-model" },
  },
  endpoint: {
    base_url: { env: "AIX_BASE_URL" },
    provider: "litellm",
  },
  profiles: {
    work: {
      label: "Work",
      api_key: { env: "AIX_API_KEY" },
      models: {
        default: "work-model",
        aliases: { fast: "work-fast-model" },
      },
    },
    local: { label: "Local", api_key: "sk-local-key", base_url: "https://local.example.com" },
  },
}"#;

    const MODEL_CONFIG: &str = r#"
[endpoint]
base_url = "https://example.com"

[models]
default = "global-model"

[models.aliases]
fast = "global-fast-model"
smart = "global-smart-model"

[profiles.work]
api_key = "sk-test"

[profiles.work.models]
default = "work-model"

[profiles.work.models.aliases]
fast = "work-fast-model"
"#;

    fn assert_standard(cfg: &Config) {
        assert_eq!(cfg.default_profile.as_deref(), Some("work"));
        assert_eq!(
            cfg.endpoint.provider,
            Some(Provider::Known(KnownProvider::LiteLlm))
        );
        assert!(matches!(
            cfg.endpoint.base_url.as_ref().unwrap().0,
            SourceKind::Env(_)
        ));
        assert!(cfg.profiles.contains_key("work"));
        assert!(cfg.profiles.contains_key("local"));
        assert_eq!(cfg.models.default.as_deref(), Some("global-model"));
        assert_eq!(
            cfg.models.aliases.get("fast").map(String::as_str),
            Some("global-fast-model")
        );
        assert_eq!(
            cfg.profiles["work"].models.default.as_deref(),
            Some("work-model")
        );
        assert_eq!(
            cfg.profiles["work"]
                .models
                .aliases
                .get("fast")
                .map(String::as_str),
            Some("work-fast-model")
        );
        assert!(matches!(
            &cfg.profiles["work"].auth,
            ProfileAuth::ApiKey { api_key } if matches!(api_key.0, SourceKind::Env(_))
        ));
        assert!(matches!(
            &cfg.profiles["local"].auth,
            ProfileAuth::ApiKey { api_key } if matches!(api_key.0, SourceKind::Direct(_))
        ));
        assert!(matches!(
            cfg.profiles["local"].base_url.as_ref().unwrap().0,
            SourceKind::Direct(_)
        ));
    }

    #[test]
    fn parse_toml() {
        let cfg: Config = toml::from_str(TOML).unwrap();
        assert_standard(&cfg);
    }

    #[test]
    fn parse_yaml() {
        let cfg: Config = serde_yaml::from_str(YAML).unwrap();
        assert_standard(&cfg);
    }

    #[test]
    fn parse_json() {
        let cfg: Config = serde_json::from_str(JSON).unwrap();
        assert_standard(&cfg);
    }

    #[test]
    fn parse_json5() {
        let cfg: Config = json5::from_str(JSON5).unwrap();
        assert_standard(&cfg);
    }

    #[test]
    fn chatgpt_profile_parses_in_all_supported_formats_without_an_endpoint() {
        let configs = [
            toml::from_str::<Config>(
                "[profiles.personal]\nlabel = 'Personal'\nauth = { type = 'chatgpt' }\n",
            )
            .unwrap(),
            serde_yaml::from_str::<Config>(
                "profiles:\n  personal:\n    label: Personal\n    auth:\n      type: chatgpt\n",
            )
            .unwrap(),
            serde_json::from_str::<Config>(
                r#"{"profiles":{"personal":{"label":"Personal","auth":{"type":"chatgpt"}}}}"#,
            )
            .unwrap(),
            json5::from_str::<Config>(
                "{profiles:{personal:{label:'Personal',auth:{type:'chatgpt'}}}}",
            )
            .unwrap(),
        ];

        for config in configs {
            validate(&config).unwrap();
            assert!(matches!(
                config.profiles["personal"].auth,
                ProfileAuth::ChatGpt
            ));
            assert!(config.endpoint.base_url.is_none());
        }
    }

    #[test]
    fn profile_requires_exactly_one_supported_auth_source() {
        for invalid in [
            "[profiles.work]",
            "[profiles.work]\napi_key = 'key'\nauth = { type = 'chatgpt' }",
            "[profiles.work]\nauth = { type = 'oauth' }",
        ] {
            assert!(
                toml::from_str::<Config>(invalid).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn chatgpt_profile_rejects_profile_gateway_override() {
        let cfg: Config = toml::from_str(
            "[profiles.personal]\nauth = { type = 'chatgpt' }\nbase_url = 'https://example.invalid'\n",
        )
        .unwrap();
        assert!(matches!(
            validate(&cfg),
            Err(AixError::ChatGptProfileBaseUrl { ref name }) if name == "personal"
        ));
    }

    #[test]
    fn chatgpt_tool_binding_parses_in_all_supported_formats() {
        let configs = [
            toml::from_str::<Config>(
                r#"
[profiles.personal]
auth = { type = "chatgpt" }
[tools.codex]
api_format = "openai"
[tools.codex.chatgpt]
access_token_env = "ACCESS_TOKEN"
prepend_args = ["app-server", "--listen", "stdio://"]
clear_env = ["OPENAI_API_KEY", "CODEX_API_KEY"]
"#,
            )
            .unwrap(),
            serde_yaml::from_str::<Config>(
                "profiles:\n  personal:\n    auth:\n      type: chatgpt\ntools:\n  codex:\n    api_format: openai\n    chatgpt:\n      access_token_env: ACCESS_TOKEN\n      prepend_args: [app-server, --listen, stdio://]\n      clear_env: [OPENAI_API_KEY, CODEX_API_KEY]\n",
            )
            .unwrap(),
            serde_json::from_str::<Config>(
                r#"{"profiles":{"personal":{"auth":{"type":"chatgpt"}}},"tools":{"codex":{"api_format":"openai","chatgpt":{"access_token_env":"ACCESS_TOKEN","prepend_args":["app-server","--listen","stdio://"],"clear_env":["OPENAI_API_KEY","CODEX_API_KEY"]}}}}"#,
            )
            .unwrap(),
            json5::from_str::<Config>(
                "{profiles:{personal:{auth:{type:'chatgpt'}}},tools:{codex:{api_format:'openai',chatgpt:{access_token_env:'ACCESS_TOKEN',prepend_args:['app-server','--listen','stdio://'],clear_env:['OPENAI_API_KEY','CODEX_API_KEY']}}}}",
            )
            .unwrap(),
        ];

        for config in configs {
            validate(&config).unwrap();
            let binding = config.tools["codex"].chatgpt.as_ref().unwrap();
            assert_eq!(binding.access_token_env, "ACCESS_TOKEN");
            assert_eq!(binding.prepend_args, ["app-server", "--listen", "stdio://"]);
            assert_eq!(binding.clear_env, ["OPENAI_API_KEY", "CODEX_API_KEY"]);
        }
    }

    #[test]
    fn chatgpt_tool_binding_rejects_invalid_or_conflicting_env_names() {
        for binding in [
            "access_token_env = ''",
            "access_token_env = 'NOT VALID'",
            "access_token_env = 'ACCESS_TOKEN'\nclear_env = ['NOT VALID']",
            "access_token_env = 'ACCESS_TOKEN'\nclear_env = ['ACCESS_TOKEN']",
        ] {
            let input = format!(
                "[profiles.personal]\nauth = {{ type = 'chatgpt' }}\n[tools.codex]\napi_format = 'openai'\n[tools.codex.chatgpt]\n{binding}\n"
            );
            let config: Config = toml::from_str(&input).unwrap();
            assert!(validate(&config).is_err(), "accepted binding: {binding}");
        }
    }

    #[test]
    fn prompt_presets_parse_in_all_supported_formats() {
        let toml = r#"
[endpoint]
base_url = "https://example.com"
[profiles.work]
api_key = "test-key"
[prompts.diagnose]
prompt = "Diagnose the supplied output."
system = "Separate evidence from inference."
model = "smart"
"#;
        let yaml = r#"
endpoint:
  base_url: https://example.com
profiles:
  work:
    api_key: test-key
prompts:
  diagnose:
    prompt: Diagnose the supplied output.
    system: Separate evidence from inference.
    model: smart
"#;
        let json = r#"{
  "endpoint": { "base_url": "https://example.com" },
  "profiles": { "work": { "api_key": "test-key" } },
  "prompts": {
    "diagnose": {
      "prompt": "Diagnose the supplied output.",
      "system": "Separate evidence from inference.",
      "model": "smart"
    }
  }
}"#;
        let json5 = r#"{
  endpoint: { base_url: "https://example.com" },
  profiles: { work: { api_key: "test-key" } },
  prompts: {
    diagnose: {
      prompt: "Diagnose the supplied output.",
      system: "Separate evidence from inference.",
      model: "smart",
    },
  },
}"#;

        for cfg in [
            toml::from_str::<Config>(toml).unwrap(),
            serde_yaml::from_str::<Config>(yaml).unwrap(),
            serde_json::from_str::<Config>(json).unwrap(),
            json5::from_str::<Config>(json5).unwrap(),
        ] {
            let preset = &cfg.prompts["diagnose"];
            assert_eq!(preset.prompt, "Diagnose the supplied output.");
            assert_eq!(
                preset.system.as_deref(),
                Some("Separate evidence from inference.")
            );
            assert_eq!(preset.model.as_deref(), Some("smart"));
            assert!(validate(&cfg).is_ok());
        }
    }

    #[test]
    fn model_settings_are_optional_for_existing_configs() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = "https://example.com"
[profiles.work]
api_key = "sk-test"
"#,
        )
        .unwrap();
        assert_eq!(cfg.models, ModelConfig::default());
        assert_eq!(cfg.profiles["work"].models, ModelConfig::default());
    }

    #[test]
    fn explicit_raw_model_id_is_returned_unchanged() {
        let cfg: Config = toml::from_str(MODEL_CONFIG).unwrap();
        let profile = &cfg.profiles["work"];
        assert_eq!(
            resolve_model(Some("provider/model-v2"), &cfg, profile).unwrap(),
            "provider/model-v2"
        );
    }

    #[test]
    fn explicit_top_level_alias_resolves_to_model_id() {
        let cfg: Config = toml::from_str(MODEL_CONFIG).unwrap();
        let profile = &cfg.profiles["work"];
        assert_eq!(
            resolve_model(Some("smart"), &cfg, profile).unwrap(),
            "global-smart-model"
        );
    }

    #[test]
    fn profile_alias_shadows_top_level_alias() {
        let cfg: Config = toml::from_str(MODEL_CONFIG).unwrap();
        let profile = &cfg.profiles["work"];
        assert_eq!(
            resolve_model(Some("fast"), &cfg, profile).unwrap(),
            "work-fast-model"
        );
    }

    #[test]
    fn profile_default_overrides_top_level_default() {
        let cfg: Config = toml::from_str(MODEL_CONFIG).unwrap();
        let profile = &cfg.profiles["work"];
        assert_eq!(resolve_model(None, &cfg, profile).unwrap(), "work-model");
    }

    #[test]
    fn resolve_base_url_prefers_profile_override() {
        let cfg: Config = toml::from_str(TOML).unwrap();
        let profile = &cfg.profiles["local"];
        assert_eq!(
            resolve_base_url(profile, &cfg.endpoint)
                .unwrap()
                .expose_secret(),
            "https://local.example.com"
        );
    }

    #[test]
    fn top_level_default_is_used_when_profile_has_none() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = "https://example.com"
[models]
default = "global-model"
[profiles.work]
api_key = "sk-test"
"#,
        )
        .unwrap();
        let profile = &cfg.profiles["work"];
        assert_eq!(resolve_model(None, &cfg, profile).unwrap(), "global-model");
    }

    #[test]
    fn resolve_base_url_falls_back_to_endpoint() {
        let cfg: Config = toml::from_str(TOML).unwrap();
        let profile = &cfg.profiles["work"];
        std::env::set_var("AIX_BASE_URL", "https://shared.example.com");
        let result = resolve_base_url(profile, &cfg.endpoint);
        std::env::remove_var("AIX_BASE_URL");
        assert_eq!(
            result.unwrap().expose_secret(),
            "https://shared.example.com"
        );
    }

    #[test]
    fn model_defaults_are_raw_ids_and_not_resolved_as_aliases() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = "https://example.com"
[models]
default = "fast"
[models.aliases]
fast = "expanded-model-id"
[profiles.work]
api_key = "sk-test"
"#,
        )
        .unwrap();
        let profile = &cfg.profiles["work"];
        assert_eq!(resolve_model(None, &cfg, profile).unwrap(), "fast");
    }

    #[test]
    fn missing_model_default_has_actionable_error() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = "https://example.com"
[profiles.work]
api_key = "sk-test"
"#,
        )
        .unwrap();
        let profile = &cfg.profiles["work"];
        let err = resolve_model(None, &cfg, profile).unwrap_err();
        assert!(matches!(err, AixError::NoModelConfigured));
        let message = err.to_string();
        assert!(message.contains("--model <MODEL>"), "got: {message}");
        assert!(message.contains("[models].default"), "got: {message}");
    }

    #[test]
    fn parse_toml_unknown_field_rejected() {
        let bad = TOML.to_string() + "\nunknown_key = true\n";
        assert!(toml::from_str::<Config>(&bad).is_err());
    }

    #[test]
    fn find_explicit_path_returns_it() {
        let p = std::path::Path::new("/any/path.toml");
        let result = find_config_path(Some(p)).unwrap();
        assert_eq!(result, Some(p.to_path_buf()));
    }

    #[test]
    fn find_none_does_not_panic() {
        // Platform dir search shouldn't panic even if dir doesn't exist.
        let _ = find_config_path(None);
    }

    #[test]
    fn default_config_dir_is_aix_subdirectory_of_platform_config_dir() {
        let Some(base_dirs) = BaseDirs::new() else {
            return;
        };
        assert_eq!(
            default_config_dir(),
            Some(base_dirs.config_dir().join("aix"))
        );
    }

    #[test]
    fn load_toml_file() {
        use assert_fs::prelude::*;
        let file = assert_fs::NamedTempFile::new("aix.toml").unwrap();
        file.write_str(TOML).unwrap();
        let cfg = load(file.path()).unwrap();
        assert_eq!(cfg.default_profile.as_deref(), Some("work"));
    }

    #[test]
    fn load_yaml_file() {
        use assert_fs::prelude::*;
        let file = assert_fs::NamedTempFile::new("aix.yaml").unwrap();
        file.write_str(YAML).unwrap();
        let cfg = load(file.path()).unwrap();
        assert_eq!(cfg.default_profile.as_deref(), Some("work"));
    }

    #[test]
    fn load_json_file() {
        use assert_fs::prelude::*;
        let file = assert_fs::NamedTempFile::new("aix.json").unwrap();
        file.write_str(JSON).unwrap();
        let cfg = load(file.path()).unwrap();
        assert_eq!(cfg.default_profile.as_deref(), Some("work"));
    }

    #[test]
    fn load_json5_file() {
        use assert_fs::prelude::*;
        let file = assert_fs::NamedTempFile::new("aix.json5").unwrap();
        file.write_str(JSON5).unwrap();
        let cfg = load(file.path()).unwrap();
        assert_eq!(cfg.default_profile.as_deref(), Some("work"));
    }

    #[test]
    fn load_unknown_extension_errors() {
        use assert_fs::prelude::*;
        let file = assert_fs::NamedTempFile::new("aix.xyz").unwrap();
        file.write_str("").unwrap();
        let err = load(file.path()).unwrap_err();
        assert!(matches!(err, AixError::UnknownFormat { .. }));
    }

    #[test]
    fn load_missing_file_errors() {
        let result = load(std::path::Path::new("/nonexistent/aix.toml"));
        assert!(matches!(result, Err(AixError::ParseError { .. })));
    }

    #[test]
    fn validate_passes_on_well_formed_config() {
        let cfg: Config = toml::from_str(TOML).unwrap();
        assert!(validate(&cfg).is_ok());
    }

    #[test]
    fn validate_rejects_empty_global_model_default() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = "https://example.com"
[models]
default = ""
[profiles.work]
api_key = "sk-test"
"#,
        )
        .unwrap();
        assert!(matches!(
            validate(&cfg),
            Err(AixError::EmptyModelDefault { ref scope }) if scope == "models"
        ));
    }

    #[test]
    fn validate_rejects_empty_profile_model_default() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = "https://example.com"
[profiles.work]
api_key = "sk-test"
[profiles.work.models]
default = "   "
"#,
        )
        .unwrap();
        assert!(matches!(
            validate(&cfg),
            Err(AixError::EmptyModelDefault { ref scope }) if scope == "profiles.work.models"
        ));
    }

    #[test]
    fn validate_rejects_empty_model_alias_name() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = "https://example.com"
[models.aliases]
"" = "model-id"
[profiles.work]
api_key = "sk-test"
"#,
        )
        .unwrap();
        assert!(matches!(
            validate(&cfg),
            Err(AixError::EmptyModelAliasName { ref scope }) if scope == "models"
        ));
    }

    #[test]
    fn validate_rejects_empty_model_alias_target() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = "https://example.com"
[models.aliases]
fast = "  "
[profiles.work]
api_key = "sk-test"
"#,
        )
        .unwrap();
        assert!(matches!(
            validate(&cfg),
            Err(AixError::EmptyModelAliasTarget { ref scope, ref alias })
                if scope == "models" && alias == "fast"
        ));
    }

    #[test]
    fn validate_fails_when_default_profile_missing() {
        let bad = r#"
default_profile = "nonexistent"
[endpoint]
base_url = { env = "X" }
[profiles.work]
api_key = "sk-test"
"#;
        let cfg: Config = toml::from_str(bad).unwrap();
        let err = validate(&cfg).unwrap_err();
        assert!(
            matches!(err, AixError::UnknownDefaultProfile(ref name) if name == "nonexistent"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_passes_when_no_default_profile() {
        let no_default = r#"
[endpoint]
base_url = { env = "X" }
[profiles.work]
api_key = "sk-test"
"#;
        let cfg: Config = toml::from_str(no_default).unwrap();
        assert!(validate(&cfg).is_ok());
    }

    #[test]
    fn validate_rejects_invalid_profile_env_name() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = "https://example.com"
[profiles.work]
api_key = "sk-test"
[profiles.work.env]
"NOT VALID" = "value"
"#,
        )
        .unwrap();
        let err = validate(&cfg).unwrap_err();
        assert!(
            matches!(err, AixError::InvalidEnvironmentVariableName { ref profile, ref name }
                if profile == "work" && name == "NOT VALID"),
            "unexpected error: {err}"
        );
    }

    // --- env_files ---

    #[test]
    fn env_files_empty_by_default() {
        let cfg: Config = toml::from_str(TOML).unwrap();
        assert!(cfg.env_files.is_empty());
    }

    #[test]
    fn env_files_parsed() {
        // env_files must appear before section headers so TOML assigns it to
        // the top-level table, not to the last open section.
        let with_env = r#"
env_files = ["~/.config/aix/secrets.env", "/run/secrets/extra.env"]

[endpoint]
base_url = { env = "X" }

[profiles.work]
api_key = "sk-test"
"#;
        let cfg: Config = toml::from_str(with_env).unwrap();
        assert_eq!(cfg.env_files.len(), 2);
    }

    #[test]
    fn load_env_files_loads_values() {
        use assert_fs::prelude::*;
        let tmp = assert_fs::NamedTempFile::new("test.env").unwrap();
        tmp.write_str("AIX_TEST_ENVFILE_LOAD_A3B4=from-file\n")
            .unwrap();

        // Ensure the var isn't already set.
        std::env::remove_var("AIX_TEST_ENVFILE_LOAD_A3B4");

        let cfg = Config {
            default_profile: None,
            env_files: vec![tmp.path().to_path_buf()],
            endpoint: toml::from_str::<Config>(TOML).unwrap().endpoint,
            profiles: Default::default(),
            cache: Default::default(),
            models: Default::default(),
            prompts: Default::default(),
            tools: Default::default(),
            run_policies: Default::default(),
        };
        load_env_files(&cfg).unwrap();
        assert_eq!(
            std::env::var("AIX_TEST_ENVFILE_LOAD_A3B4").unwrap(),
            "from-file"
        );
    }

    #[test]
    fn load_env_files_real_env_wins() {
        use assert_fs::prelude::*;
        let tmp = assert_fs::NamedTempFile::new("test.env").unwrap();
        tmp.write_str("AIX_TEST_ENVFILE_WIN_C5D6=from-file\n")
            .unwrap();

        std::env::set_var("AIX_TEST_ENVFILE_WIN_C5D6", "from-process");

        let cfg = Config {
            default_profile: None,
            env_files: vec![tmp.path().to_path_buf()],
            endpoint: toml::from_str::<Config>(TOML).unwrap().endpoint,
            profiles: Default::default(),
            cache: Default::default(),
            models: Default::default(),
            prompts: Default::default(),
            tools: Default::default(),
            run_policies: Default::default(),
        };
        load_env_files(&cfg).unwrap();
        // Process env must win over file value.
        assert_eq!(
            std::env::var("AIX_TEST_ENVFILE_WIN_C5D6").unwrap(),
            "from-process"
        );
    }

    #[test]
    fn compat_section_now_rejected() {
        let bad = r#"
[compat]
anthropic_env = true

[endpoint]
base_url = { env = "X" }

[profiles.work]
api_key = "sk-test"
"#;
        assert!(toml::from_str::<Config>(bad).is_err());
    }

    #[test]
    fn load_env_files_missing_file_errors() {
        let cfg = Config {
            default_profile: None,
            env_files: vec![PathBuf::from("/nonexistent/aix-secrets-xyz.env")],
            endpoint: toml::from_str::<Config>(TOML).unwrap().endpoint,
            profiles: Default::default(),
            cache: Default::default(),
            models: Default::default(),
            prompts: Default::default(),
            tools: Default::default(),
            run_policies: Default::default(),
        };
        let err = load_env_files(&cfg).unwrap_err();
        assert!(matches!(err, AixError::EnvFileLoad { .. }));
        let msg = err.to_string();
        assert!(msg.contains("aix-secrets-xyz.env"));
    }

    // --- sorted_profiles ---

    #[test]
    fn sorted_profiles_sorted_by_name() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = { env = "X" }

[profiles.zzz]
label = "Z profile"
api_key = "sk-z"

[profiles.aaa]
label = "A profile"
api_key = "sk-a"
"#,
        )
        .unwrap();
        let pairs = sorted_profiles(&cfg).unwrap();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "aaa");
        assert_eq!(pairs[1].0, "zzz");
    }

    #[test]
    fn sorted_profiles_label_falls_back_to_name() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = { env = "X" }

[profiles.myprofile]
api_key = "sk-test"
"#,
        )
        .unwrap();
        let pairs = sorted_profiles(&cfg).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "myprofile");
        assert_eq!(pairs[0].1, "myprofile");
    }

    #[test]
    fn sorted_profiles_uses_label_when_set() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = { env = "X" }

[profiles.work]
label = "Work account"
api_key = "sk-work"
"#,
        )
        .unwrap();
        let pairs = sorted_profiles(&cfg).unwrap();
        assert_eq!(pairs[0].1, "Work account");
    }

    // --- duplicate label validation ---

    #[test]
    fn validate_fails_on_duplicate_labels() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = { env = "X" }

[profiles.alpha]
label = "Shared"
api_key = "sk-a"

[profiles.beta]
label = "Shared"
api_key = "sk-b"
"#,
        )
        .unwrap();
        let err = validate(&cfg).unwrap_err();
        assert!(
            matches!(err, AixError::DuplicateLabel { ref label, ref first, ref second }
                if label == "Shared" && first == "alpha" && second == "beta"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_passes_when_labels_are_distinct() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = { env = "X" }

[profiles.a]
label = "Alpha"
api_key = "sk-a"

[profiles.b]
label = "Beta"
api_key = "sk-b"
"#,
        )
        .unwrap();
        assert!(validate(&cfg).is_ok());
    }

    // --- Provider ---

    #[test]
    fn provider_known_parses() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = { env = "X" }
provider = "litellm"
[profiles.work]
api_key = "sk-test"
"#,
        )
        .unwrap();
        assert_eq!(
            cfg.endpoint.provider,
            Some(Provider::Known(KnownProvider::LiteLlm))
        );
    }

    #[test]
    fn provider_custom_parses() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = { env = "X" }
provider = "my-future-provider"
[profiles.work]
api_key = "sk-test"
"#,
        )
        .unwrap();
        assert!(
            matches!(cfg.endpoint.provider, Some(Provider::Custom(ref s)) if s == "my-future-provider")
        );
    }

    #[test]
    fn provider_absent_is_none() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = { env = "X" }
[profiles.work]
api_key = "sk-test"
"#,
        )
        .unwrap();
        assert_eq!(cfg.endpoint.provider, None);
    }

    // --- Gateway ---

    #[test]
    fn gateway_known_parses() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = { env = "X" }
gateway = "litellm"
[profiles.work]
api_key = "sk-test"
"#,
        )
        .unwrap();
        assert_eq!(
            cfg.endpoint.gateway,
            Some(Gateway::Known(KnownGateway::Litellm))
        );
    }

    #[test]
    fn gateway_custom_parses() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = { env = "X" }
gateway = "my-custom-gateway"
[profiles.work]
api_key = "sk-test"
"#,
        )
        .unwrap();
        assert!(
            matches!(cfg.endpoint.gateway, Some(Gateway::Custom(ref s)) if s == "my-custom-gateway")
        );
    }

    #[test]
    fn gateway_absent_is_none() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = { env = "X" }
[profiles.work]
api_key = "sk-test"
"#,
        )
        .unwrap();
        assert_eq!(cfg.endpoint.gateway, None);
    }

    // --- missing / empty profiles ---

    #[test]
    fn missing_profiles_section_parses_as_empty_map() {
        let toml = r#"
[endpoint]
base_url = "https://example.com"
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(cfg.profiles.is_empty());
    }

    #[test]
    fn validate_fails_when_profiles_is_empty() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = "https://example.com"
"#,
        )
        .unwrap();
        let err = validate(&cfg).unwrap_err();
        assert!(
            matches!(err, AixError::NoProfilesConfigured),
            "unexpected error: {err}"
        );
        assert!(err.to_string().contains("[profiles]"), "got: {err}");
    }

    // --- format_available_profiles spacing ---

    #[test]
    fn format_available_profiles_single_space_before_label_paren() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = "https://example.com"
[profiles.swtb]
label = "SWTB"
api_key = "sk-a"
[profiles.work]
label = "Work account"
api_key = "sk-b"
"#,
        )
        .unwrap();
        let out = format_available_profiles(&cfg);
        assert!(
            out.contains("swtb (SWTB)"),
            "expected single space, got:\n{out}"
        );
        assert!(
            out.contains("work (Work account)"),
            "expected single space, got:\n{out}"
        );
        assert!(!out.contains("swtb  ("), "found double space, got:\n{out}");
    }

    #[test]
    fn format_available_profiles_omits_parens_when_label_equals_name() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = "https://example.com"
[profiles.swtb]
label = "swtb"
api_key = "sk-a"
"#,
        )
        .unwrap();
        let out = format_available_profiles(&cfg);
        assert!(out.contains("swtb"), "should show name: {out}");
        assert!(
            !out.contains('('),
            "should not show parens when label == name: {out}"
        );
    }

    #[test]
    fn format_available_profiles_omits_parens_when_no_label() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = "https://example.com"
[profiles.swtb]
api_key = "sk-a"
"#,
        )
        .unwrap();
        let out = format_available_profiles(&cfg);
        assert!(out.contains("swtb"), "should show name: {out}");
        assert!(
            !out.contains('('),
            "should not show parens when no label: {out}"
        );
    }

    #[test]
    fn validate_passes_when_all_labels_none_and_names_distinct() {
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = { env = "X" }

[profiles.a]
api_key = "sk-a"

[profiles.b]
api_key = "sk-b"
"#,
        )
        .unwrap();
        assert!(validate(&cfg).is_ok());
    }

    #[test]
    fn validate_fails_when_explicit_label_clashes_with_fallback_name_label() {
        // Profile "other" has label = "work", which is the same as profile "work"'s
        // fallback display label (its own name). The interactive selector would show
        // two "work" options and pick the wrong one.
        let cfg: Config = toml::from_str(
            r#"
[endpoint]
base_url = { env = "X" }

[profiles.work]
api_key = "sk-work"

[profiles.other]
label = "work"
api_key = "sk-other"
"#,
        )
        .unwrap();
        let err = validate(&cfg).unwrap_err();
        assert!(
            matches!(err, AixError::DuplicateLabel { ref label, .. } if label == "work"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn sorted_profiles_with_env_label() {
        let var = "AIX_TEST_SORTED_PROFILES_LABEL_A1B2";
        std::env::set_var(var, "My Work Label");
        let cfg: Config = toml::from_str(&format!(
            r#"
[endpoint]
base_url = {{ env = "X" }}
[profiles.work]
label = {{ env = "{var}" }}
api_key = "sk-test"
"#
        ))
        .unwrap();
        let pairs = sorted_profiles(&cfg);
        std::env::remove_var(var);
        let pairs = pairs.unwrap();
        assert_eq!(pairs[0].1, "My Work Label");
    }

    #[test]
    fn sorted_profiles_missing_env_label_returns_err() {
        let var = "AIX_TEST_SORTED_PROFILES_LABEL_MISSING_C3D4";
        std::env::remove_var(var);
        let cfg: Config = toml::from_str(&format!(
            r#"
[endpoint]
base_url = {{ env = "X" }}
[profiles.work]
label = {{ env = "{var}" }}
api_key = "sk-test"
"#
        ))
        .unwrap();
        assert!(sorted_profiles(&cfg).is_err());
    }

    #[test]
    fn validate_surfaces_bad_label_source() {
        let var = "AIX_TEST_VALIDATE_LABEL_MISSING_E5F6";
        std::env::remove_var(var);
        let cfg: Config = toml::from_str(&format!(
            r#"
[endpoint]
base_url = {{ env = "X" }}
[profiles.work]
label = {{ env = "{var}" }}
api_key = "sk-test"
"#
        ))
        .unwrap();
        let err = validate(&cfg).unwrap_err();
        assert!(
            matches!(err, AixError::SecretMissingEnvVar { .. }),
            "expected SecretMissingEnvVar, got: {err}"
        );
    }

    #[test]
    fn format_available_profiles_falls_back_on_bad_label() {
        let var = "AIX_TEST_FORMAT_PROFILES_LABEL_BAD_G7H8";
        std::env::remove_var(var);
        let cfg: Config = toml::from_str(&format!(
            r#"
[endpoint]
base_url = {{ env = "X" }}
[profiles.work]
label = {{ env = "{var}" }}
api_key = "sk-test"
"#
        ))
        .unwrap();
        let out = format_available_profiles(&cfg);
        assert!(
            out.contains("work"),
            "should fall back to profile name: {out}"
        );
    }

    // --- CacheConfig ---

    #[test]
    fn cache_config_defaults_when_section_absent() {
        let cfg: Config = toml::from_str(TOML).unwrap();
        assert_eq!(cfg.cache.ttl_secs, 3600);
        assert!(!cfg.cache.disabled);
    }

    #[test]
    fn cache_config_parses_full_section() {
        let toml = r#"
[endpoint]
base_url = { env = "X" }
[profiles.work]
api_key = "sk-test"
[cache]
ttl_secs = 600
disabled = true
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.cache.ttl_secs, 600);
        assert!(cfg.cache.disabled);
    }

    #[test]
    fn cache_config_partial_section_uses_field_defaults() {
        let toml = r#"
[endpoint]
base_url = { env = "X" }
[profiles.work]
api_key = "sk-test"
[cache]
disabled = true
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.cache.ttl_secs, 3600);
        assert!(cfg.cache.disabled);
    }

    #[test]
    fn cache_config_rejects_unknown_fields() {
        let toml = r#"
[endpoint]
base_url = { env = "X" }
[profiles.work]
api_key = "sk-test"
[cache]
unknown_key = "bad"
"#;
        assert!(toml::from_str::<Config>(toml).is_err());
    }
}
