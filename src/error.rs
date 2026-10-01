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

    #[error("no model specified and no default model is configured; pass --model <MODEL> or configure [models].default or [profiles.<PROFILE>.models].default")]
    NoModelConfigured,

    #[error("`aix ask` needs an instruction: provide PROMPT; files alone are context, not an instruction")]
    AskInstructionRequired,

    #[error("`aix ask` needs a prompt or non-empty piped stdin; see `aix ask --help`")]
    AskInputRequired,

    #[error("prompt names must not be empty")]
    EmptyPromptName,

    #[error("`aix prompt` needs a preset NAME or `--list`")]
    PromptNameRequired,

    #[error("prompt preset {name:?} must have a non-empty {field}")]
    EmptyPromptField { name: String, field: &'static str },

    #[error("prompt preset {name:?} is not defined\n\nAvailable prompts:\n{available_hint}")]
    PromptNotFound {
        name: String,
        available_hint: String,
    },

    #[error("failed to read stdin: {source}")]
    AskStdinRead {
        #[source]
        source: std::io::Error,
    },

    #[error("failed to read explicit input file {path}: {source}")]
    AskFileRead {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("model default in {scope} must be a non-empty model ID")]
    EmptyModelDefault { scope: String },

    #[error("model alias name in {scope} must not be empty")]
    EmptyModelAliasName { scope: String },

    #[error("model alias target for {alias:?} in {scope} must not be empty")]
    EmptyModelAliasTarget { scope: String, alias: String },

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

    #[error("invalid environment variable name {name:?} in profile {profile:?}")]
    InvalidEnvironmentVariableName { profile: String, name: String },

    #[error("tool names must not be empty")]
    EmptyToolName,

    #[error("configured command for tool {name:?} must not be empty")]
    EmptyToolCommand { name: String },

    #[error("invalid environment variable name {name:?} in tool {tool:?}")]
    InvalidToolEnvironmentVariableName { tool: String, name: String },

    #[allow(dead_code)]
    #[error("profile selection cancelled")]
    SelectionCancelled,

    #[allow(dead_code)]
    #[error("no profile specified; pass a profile name or run in an interactive terminal")]
    NoInteractiveTerminal,

    #[error("exec requires a command after --")]
    ExecNoCommand,

    #[error("run requires a command after --")]
    RunNoCommand,

    #[error("run history I/O failed at {path}: {source}")]
    RunHistoryIo {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("run record {run_id} was not found")]
    RunNotFound { run_id: String },

    #[error("failed to encode run record: {0}")]
    RunRecordSerialization(#[from] serde_json::Error),

    #[error("failed to install interrupt handler: {0}")]
    RunInterruptHandler(#[source] std::io::Error),

    #[error(
        "`aix shell` does not accept commands after --; use `aix exec` to run a command directly"
    )]
    ShellExtraArgs,

    #[error(
        "JSON output is only supported by `aix current`, `aix profiles`, `aix spend`, `aix models`, `aix status`, `aix usage`, `aix ask`, `aix prompt`, and `aix runs`"
    )]
    JsonUnsupportedCommand,

    #[error("invalid usage date range: {reason}")]
    InvalidUsageRange { reason: String },

    #[error("`aix usage` requires a LiteLLM-compatible gateway; set `gateway = \"litellm\"` in [endpoint], or omit `gateway` to use the default")]
    UsageNotLiteLlm,

    #[error("usage history is unavailable on this gateway or LiteLLM version (the daily activity endpoint is unsupported)")]
    UsageUnavailable,

    #[error("executable not found: {program}")]
    ExecutableNotFound { program: String },

    #[error("failed to spawn {program}: {source}")]
    ProcessSpawn {
        program: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed while waiting for {program}: {source}")]
    ProcessWait {
        program: String,
        #[source]
        source: std::io::Error,
    },

    #[error("gateway returned HTTP {status}: {body}")]
    GatewayError { status: u16, body: String },

    #[error("gateway protocol error: {0}")]
    GatewayProtocolError(&'static str),

    #[error("gateway request failed with HTTP {status}")]
    GatewayRequestFailed { status: u16 },

    #[error("budget exceeded: ${spend:.2} of ${max_budget:.2}")]
    BudgetExceeded { spend: f64, max_budget: f64 },

    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("aix spend requires a LiteLLM-compatible gateway; set `gateway = \"litellm\"` in [endpoint], or omit `gateway` to use the default")]
    NotLiteLlm,
}

