use crate::commands::launch::LaunchEnv;
use crate::config::OpenCodeProfileConfig;
use crate::error::AixError;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const TOOL_NAME: &str = "opencode";
const CONFIG_DIR_ENV: &str = "OPENCODE_CONFIG_DIR";
const APP_CONFIG_ENV: &str = "OPENCODE_CONFIG";
const APP_CONTENT_ENV: &str = "OPENCODE_CONFIG_CONTENT";
const CLI_CONTENT_ENV: &str = "OPENCODE_CLI_CONFIG_CONTENT";
const SERVER_ENV: [&str; 2] = ["OPENCODE_SERVER", "OPENCODE_SERVER_URL"];
const APP_CONFIG_FILES: [&str; 2] = ["opencode.json", "opencode.jsonc"];
const CLI_CONFIG_FILE: &str = "cli.json";

#[derive(Debug)]
enum SourceDecision {
    Standard,
    Selected(PathBuf),
    Fallback { path: PathBuf, reason: String },
}

impl SourceDecision {
    fn selected_path(&self) -> Option<&Path> {
        match self {
            Self::Selected(path) => Some(path),
            Self::Standard | Self::Fallback { .. } => None,
        }
    }
}

pub(super) struct ProfileConfigPlan {
    profile: String,
    app: SourceDecision,
    cli: SourceDecision,
}

impl ProfileConfigPlan {
    pub(super) fn resolve(
        profile: &str,
        aix_config_path: &Path,
        config: &OpenCodeProfileConfig,
    ) -> Self {
        Self {
            profile: profile.to_string(),
            app: resolve_source(
                profile,
                aix_config_path,
                "config_file",
                config.config_file.as_deref(),
            ),
            cli: resolve_source(
                profile,
                aix_config_path,
                "cli_config_file",
                config.cli_config_file.as_deref(),
            ),
        }
    }

    fn active(&self) -> bool {
        self.app.selected_path().is_some() || self.cli.selected_path().is_some()
    }

    pub(super) fn validate_arguments(&self, args: &[String]) -> Result<(), AixError> {
        if self.active() {
            reject_conflicting_arguments(args)?;
        }
        Ok(())
    }

    pub(super) fn prepare(
        &mut self,
        env: &mut LaunchEnv,
        child_args: &mut Vec<String>,
        dry_run: bool,
    ) -> Result<Option<TempDir>, AixError> {
        if !self.active() {
            if dry_run {
                self.print_dry_run(false);
            }
            return Ok(None);
        }

        self.validate_arguments(child_args)?;
        if dry_run {
            self.apply_environment_overrides(env, None);
            ensure_standalone(child_args);
            self.print_dry_run(has_standalone_flag(child_args));
            return Ok(None);
        }

        let standard_dir = standard_config_dir(env);
        let Some((stage, directory)) = self.stage(&standard_dir)? else {
            return Ok(None);
        };

        self.apply_environment_overrides(env, Some(&directory));
        ensure_standalone(child_args);
        Ok(Some(stage))
    }

    fn stage(&mut self, standard_dir: &Path) -> Result<Option<(TempDir, PathBuf)>, AixError> {
        loop {
            if !self.active() {
                return Ok(None);
            }

            let stage = tempfile::Builder::new()
                .prefix("aix-opencode-config-")
                .tempdir()
                .map_err(|source| AixError::OpenCodeConfigStage { source })?;
            match build_stage(stage.path(), standard_dir, &self.app, &self.cli) {
                Ok(()) => {
                    let path = stage.path().to_path_buf();
                    return Ok(Some((stage, path)));
                }
                Err(StageFailure::App(source)) => {
                    Self::fallback(&self.profile, "config_file", &mut self.app, source);
                }
                Err(StageFailure::Cli(source)) => {
                    Self::fallback(&self.profile, "cli_config_file", &mut self.cli, source);
                }
                Err(StageFailure::Standard(source)) => {
                    return Err(AixError::OpenCodeConfigStage { source });
                }
            }
        }
    }

    fn fallback(profile: &str, source: &str, decision: &mut SourceDecision, error: io::Error) {
        let Some(path) = decision.selected_path().map(Path::to_path_buf) else {
            return;
        };
        let reason = error.to_string();
        warn_unavailable(profile, source, &path, &reason);
        *decision = SourceDecision::Fallback { path, reason };
    }

