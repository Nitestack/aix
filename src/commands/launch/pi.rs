use super::LaunchEnv;
use crate::config::{self, PiToolConfig};
use crate::error::AixError;
use color_eyre::eyre::{eyre, WrapErr};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::{Builder, TempDir};

const AGENT_DIR_ENV: &str = "PI_CODING_AGENT_DIR";
const STORED_DATA_FILES: [&str; 3] = ["auth.json", "mcp-auth.json", "trust.json"];
const SHARED_PACKAGE_DIRS: [&str; 2] = ["npm", "git"];

pub(super) struct PiAgentConfigLaunch {
    profile: String,
    source: Option<PathBuf>,
    configured_source: PathBuf,
    source_id: String,
    api_format: config::ApiFormat,
    stage: Option<TempDir>,
}

impl PiAgentConfigLaunch {
    pub(super) fn resolve(
        profile: &str,
        config_path: &Path,
        config: &PiToolConfig,
        api_format: config::ApiFormat,
    ) -> color_eyre::Result<Self> {
        let configured_source = resolve_agent_dir(&config.agent_dir, config_path)?;
        let source_id = format!("profiles.{profile}.tool_configs.pi.agent_dir");
        let source = match inspect_agent_dir(&configured_source) {
            Ok(()) => Some(configured_source.clone()),
            Err(reason) => {
                warn_unavailable(profile, &source_id, &configured_source, &reason);
                None
            }
        };

        Ok(Self {
            profile: profile.to_string(),
            source,
            configured_source,
            source_id,
            api_format,
            stage: None,
        })
    }

    pub(super) fn validate_arguments(&self, args: &[String]) -> color_eyre::Result<()> {
        if self.source.is_none() {
            return Ok(());
        }

        for arg in args {
            if arg == "--agent-dir" || arg.starts_with("--agent-dir=") {
                return Err(AixError::PiAgentConfigArgumentConflict {
                    option: "--agent-dir",
                }
                .into());
            }
            if arg == "--api-key" || arg.starts_with("--api-key=") {
                return Err(AixError::PiAgentConfigArgumentConflict {
                    option: "--api-key",
                }
                .into());
            }
        }

        Ok(())
    }

    pub(super) fn print_dry_run(&self) {
        if self.source.is_some() {
            eprintln!(
                "Pi agent configuration: profile {:?}, tool \"pi\", source {} ({})",
                self.profile,
                self.source_id,
                self.configured_source.display()
            );
        } else {
            eprintln!(
                "Pi agent configuration: profile {:?}, tool \"pi\", source {} unavailable; using standard configuration",
                self.profile, self.source_id
            );
        }
    }

    pub(super) fn prepare(&mut self, env: &mut LaunchEnv) -> color_eyre::Result<()> {
        let Some(source) = self.source.as_ref() else {
            return Ok(());
        };

        let standard_agent_dir = standard_agent_dir()?;
        let temp = Builder::new()
            .prefix("aix-pi-agent-")
            .tempdir()
            .wrap_err("create temporary Pi agent configuration directory")?;
        let staged_agent_dir = temp.path().join("agent");
        fs::create_dir(&staged_agent_dir)
            .wrap_err("create staged Pi agent configuration directory")?;

        if let Err(error) = stage_agent_config(
            source,
            &staged_agent_dir,
            &standard_agent_dir,
            self.api_format,
            env,
        ) {
            match error {
                StageError::Source(error) => {
                    let reason = format!("cannot read selected directory ({error})");
                    warn_unavailable(
                        &self.profile,
                        &self.source_id,
                        &self.configured_source,
                        &reason,
                    );
                    self.source = None;
                    return Ok(());
                }
                StageError::Destination(error) => {
                    return Err(error).wrap_err("stage selected Pi agent configuration");
                }
            }
        }

        prepare_shared_store(&standard_agent_dir)
            .wrap_err("prepare shared Pi user data directories")?;
        add_environment_override(env, AGENT_DIR_ENV, &staged_agent_dir);
        self.stage = Some(temp);
        Ok(())
    }

    pub(super) fn add_runtime_arguments(
        &self,
        args: &[String],
        env: &LaunchEnv,
    ) -> color_eyre::Result<Vec<String>> {
        if self.source.is_none() {
            return Ok(args.to_vec());
        }

        if args.first().is_some_and(|arg| arg == "auth") {
            return Ok(args.to_vec());
        }

        let key_name = match self.api_format {
            config::ApiFormat::Anthropic => "ANTHROPIC_API_KEY",
            config::ApiFormat::OpenAi | config::ApiFormat::Both => "OPENAI_API_KEY",
        };
        let api_key = effective_value(key_name, &env.vars).ok_or_else(|| {
            eyre!("Pi profile configuration requires {key_name}, but the launch environment did not provide it")
        })?;

        let mut result = Vec::with_capacity(args.len() + 2);
        result.extend(["--api-key".to_string(), api_key]);
        result.extend_from_slice(args);
        Ok(result)
    }
}

