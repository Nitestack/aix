#[derive(Debug, thiserror::Error)]
pub enum AixError {
    #[allow(dead_code)]
    #[error("not yet implemented: {0}")]
    NotImplemented(&'static str),

    #[error("unknown config format: {ext}")]
    UnknownFormat { ext: String },

    #[error("failed to parse {path}: {source}")]
    ParseError {
        path: std::path::PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("default_profile \"{0}\" is not defined in profiles")]
    UnknownDefaultProfile(String),

    #[allow(dead_code)]
    #[error("ambiguous secret source for {field}: specify exactly one of env / file / command")]
    AmbiguousSecretSource { field: String },

    #[error("no config file found")]
    NoConfigFile,

    #[allow(dead_code)]
    #[error("env var {name:?} is not set")]
    SecretMissingEnvVar { name: String },

    #[allow(dead_code)]
    #[error("failed to read secret file {path}: {source}")]
    SecretFileRead {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[allow(dead_code)]
    #[error("secret command {cmd:?} failed (exit code: {exit_code:?})")]
    SecretCommandFailed { cmd: String, exit_code: Option<i32> },

    #[allow(dead_code)]
    #[error("failed to spawn secret command {cmd:?}: {source}")]
    SecretCommandSpawn {
        cmd: String,
        #[source]
        source: std::io::Error,
    },

    #[allow(dead_code)]
    #[error("secret command {cmd:?} produced non-UTF-8 output")]
    SecretCommandEncoding { cmd: String },

    #[allow(dead_code)]
    #[error("failed to load env file {path}: {source}")]
    EnvFileLoad {
        path: std::path::PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("profile \"{name}\" is not defined\n\nAvailable profiles:\n{available_hint}")]
    ProfileNotFound {
        name: String,
        available_hint: String,
    },

    #[allow(dead_code)]
    #[error("no profile specified and no default_profile set in config")]
    NoProfile,

    #[allow(dead_code)]
    #[error(
        "duplicate profile label \"{label}\" — found in profiles \"{first}\" and \"{second}\""
    )]
    DuplicateLabel {
        label: String,
        first: String,
        second: String,
    },

    #[error("config must define at least one profile in [profiles]")]
    NoProfilesConfigured,

    #[allow(dead_code)]
    #[error("profile selection cancelled")]
    SelectionCancelled,

    #[allow(dead_code)]
    #[error("no profile specified; pass a profile name or run in an interactive terminal")]
    NoInteractiveTerminal,

    #[error("exec requires a command after --")]
    ExecNoCommand,

    #[error("executable not found: {program}")]
    ExecutableNotFound { program: String },

    #[error("failed to spawn {program}: {source}")]
    ProcessSpawn {
        program: String,
        #[source]
        source: std::io::Error,
    },

    #[error("gateway returned HTTP {status}: {body}")]
    GatewayError { status: u16, body: String },

    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_not_found_message_includes_name_and_available() {
        let e = AixError::ProfileNotFound {
            name: "swtb".to_string(),
            available_hint: "  work\n  private".to_string(),
        };
        let msg = e.to_string();
        assert!(msg.contains("swtb"), "must mention the missing name: {msg}");
        assert!(msg.contains("work"), "must list available profiles: {msg}");
        assert!(
            msg.contains("Available profiles"),
            "must have header: {msg}"
        );
    }

    #[test]
    fn no_profile_message_is_clear() {
        let e = AixError::NoProfile;
        let msg = e.to_string();
        assert!(msg.contains("profile") || msg.contains("default"));
    }

    #[test]
    fn duplicate_label_message_includes_label_and_profiles() {
        let e = AixError::DuplicateLabel {
            label: "Work".to_string(),
            first: "alpha".to_string(),
            second: "beta".to_string(),
        };
        let msg = e.to_string();
        assert!(msg.contains("Work"), "got: {msg}");
        assert!(msg.contains("alpha"), "got: {msg}");
        assert!(msg.contains("beta"), "got: {msg}");
    }

    #[test]
    fn selection_cancelled_message_is_clear() {
        let e = AixError::SelectionCancelled;
        let msg = e.to_string();
        assert!(
            msg.contains("cancel") || msg.contains("select"),
            "got: {msg}"
        );
    }

    #[test]
    fn no_interactive_terminal_message_contains_profile() {
        let e = AixError::NoInteractiveTerminal;
        let msg = e.to_string();
        assert!(msg.contains("profile"), "got: {msg}");
    }

    #[test]
    fn executable_not_found_message_includes_program_name() {
        let e = AixError::ExecutableNotFound {
            program: "pi".to_string(),
        };
        assert!(e.to_string().contains("pi"), "got: {}", e);
    }

    #[test]
    fn gateway_error_message_includes_status_and_body() {
        let e = AixError::GatewayError {
            status: 403,
            body: "Forbidden".to_string(),
        };
        let msg = e.to_string();
        assert!(msg.contains("403"), "got: {msg}");
        assert!(msg.contains("Forbidden"), "got: {msg}");
    }

    #[test]
    fn process_spawn_message_includes_program_name() {
        let e = AixError::ProcessSpawn {
            program: "claude".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
        };
        assert!(e.to_string().contains("claude"), "got: {}", e);
    }
}