    fn apply_environment_overrides(&self, env: &mut LaunchEnv, stage: Option<&Path>) {
        remove_configured_env(env, CONFIG_DIR_ENV);
        remove_configured_env(env, SERVER_ENV[0]);
        remove_configured_env(env, SERVER_ENV[1]);
        add_clear_var(env, SERVER_ENV[0]);
        add_clear_var(env, SERVER_ENV[1]);

        if self.app.selected_path().is_some() {
            remove_configured_env(env, APP_CONFIG_ENV);
            remove_configured_env(env, APP_CONTENT_ENV);
            add_clear_var(env, APP_CONFIG_ENV);
            // ChatGPT's required process-local bridge config is appended after
            // this preparation step, so it remains active with the selected file.
            add_clear_var(env, APP_CONTENT_ENV);
        }
        if self.cli.selected_path().is_some() {
            remove_configured_env(env, CLI_CONTENT_ENV);
            add_clear_var(env, CLI_CONTENT_ENV);
        }

        match stage {
            Some(path) => env.vars.push((
                CONFIG_DIR_ENV.to_string(),
                path.to_string_lossy().into_owned(),
            )),
            None => env.display_only_vars.push(CONFIG_DIR_ENV.to_string()),
        }
    }

    fn print_dry_run(&self, private_server: bool) {
        eprintln!(
            "OpenCode profile configuration (profile {:?}, tool {:?}):",
            self.profile, TOOL_NAME
        );
        print_source_decision("config_file", &self.app);
        print_source_decision("cli_config_file", &self.cli);
        if self.active() {
            eprintln!("  would stage selected global sources under OPENCODE_CONFIG_DIR");
            if private_server {
                eprintln!("  would use OpenCode's private server (--standalone)");
            }
        }
    }
}

fn resolve_source(
    profile: &str,
    aix_config_path: &Path,
    name: &str,
    configured_path: Option<&Path>,
) -> SourceDecision {
    let Some(configured_path) = configured_path else {
        return SourceDecision::Standard;
    };
    let path = resolve_path(configured_path, aix_config_path);
    let result = fs::metadata(&path).and_then(|metadata| {
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "source is not a regular file",
            ));
        }
        File::open(&path).map(drop)
    });
    match result {
        Ok(()) => SourceDecision::Selected(path),
        Err(error) => {
            let reason = error.to_string();
            warn_unavailable(profile, name, &path, &reason);
            SourceDecision::Fallback { path, reason }
        }
    }
}

fn resolve_path(path: &Path, aix_config_path: &Path) -> PathBuf {
    let expanded = PathBuf::from(shellexpand::tilde(&path.to_string_lossy()).into_owned());
    if expanded.is_absolute() {
        expanded
    } else {
        aix_config_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .join(expanded)
    }
}

fn warn_unavailable(profile: &str, source: &str, path: &Path, reason: &str) {
    eprintln!(
        "aix: warning: profile {:?} tool {:?} source {} at {:?} is unavailable ({}); using standard OpenCode configuration for this source",
        profile,
        TOOL_NAME,
        source,
        path.display(),
        reason
    );
}

fn print_source_decision(name: &str, decision: &SourceDecision) {
    match decision {
        SourceDecision::Standard => eprintln!("  {name}: standard (not configured)"),
        SourceDecision::Selected(path) => {
            eprintln!("  {name}: selected {}", path.display())
        }
        SourceDecision::Fallback { path, reason } => eprintln!(
            "  {name}: {} unavailable ({reason}); fallback to standard",
            path.display()
        ),
    }
}