fn resolve_agent_dir(configured: &Path, config_path: &Path) -> color_eyre::Result<PathBuf> {
    let configured = configured.to_string_lossy();
    let expanded = shellexpand::tilde(&configured);
    let path = PathBuf::from(expanded.as_ref());
    if path.is_absolute() {
        return Ok(path);
    }

    let config_parent = config_path.parent().unwrap_or_else(|| Path::new("."));
    let config_parent = if config_parent.is_absolute() {
        config_parent.to_path_buf()
    } else {
        env::current_dir()?.join(config_parent)
    };
    Ok(normalize_path(&config_parent.join(path)))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                normalized.push(component.as_os_str());
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if normalized
                    .components()
                    .next_back()
                    .is_some_and(|last| matches!(last, std::path::Component::Normal(_)))
                {
                    normalized.pop();
                } else if !normalized.has_root() {
                    normalized.push("..");
                }
            }
            std::path::Component::Normal(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn inspect_agent_dir(path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => "directory does not exist".to_string(),
        _ => format!("directory cannot be inspected ({error})"),
    })?;
    if !metadata.is_dir() {
        return Err("path is not a directory".to_string());
    }
    fs::read_dir(path).map_err(|error| format!("directory is not readable ({error})"))?;
    Ok(())
}

fn warn_unavailable(profile: &str, source_id: &str, path: &Path, reason: &str) {
    eprintln!(
        "warning: profile {profile:?}, tool \"pi\": selected agent directory from {source_id} ({}) is unavailable ({reason}); using standard Pi configuration",
        path.display()
    );
}

fn standard_agent_dir() -> color_eyre::Result<PathBuf> {
    if let Some(path) = env::var_os(AGENT_DIR_ENV) {
        let path = path.to_string_lossy();
        let expanded = shellexpand::tilde(&path);
        let path = PathBuf::from(expanded.as_ref());
        return if path.is_absolute() {
            Ok(path)
        } else {
            Ok(env::current_dir()?.join(path))
        };
    }

    let home = directories::BaseDirs::new()
        .ok_or_else(|| eyre!("could not determine the user's home directory for Pi data"))?;
    Ok(home.home_dir().join(".pi").join("agent"))
}

fn effective_value(name: &str, vars: &[(String, String)]) -> Option<String> {
    vars.iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
        .or_else(|| env::var(name).ok())
}

fn add_environment_override(env: &mut LaunchEnv, name: &str, value: &Path) {
    env.vars.retain(|(key, _)| key != name);
    env.vars
        .push((name.to_string(), value.to_string_lossy().into_owned()));
}

#[derive(Debug)]
enum StageError {
    Source(std::io::Error),
    Destination(std::io::Error),
}

fn stage_agent_config(
    source: &Path,
    destination: &Path,
    standard_agent_dir: &Path,
    api_format: config::ApiFormat,
    env: &LaunchEnv,
) -> Result<(), StageError> {
    let entries = fs::read_dir(source).map_err(StageError::Source)?;
    let endpoint_overrides = endpoint_overrides(api_format, &env.vars);
    let mut has_models_file = false;

    for entry in entries {
        let entry = entry.map_err(StageError::Source)?;
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        if STORED_DATA_FILES.contains(&name_text.as_ref()) || name_text == "sessions" {
            continue;
        }
        if SHARED_PACKAGE_DIRS.contains(&name_text.as_ref()) {
            continue;
        }

        let source_path = entry.path();
        let destination_path = destination.join(&name);
        if name_text == "models.json" {
            has_models_file = true;
            let metadata = fs::metadata(&source_path).map_err(StageError::Source)?;
            if metadata.is_file() {
                let bytes = fs::read(&source_path).map_err(StageError::Source)?;
                let rewritten = rewrite_models(&bytes, &endpoint_overrides);
                fs::write(destination_path, rewritten).map_err(StageError::Destination)?;
                continue;
            }
        } else if name_text == "settings.json" {
            let metadata = fs::metadata(&source_path).map_err(StageError::Source)?;
            if metadata.is_file() {
                let bytes = fs::read(&source_path).map_err(StageError::Source)?;
                let rewritten = rewrite_relative_resources(&bytes, source);
                fs::write(destination_path, rewritten).map_err(StageError::Destination)?;
                continue;
            }
        }

        let mut ancestors = HashSet::new();
        copy_entry(&source_path, &destination_path, &mut ancestors)?;
    }

    if !has_models_file && !endpoint_overrides.is_empty() {
        let mut providers = Map::new();
        for (provider, base_url) in endpoint_overrides {
            providers.insert(
                provider.to_string(),
                serde_json::json!({ "baseUrl": base_url }),
            );
        }
        let models = serde_json::json!({ "providers": providers });
        let mut bytes = serde_json::to_vec_pretty(&models)
            .map_err(|error| StageError::Destination(std::io::Error::other(error.to_string())))?;
        bytes.push(b'\n');
        fs::write(destination.join("models.json"), bytes).map_err(StageError::Destination)?;
    }

    for name in STORED_DATA_FILES {
        link_path(
            &standard_agent_dir.join(name),
            &destination.join(name),
            false,
        )
        .map_err(StageError::Destination)?;
    }
    for name in SHARED_PACKAGE_DIRS {
        link_path(
            &standard_agent_dir.join(name),
            &destination.join(name),
            true,
        )
        .map_err(StageError::Destination)?;
    }
    // The default session path remains in the original store through this link,
    // without overriding Pi's native settings.json or project sessionDir settings.
    link_path(
        &standard_agent_dir.join("sessions"),
        &destination.join("sessions"),
        true,
    )
    .map_err(StageError::Destination)?;

    Ok(())
}