impl AixError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::UnknownFormat { .. }
            | Self::ParseError { .. }
            | Self::UnknownDefaultProfile(_)
            | Self::NoModelConfigured
            | Self::AskInstructionRequired
            | Self::AskInputRequired
            | Self::EmptyPromptName
            | Self::PromptNameRequired
            | Self::EmptyPromptField { .. }
            | Self::PromptNotFound { .. }
            | Self::AskStdinRead { .. }
            | Self::AskFileRead { .. }
            | Self::EmptyModelDefault { .. }
            | Self::EmptyModelAliasName { .. }
            | Self::EmptyModelAliasTarget { .. }
            | Self::AmbiguousSecretSource { .. }
            | Self::NoConfigFile
            | Self::ProfileNotFound { .. }
            | Self::NoProfile
            | Self::DuplicateLabel { .. }
            | Self::NoProfilesConfigured
            | Self::InvalidEnvironmentVariableName { .. }
            | Self::EmptyToolName
            | Self::EmptyToolCommand { .. }
            | Self::InvalidToolEnvironmentVariableName { .. }
            | Self::SelectionCancelled
            | Self::NoInteractiveTerminal
            | Self::ExecNoCommand
            | Self::RunNoCommand
            | Self::RunNotFound { .. }
            | Self::ShellExtraArgs
            | Self::JsonUnsupportedCommand
            | Self::InvalidUsageRange { .. }
            | Self::ExecutableNotFound { .. }
            | Self::NotLiteLlm
            | Self::UsageNotLiteLlm => 2,
            Self::SecretMissingEnvVar { .. }
            | Self::SecretFileRead { .. }
            | Self::SecretCommandFailed { .. }
            | Self::SecretCommandSpawn { .. }
            | Self::SecretCommandEncoding { .. }
            | Self::EnvFileLoad { .. } => 3,
            Self::GatewayError {
                status: 401 | 403, ..
            }
            | Self::GatewayRequestFailed { status: 401 | 403 } => 4,
            Self::GatewayError { .. }
            | Self::GatewayRequestFailed { .. }
            | Self::GatewayProtocolError(_)
            | Self::HttpError(_)
            | Self::UsageUnavailable => 5,
            Self::BudgetExceeded { .. } => 6,
            Self::NotImplemented(_)
            | Self::ProcessSpawn { .. }
            | Self::ProcessWait { .. }
            | Self::RunHistoryIo { .. }
            | Self::RunRecordSerialization(_)
            | Self::RunInterruptHandler(_) => 1,
        }
    }
}

pub fn exit_code(error: &color_eyre::Report) -> i32 {
    error
        .downcast_ref::<AixError>()
        .map_or(1, AixError::exit_code)
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

    #[test]
    fn exit_codes_use_the_documented_process_categories() {
        assert_eq!(AixError::NoConfigFile.exit_code(), 2);
        assert_eq!(AixError::ShellExtraArgs.exit_code(), 2);
        assert_eq!(
            AixError::ExecutableNotFound {
                program: "missing-tool".to_string()
            }
            .exit_code(),
            2
        );
        assert_eq!(
            AixError::SecretMissingEnvVar {
                name: "AIX_KEY".to_string()
            }
            .exit_code(),
            3
        );
        assert_eq!(
            AixError::GatewayError {
                status: 403,
                body: "forbidden".to_string()
            }
            .exit_code(),
            4
        );
        assert_eq!(
            AixError::GatewayError {
                status: 502,
                body: "unavailable".to_string()
            }
            .exit_code(),
            5
        );
        assert_eq!(
            AixError::BudgetExceeded {
                spend: 11.0,
                max_budget: 10.0
            }
            .exit_code(),
            6
        );
        assert_eq!(AixError::NotImplemented("example").exit_code(), 1);
    }
}