fn reject_conflicting_arguments(args: &[String]) -> Result<(), AixError> {
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "--" {
            break;
        }
        if ["--server", "--config", "--config-dir", "--config-file"]
            .iter()
            .any(|option| arg == *option || arg.starts_with(&format!("{option}=")))
        {
            return Err(AixError::OpenCodeConfigArgumentConflict);
        }
        if takes_value(arg) {
            index += 2;
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn takes_value(arg: &str) -> bool {
    matches!(
        arg,
        "--session"
            | "-s"
            | "--model"
            | "-m"
            | "--agent"
            | "--file"
            | "-f"
            | "--format"
            | "--title"
            | "--prompt"
            | "--log-level"
    )
}

fn ensure_standalone(args: &mut Vec<String>) {
    if has_standalone_flag(args) {
        return;
    }

    // OpenCode v2.0.20 scopes this flag to server-backed subcommands. Local
    // configuration commands such as `plugin remove` reject it, while the TUI
    // accepts it at the root.
    let insertion_index = match args.first().map(String::as_str) {
        None => Some(0),
        Some("run" | "api" | "models") => Some(1),
        Some("session" | "auth") => args.get(1).map(|_| 2),
        Some(
            "upgrade" | "update" | "uninstall" | "acp" | "debug" | "mcp" | "plugin" | "stats"
            | "mini" | "service" | "reload" | "pair" | "serve",
        ) => None,
        Some(arg) if arg.starts_with('-') => Some(0),
        Some(_) => Some(0),
    };

    if let Some(index) = insertion_index {
        args.insert(index, "--standalone".to_string());
    }
}

fn has_standalone_flag(args: &[String]) -> bool {
    args.iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| arg == "--standalone")
}

fn remove_configured_env(env: &mut LaunchEnv, name: &str) {
    env.vars
        .retain(|(configured_name, _)| configured_name != name);
    env.display_only_vars
        .retain(|configured_name| configured_name != name);
}

fn add_clear_var(env: &mut LaunchEnv, name: &str) {
    if !env.clear_vars.iter().any(|cleared| cleared == name) {
        env.clear_vars.push(name.to_string());
    }
}

fn effective_env_value(env: &LaunchEnv, name: &str) -> Option<String> {
    if let Some((_, value)) = env.vars.iter().rev().find(|(key, _)| key == name) {
        return Some(value.clone());
    }
    if env.clear_vars.iter().any(|key| key == name) {
        return None;
    }
    std::env::var(name).ok()
}

fn standard_config_dir(env: &LaunchEnv) -> PathBuf {
    if let Some(value) = effective_env_value(env, CONFIG_DIR_ENV).filter(|value| !value.is_empty())
    {
        return PathBuf::from(value);
    }
    if let Some(value) =
        effective_env_value(env, "XDG_CONFIG_HOME").filter(|value| !value.is_empty())
    {
        return PathBuf::from(value).join(TOOL_NAME);
    }
    #[cfg(windows)]
    if let Some(value) = effective_env_value(env, "APPDATA").filter(|value| !value.is_empty()) {
        return PathBuf::from(value).join(TOOL_NAME);
    }
    if let Some(home) = effective_env_value(env, "HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(home).join(".config").join(TOOL_NAME);
    }
    directories::BaseDirs::new()
        .map(|dirs| dirs.config_dir().join(TOOL_NAME))
        .unwrap_or_else(|| PathBuf::from(".config").join(TOOL_NAME))
}

#[derive(Debug)]
enum StageFailure {
    App(io::Error),
    Cli(io::Error),
    Standard(io::Error),
}

fn build_stage(
    stage: &Path,
    standard: &Path,
    app: &SourceDecision,
    cli: &SourceDecision,
) -> Result<(), StageFailure> {
    if let Some(app_path) = app.selected_path() {
        let source_dir = app_path.parent().unwrap_or_else(|| Path::new("."));
        copy_tree_contents(
            source_dir,
            stage,
            &[APP_CONFIG_FILES[0], APP_CONFIG_FILES[1], CLI_CONFIG_FILE],
        )
        .map_err(StageFailure::App)?;
        let filename = match app_path.file_name().and_then(|name| name.to_str()) {
            Some("opencode.jsonc") => "opencode.jsonc",
            _ => "opencode.json",
        };
        copy_file(app_path, &stage.join(filename)).map_err(StageFailure::App)?;
    } else {
        let skip_cli = cli.selected_path().is_some();
        let skip = if skip_cli {
            &[CLI_CONFIG_FILE][..]
        } else {
            &[][..]
        };
        copy_standard_tree(standard, stage, skip).map_err(StageFailure::Standard)?;
    }

    if let Some(cli_path) = cli.selected_path() {
        copy_file(cli_path, &stage.join(CLI_CONFIG_FILE)).map_err(StageFailure::Cli)?;
    } else if app.selected_path().is_some() {
        let standard_cli = standard.join(CLI_CONFIG_FILE);
        match fs::metadata(&standard_cli) {
            Ok(metadata) if metadata.is_file() => {
                copy_file(&standard_cli, &stage.join(CLI_CONFIG_FILE))
                    .map_err(StageFailure::Standard)?;
            }
            Ok(_) => {
                return Err(StageFailure::Standard(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "standard cli.json is not a regular file",
                )))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(StageFailure::Standard(error)),
        }
    }
    Ok(())
}

fn copy_standard_tree(standard: &Path, stage: &Path, excluded: &[&str]) -> io::Result<()> {
    match fs::metadata(standard) {
        Ok(metadata) if metadata.is_dir() => copy_tree_contents(standard, stage, excluded),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "standard OpenCode config root is not a directory",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn copy_tree_contents(source: &Path, destination: &Path, excluded: &[&str]) -> io::Result<()> {
    let mut ancestors = HashSet::new();
    copy_directory(source, destination, excluded, &mut ancestors, true)
}

fn copy_directory(
    source: &Path,
    destination: &Path,
    excluded: &[&str],
    ancestors: &mut HashSet<PathBuf>,
    root: bool,
) -> io::Result<()> {
    let canonical = fs::canonicalize(source)?;
    if !ancestors.insert(canonical.clone()) {
        return Ok(());
    }
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if root && excluded.iter().any(|item| name == *item) {
            continue;
        }
        let source_path = entry.path();
        let destination_path = destination.join(&name);
        let metadata = match fs::metadata(&source_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if metadata.is_dir() {
            fs::create_dir_all(&destination_path)?;
            copy_directory(&source_path, &destination_path, &[], ancestors, false)?;
        } else if metadata.is_file() {
            copy_file(&source_path, &destination_path)?;
        }
    }
    ancestors.remove(&canonical);
    Ok(())
}

fn copy_file(source: &Path, destination: &Path) -> io::Result<()> {
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    io::copy(&mut input, &mut output)?;
    let mut permissions = input.metadata()?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() | 0o200);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    fs::set_permissions(destination, permissions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::OpenCodeProfileConfig;
    use std::path::PathBuf;

    fn launch_env() -> LaunchEnv {
        LaunchEnv {
            vars: Vec::new(),
            auth_vars: Vec::new(),
            display_only_vars: Vec::new(),
            clear_vars: Vec::new(),
            remove_vars: Vec::new(),
            profile_name: "work".to_string(),
        }
    }

    #[test]
    fn path_forms_resolve_against_aix_config_and_home() {
        let root = tempfile::tempdir().unwrap();
        let config_dir = root.path().join("config");
        let aix_config = config_dir.join("aix.toml");
        let home = root.path().join("home");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let previous_home = std::env::var_os("HOME");
        std::env::set_var("HOME", &home);

        assert_eq!(
            resolve_path(Path::new("../profile/opencode.json"), &aix_config),
            config_dir.join("../profile/opencode.json")
        );
        assert_eq!(
            resolve_path(Path::new("~/opencode.json"), &aix_config),
            home.join("opencode.json")
        );
        let absolute = root.path().join("absolute.json");
        assert_eq!(resolve_path(&absolute, &aix_config), absolute);

        match previous_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn selected_app_is_staged_with_its_relative_resources_and_standard_cli_file() {
        let root = tempfile::tempdir().unwrap();
        let app_dir = root.path().join("profile/app");
        let app = app_dir.join("custom.json");
        let resource = app_dir.join("resources/prompt.txt");
        let standard = root.path().join("global");
        std::fs::create_dir_all(resource.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&standard).unwrap();
        std::fs::write(&app, "{\"username\":\"selected\"}").unwrap();
        std::fs::write(&resource, "relative resource").unwrap();
        std::fs::write(
            standard.join("opencode.json"),
            "{\"username\":\"global must not leak\"}",
        )
        .unwrap();
        std::fs::write(standard.join(CLI_CONFIG_FILE), "{\"animations\":false}").unwrap();
        let stage = tempfile::tempdir().unwrap();
        let app_decision = SourceDecision::Selected(app.clone());

        build_stage(
            stage.path(),
            &standard,
            &app_decision,
            &SourceDecision::Standard,
        )
        .unwrap();

        let staged_app = std::fs::read_to_string(stage.path().join("opencode.json")).unwrap();
        assert!(staged_app.contains("selected"));
        assert!(!staged_app.contains("global must not leak"));
        assert_eq!(
            std::fs::read_to_string(stage.path().join("resources/prompt.txt")).unwrap(),
            "relative resource"
        );
        assert_eq!(
            std::fs::read_to_string(stage.path().join(CLI_CONFIG_FILE)).unwrap(),
            "{\"animations\":false}"
        );
        assert_eq!(
            std::fs::read_to_string(&app).unwrap(),
            "{\"username\":\"selected\"}"
        );
        stage.close().unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn unreadable_selected_files_fall_back_without_parsing_their_contents() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let selected = root.path().join("opencode.json");
        std::fs::write(&selected, "not parsed by aix").unwrap();
        std::fs::set_permissions(&selected, std::fs::Permissions::from_mode(0o000)).unwrap();
        if File::open(&selected).is_ok() {
            // Some test runners have permission to read mode-000 files.
            return;
        }
        let config = OpenCodeProfileConfig {
            config_file: Some(selected),
            cli_config_file: None,
        };

        let plan = ProfileConfigPlan::resolve("work", &root.path().join("aix.toml"), &config);

        assert!(!plan.active());
        assert!(matches!(plan.app, SourceDecision::Fallback { .. }));
    }

    #[test]
    fn concurrent_profile_stages_keep_their_application_config_isolated() {
        let root = tempfile::tempdir().unwrap();
        let mut workers = Vec::new();
        for (profile, marker) in [("alpha", "ALPHA_CONFIG"), ("beta", "BETA_CONFIG")] {
            let source = root.path().join(profile).join("opencode.json");
            std::fs::create_dir_all(source.parent().unwrap()).unwrap();
            std::fs::write(&source, format!("{{\"username\":\"{marker}\"}}")).unwrap();
            let standard = root.path().join("standard");
            let profile = profile.to_string();
            let marker = marker.to_string();
            workers.push(std::thread::spawn(move || {
                let mut plan = ProfileConfigPlan {
                    profile,
                    app: SourceDecision::Selected(source),
                    cli: SourceDecision::Standard,
                };
                let (stage, path) = plan.stage(&standard).unwrap().unwrap();
                let config = std::fs::read_to_string(path.join("opencode.json")).unwrap();
                (stage, path, marker, config)
            }));
        }

        let results = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        assert_ne!(results[0].1, results[1].1);
        for (_, _, marker, config) in results {
            assert!(config.contains(&marker));
            assert!(!config.contains(if marker == "ALPHA_CONFIG" {
                "BETA_CONFIG"
            } else {
                "ALPHA_CONFIG"
            }));
        }
    }

    #[test]
    fn cli_only_replacement_retains_standard_application_config() {
        let root = tempfile::tempdir().unwrap();
        let standard = root.path().join("global");
        let cli = root.path().join("profile/cli.json");
        std::fs::create_dir_all(&standard).unwrap();
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::fs::write(standard.join("opencode.json"), "{\"model\":\"global\"}").unwrap();
        std::fs::write(standard.join(CLI_CONFIG_FILE), "{\"animations\":false}").unwrap();
        std::fs::write(&cli, "{\"animations\":true}").unwrap();
        let stage = tempfile::tempdir().unwrap();

        build_stage(
            stage.path(),
            &standard,
            &SourceDecision::Standard,
            &SourceDecision::Selected(cli),
        )
        .unwrap();

        assert!(stage.path().join("opencode.json").is_file());
        let cli_contents = std::fs::read_to_string(stage.path().join(CLI_CONFIG_FILE)).unwrap();
        assert!(cli_contents.contains("true"));
        assert!(!cli_contents.contains("false"));
    }

    #[test]
    fn selected_sources_clear_only_their_conflicting_selectors() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("opencode.json");
        let cli = root.path().join("cli.json");
        std::fs::write(&app, "{}").unwrap();
        std::fs::write(&cli, "{}").unwrap();
        let config = OpenCodeProfileConfig {
            config_file: Some(app),
            cli_config_file: Some(cli),
        };
        let plan = ProfileConfigPlan {
            profile: "work".to_string(),
            app: SourceDecision::Selected(config.config_file.unwrap()),
            cli: SourceDecision::Selected(config.cli_config_file.unwrap()),
        };
        let mut env = launch_env();
        env.vars = [
            (CONFIG_DIR_ENV.to_string(), "/wrong/root".to_string()),
            (APP_CONFIG_ENV.to_string(), "/wrong/app.json".to_string()),
            (APP_CONTENT_ENV.to_string(), "not-a-secret".to_string()),
            (CLI_CONTENT_ENV.to_string(), "not-cli".to_string()),
            ("UNRELATED".to_string(), "keep".to_string()),
        ]
        .into();
        let stage = tempfile::tempdir().unwrap();

        plan.apply_environment_overrides(&mut env, Some(stage.path()));

        assert!(!env.vars.iter().any(|(key, _)| key == APP_CONFIG_ENV));
        assert!(!env.vars.iter().any(|(key, _)| key == APP_CONTENT_ENV));
        assert!(!env.vars.iter().any(|(key, _)| key == CLI_CONTENT_ENV));
        assert!(env.vars.iter().any(|(key, value)| key == CONFIG_DIR_ENV
            && value == stage.path().to_string_lossy().as_ref()));
        assert!(env
            .vars
            .iter()
            .any(|(key, value)| key == "UNRELATED" && value == "keep"));
        assert!(env.clear_vars.iter().any(|key| key == APP_CONFIG_ENV));
        assert!(env.clear_vars.iter().any(|key| key == APP_CONTENT_ENV));
        assert!(env.clear_vars.iter().any(|key| key == CLI_CONTENT_ENV));
        assert!(env.clear_vars.iter().any(|key| key == SERVER_ENV[0]));
    }

    #[test]
    fn conflicting_server_and_config_arguments_are_rejected_but_model_overrides_are_allowed() {
        for args in [
            vec!["run".to_string(), "--server=http://other".to_string()],
            vec![
                "run".to_string(),
                "--config".to_string(),
                "other.json".to_string(),
            ],
        ] {
            assert!(matches!(
                reject_conflicting_arguments(&args),
                Err(AixError::OpenCodeConfigArgumentConflict)
            ));
        }
        assert!(reject_conflicting_arguments(&[
            "run".to_string(),
            "--model".to_string(),
            "openai/model".to_string(),
        ])
        .is_ok());
        assert!(reject_conflicting_arguments(&[
            "run".to_string(),
            "--".to_string(),
            "--server".to_string(),
        ])
        .is_ok());
    }

    #[test]
    fn standalone_is_inserted_after_server_backed_subcommands() {
        for (mut args, expected) in [
            (
                vec![
                    "run".to_string(),
                    "--model".to_string(),
                    "openai/model".to_string(),
                ],
                vec!["run", "--standalone", "--model", "openai/model"],
            ),
            (
                vec!["api".to_string(), "config.get".to_string()],
                vec!["api", "--standalone", "config.get"],
            ),
            (
                vec![
                    "session".to_string(),
                    "list".to_string(),
                    "--format".to_string(),
                    "json".to_string(),
                ],
                vec!["session", "list", "--standalone", "--format", "json"],
            ),
            (
                vec!["auth".to_string(), "list".to_string()],
                vec!["auth", "list", "--standalone"],
            ),
            (Vec::new(), vec!["--standalone"]),
        ] {
            ensure_standalone(&mut args);
            ensure_standalone(&mut args);
            assert_eq!(args, expected);
        }

        let mut explicit = vec!["run".to_string(), "--standalone".to_string()];
        ensure_standalone(&mut explicit);
        assert_eq!(explicit, ["run", "--standalone"]);

        let mut configuration_command = vec![
            "plugin".to_string(),
            "remove".to_string(),
            "example-plugin".to_string(),
        ];
        ensure_standalone(&mut configuration_command);
        assert_eq!(
            configuration_command,
            ["plugin", "remove", "example-plugin"]
        );
    }

    #[test]
    fn profile_plan_reports_unavailable_sources_without_activating_any_override() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("not-a-file");
        std::fs::create_dir(&directory).unwrap();
        let config = OpenCodeProfileConfig {
            config_file: Some(directory),
            cli_config_file: None,
        };
        let plan = ProfileConfigPlan::resolve("work", &root.path().join("aix.toml"), &config);
        assert!(!plan.active());
        assert!(matches!(plan.app, SourceDecision::Fallback { .. }));
        assert!(matches!(plan.cli, SourceDecision::Standard));
    }

    #[test]
    fn standard_config_dir_uses_a_child_config_dir_override_as_the_counterpart_source() {
        let env = LaunchEnv {
            vars: vec![(CONFIG_DIR_ENV.to_string(), "/profile/global".to_string())],
            ..launch_env()
        };
        assert_eq!(standard_config_dir(&env), PathBuf::from("/profile/global"));
    }
}