fn endpoint_overrides(
    api_format: config::ApiFormat,
    vars: &[(String, String)],
) -> Vec<(&'static str, String)> {
    let mut providers = Vec::new();
    if api_format.supports_openai() {
        if let Some(url) = effective_value("OPENAI_BASE_URL", vars) {
            providers.push(("openai", url));
        }
    }
    if api_format.supports_anthropic() {
        if let Some(url) = effective_value("ANTHROPIC_BASE_URL", vars) {
            providers.push(("anthropic", url));
        }
    }
    providers
}

fn rewrite_models(bytes: &[u8], endpoints: &[(&str, String)]) -> Vec<u8> {
    let Ok(mut config) = serde_json::from_slice::<Value>(bytes) else {
        return bytes.to_vec();
    };
    let Some(root) = config.as_object_mut() else {
        return bytes.to_vec();
    };
    let providers = root
        .entry("providers")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(providers) = providers.as_object_mut() else {
        return bytes.to_vec();
    };

    for (provider_name, base_url) in endpoints {
        let provider = providers
            .entry((*provider_name).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(provider) = provider.as_object_mut() else {
            continue;
        };
        provider.insert("baseUrl".to_string(), Value::String(base_url.clone()));
        if let Some(models) = provider.get_mut("models").and_then(Value::as_array_mut) {
            for model in models {
                if let Some(model) = model.as_object_mut() {
                    model.insert("baseUrl".to_string(), Value::String(base_url.clone()));
                }
            }
        }
    }

    serialize_json(config, bytes)
}

fn rewrite_relative_resources(bytes: &[u8], agent_dir: &Path) -> Vec<u8> {
    let Ok(mut settings) = serde_json::from_slice::<Value>(bytes) else {
        return bytes.to_vec();
    };
    let Some(root) = settings.as_object_mut() else {
        return bytes.to_vec();
    };

    for key in ["extensions", "skills", "prompts", "themes"] {
        if let Some(resources) = root.get_mut(key).and_then(Value::as_array_mut) {
            for resource in resources {
                rewrite_resource_value(resource, agent_dir);
            }
        }
    }
    if let Some(packages) = root.get_mut("packages").and_then(Value::as_array_mut) {
        for package in packages {
            match package {
                Value::String(_) => rewrite_resource_value(package, agent_dir),
                Value::Object(package) => {
                    if let Some(source) = package.get_mut("source") {
                        rewrite_resource_value(source, agent_dir);
                    }
                }
                _ => {}
            }
        }
    }

    serialize_json(settings, bytes)
}

fn rewrite_resource_value(value: &mut Value, agent_dir: &Path) {
    let Some(source) = value.as_str() else {
        return;
    };
    let source = source.trim();
    if source.is_empty()
        || Path::new(source).is_absolute()
        || source.starts_with('~')
        || [
            "npm:", "git:", "github:", "http:", "https:", "ssh:", "builtin:", "file:",
        ]
        .iter()
        .any(|prefix| source.starts_with(prefix))
    {
        return;
    }

    *value = Value::String(agent_dir.join(source).to_string_lossy().into_owned());
}

fn serialize_json(value: Value, original: &[u8]) -> Vec<u8> {
    let Ok(mut rewritten) = serde_json::to_vec_pretty(&value) else {
        return original.to_vec();
    };
    rewritten.push(b'\n');
    rewritten
}

fn copy_entry(
    source: &Path,
    destination: &Path,
    ancestors: &mut HashSet<PathBuf>,
) -> Result<(), StageError> {
    let canonical = fs::canonicalize(source).map_err(StageError::Source)?;
    if !ancestors.insert(canonical.clone()) {
        return Ok(());
    }
    let result = copy_entry_contents(&canonical, destination, ancestors);
    ancestors.remove(&canonical);
    result
}

fn copy_entry_contents(
    source: &Path,
    destination: &Path,
    ancestors: &mut HashSet<PathBuf>,
) -> Result<(), StageError> {
    let metadata = fs::metadata(source).map_err(StageError::Source)?;
    if metadata.is_dir() {
        fs::create_dir(destination).map_err(StageError::Destination)?;
        let entries = fs::read_dir(source).map_err(StageError::Source)?;
        for entry in entries {
            let entry = entry.map_err(StageError::Source)?;
            copy_entry(
                &entry.path(),
                &destination.join(entry.file_name()),
                ancestors,
            )?;
        }
    } else if metadata.is_file() {
        let bytes = fs::read(source).map_err(StageError::Source)?;
        fs::write(destination, bytes).map_err(StageError::Destination)?;
    }
    Ok(())
}

fn prepare_shared_store(agent_dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(agent_dir)?;
    for name in SHARED_PACKAGE_DIRS {
        fs::create_dir_all(agent_dir.join(name))?;
    }
    fs::create_dir_all(agent_dir.join("sessions"))
}

#[cfg(unix)]
fn link_path(target: &Path, link: &Path, _is_dir: bool) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn link_path(target: &Path, link: &Path, is_dir: bool) -> std::io::Result<()> {
    if is_dir {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_resource_paths_keep_their_original_base() {
        let root = assert_fs::TempDir::new().unwrap();
        let agent_dir = root.path().join("profile").join("agent");
        let settings = br#"{"sessionDir":"../existing-sessions","extensions":["../shared/ext.js","npm:example"],"packages":["../../packages/demo",{"source":"../shared/pkg"}],"skills":["./skills"]}"#;

        let rewritten = rewrite_relative_resources(settings, &agent_dir);
        let parsed: Value = serde_json::from_slice(&rewritten).unwrap();
        assert_eq!(parsed["sessionDir"], "../existing-sessions");
        assert_eq!(
            parsed["extensions"][0],
            agent_dir
                .join("../shared/ext.js")
                .to_string_lossy()
                .as_ref()
        );
        assert_eq!(parsed["extensions"][1], "npm:example");
        assert_eq!(
            parsed["packages"][0],
            agent_dir
                .join("../../packages/demo")
                .to_string_lossy()
                .as_ref()
        );
        assert_eq!(
            parsed["packages"][1]["source"],
            agent_dir.join("../shared/pkg").to_string_lossy().as_ref()
        );
        assert_eq!(
            parsed["skills"][0],
            agent_dir.join("./skills").to_string_lossy().as_ref()
        );
    }

    #[test]
    fn malformed_native_files_are_preserved_for_pi_to_validate() {
        let malformed = b"{not valid json";
        assert_eq!(
            rewrite_models(malformed, &[("openai", "http://gateway/v1".into())]),
            malformed
        );
        assert_eq!(
            rewrite_relative_resources(malformed, Path::new("/selected/agent")),
            malformed
        );
    }

    #[test]
    fn model_endpoint_override_leaves_other_providers_untouched() {
        let models = br#"{"providers":{"openai":{"baseUrl":"https://old.invalid/v1","models":[{"id":"m","baseUrl":"https://model.invalid/v1"}]},"anthropic":{"baseUrl":"https://native.invalid"}}}"#;
        let rewritten = rewrite_models(models, &[("openai", "http://gateway/v1".into())]);
        let parsed: Value = serde_json::from_slice(&rewritten).unwrap();
        assert_eq!(
            parsed["providers"]["openai"]["baseUrl"],
            "http://gateway/v1"
        );
        assert_eq!(
            parsed["providers"]["openai"]["models"][0]["baseUrl"],
            "http://gateway/v1"
        );
        assert_eq!(
            parsed["providers"]["anthropic"]["baseUrl"],
            "https://native.invalid"
        );
    }

    #[test]
    fn source_and_standard_data_are_staged_separately_without_mutating_inputs() {
        let root = assert_fs::TempDir::new().unwrap();
        let source = root.path().join("profile-agent");
        let standard = root.path().join("normal-agent");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&standard).unwrap();
        fs::create_dir_all(standard.join("sessions")).unwrap();
        fs::create_dir_all(standard.join("npm")).unwrap();
        fs::create_dir_all(standard.join("git")).unwrap();
        fs::write(source.join("settings.json"), br#"{"theme":"light"}"#).unwrap();
        fs::write(
            source.join("models.json"),
            br#"{"providers":{"openai":{"models":[]}}}"#,
        )
        .unwrap();
        fs::write(source.join("AGENTS.md"), "selected instructions").unwrap();
        fs::write(standard.join("auth.json"), "{\"openai\":{}}\n").unwrap();
        fs::write(standard.join("trust.json"), "{}\n").unwrap();

        let destination = root.path().join("stage");
        fs::create_dir(&destination).unwrap();
        let env = LaunchEnv {
            vars: vec![("OPENAI_BASE_URL".into(), "http://gateway.invalid/v1".into())],
            auth_vars: Vec::new(),
            display_only_vars: Vec::new(),
            clear_vars: Vec::new(),
            remove_vars: Vec::new(),
            profile_name: "work".into(),
        };
        stage_agent_config(
            &source,
            &destination,
            &standard,
            config::ApiFormat::OpenAi,
            &env,
        )
        .unwrap();

        assert_eq!(
            fs::read(destination.join("AGENTS.md")).unwrap(),
            b"selected instructions"
        );
        assert_eq!(
            fs::read_to_string(source.join("models.json")).unwrap(),
            r#"{"providers":{"openai":{"models":[]}}}"#
        );
        assert_eq!(
            fs::read_to_string(destination.join("auth.json")).unwrap(),
            fs::read_to_string(standard.join("auth.json")).unwrap()
        );
        assert_eq!(
            fs::read_to_string(destination.join("trust.json")).unwrap(),
            fs::read_to_string(standard.join("trust.json")).unwrap()
        );
        #[cfg(unix)]
        for name in ["auth.json", "trust.json", "sessions"] {
            assert!(
                fs::symlink_metadata(destination.join(name))
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "{name} must share existing Pi data without copying"
            );
        }
        let models: Value =
            serde_json::from_slice(&fs::read(destination.join("models.json")).unwrap()).unwrap();
        assert_eq!(
            models["providers"]["openai"]["baseUrl"],
            "http://gateway.invalid/v1"
        );
    }

    #[test]
    fn agent_path_resolves_against_config_directory_and_supports_home_shorthand() {
        let root = assert_fs::TempDir::new().unwrap();
        let config_path = root.path().join("config").join("aix.toml");
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        let relative = resolve_agent_dir(Path::new("../pi-agent"), &config_path).unwrap();
        assert_eq!(relative, root.path().join("pi-agent"));

        let absolute = root.path().join("absolute-agent");
        assert_eq!(
            resolve_agent_dir(&absolute, &config_path).unwrap(),
            absolute
        );

        let home = directories::BaseDirs::new()
            .unwrap()
            .home_dir()
            .to_path_buf();
        let expanded = resolve_agent_dir(Path::new("~/.pi/agent"), &config_path).unwrap();
        assert_eq!(expanded, home.join(".pi/agent"));
    }

    #[test]
    fn agent_directory_inspection_rejects_missing_paths_and_files() {
        let root = assert_fs::TempDir::new().unwrap();
        let file = root.path().join("not-a-directory");
        fs::write(&file, "file").unwrap();

        assert_eq!(
            inspect_agent_dir(&file).unwrap_err(),
            "path is not a directory"
        );
        assert_eq!(
            inspect_agent_dir(&root.path().join("missing")).unwrap_err(),
            "directory does not exist"
        );
    }

    #[test]
    fn explicit_native_provider_and_model_overrides_remain_allowed() {
        let root = assert_fs::TempDir::new().unwrap();
        fs::create_dir(root.path().join("agent")).unwrap();
        let config = PiToolConfig {
            agent_dir: root.path().join("agent"),
        };
        let config_file = root.path().join("aix.toml");
        fs::write(&config_file, "").unwrap();
        let launch =
            PiAgentConfigLaunch::resolve("work", &config_file, &config, config::ApiFormat::OpenAi)
                .unwrap();
        launch
            .validate_arguments(&[
                "--provider".into(),
                "anthropic".into(),
                "--model".into(),
                "anthropic/claude".into(),
            ])
            .unwrap();
        assert!(launch
            .validate_arguments(&["--agent-dir=/tmp/other".into()])
            .is_err());
        assert!(launch
            .validate_arguments(&["--api-key".into(), "other".into()])
            .is_err());
    }
}
