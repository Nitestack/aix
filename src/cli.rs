use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "aix",
    version,
    about = "profile-aware AI gateway client and tool launcher"
)]
pub struct Cli {
    /// Profile to use (overrides AIX_PROFILE env var)
    #[arg(long, short, global = true)]
    pub profile: Option<String>,

    /// Do not open an interactive profile picker
    #[arg(long, global = true)]
    pub non_interactive: bool,

    /// Maximum duration for each aix-owned HTTP request (for example, 750ms or 5s)
    #[arg(long, global = true, value_name = "DURATION", default_value = "30s", value_parser = parse_duration)]
    pub timeout: Duration,

    /// Path to config file (overrides AIX_CONFIG env var)
    #[arg(long, global = true, env = "AIX_CONFIG")]
    pub config: Option<PathBuf>,

    /// Emit a stable JSON envelope when supported by the command
    #[arg(long, global = true)]
    pub json: bool,

    /// Report a failed command on stderr as text or as one JSON object
    #[arg(
        long,
        global = true,
        value_enum,
        value_name = "FORMAT",
        default_value = "text"
    )]
    pub error_format: ErrorFormat,

    #[command(subcommand)]
    pub command: Command,
}

/// Cache age limit in whole seconds: `0`, a bare number of seconds, or a
/// number with `s`, `m`, `h`, or `d`.
fn parse_max_age(value: &str) -> Result<u64, String> {
    let invalid = || "expected a cache age such as 0, 60s, 5m, 1h, or 1d".to_string();
    let (number, multiplier) = match value.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        None => (value, 1),
        Some((index, _)) => {
            let multiplier = match &value[index..] {
                "s" => 1,
                "m" => 60,
                "h" => 60 * 60,
                "d" => 24 * 60 * 60,
                _ => return Err(invalid()),
            };
            (&value[..index], multiplier)
        }
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|amount| amount.checked_mul(multiplier))
        .ok_or_else(invalid)
}

fn parse_duration(value: &str) -> Result<Duration, String> {
    let (number, unit) = if let Some(number) = value.strip_suffix("ms") {
        (number, "ms")
    } else if let Some(number) = value.strip_suffix('s') {
        (number, "s")
    } else if let Some(number) = value.strip_suffix('m') {
        (number, "m")
    } else {
        return Err("expected a duration such as 750ms, 5s, or 2m".to_string());
    };
    let amount = number
        .parse::<u64>()
        .map_err(|_| "duration must be a positive whole number with ms, s, or m".to_string())?;
    if amount == 0 {
        return Err("duration must be greater than zero".to_string());
    }

    match unit {
        "ms" => Ok(Duration::from_millis(amount)),
        "s" => Ok(Duration::from_secs(amount)),
        "m" => amount
            .checked_mul(60)
            .map(Duration::from_secs)
            .ok_or_else(|| "duration is too large".to_string()),
        _ => unreachable!("unit is selected above"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ErrorFormat {
    Text,
    Json,
}

#[derive(Subcommand)]
pub enum Command {
    /// Print shell code that installs the aix shell wrapper
    Init {
        /// Shell to generate integration code for
        shell: Shell,
    },
    /// Select or clear the active shell profile
    Use {
        /// Profile name to select
        #[arg(required_unless_present = "clear", conflicts_with = "clear")]
        profile: Option<String>,
        /// Unset AIX_PROFILE in the current shell
        #[arg(long)]
        clear: bool,
        /// Shell syntax for the assignment (defaults to sh)
        #[arg(long, value_enum, conflicts_with = "format")]
        shell: Option<Shell>,
        /// Output encoding instead of shell assignment syntax
        #[arg(long, value_enum, conflicts_with = "shell")]
        format: Option<UseFormat>,
    },
    /// Show the effective profile without opening the interactive picker
    Current {
        /// Output format; `short` is the same one-line output as the default
        #[arg(long, value_enum)]
        format: Option<CurrentFormat>,
    },
    /// List available profiles
    Profiles,
    /// List configured run policies
    Policies,
    /// Inspect a configured run policy
    Policy {
        #[command(subcommand)]
        action: PolicyAction,
    },
    /// Print environment variables for the selected profile
    Env {
        /// Profile name (positional; overrides the global --profile flag)
        profile: Option<String>,
        /// Output format
        #[arg(long, value_enum, default_value = "sh")]
        format: EnvFormat,
    },
    /// Launch a shell with profile environment set
    Shell {
        /// Profile name (positional; overrides the global --profile flag)
        profile: Option<String>,
        /// Print the shell command and variable names that would be set, without running
        #[arg(long)]
        dry_run: bool,
        /// Arguments after -- are not accepted; use `aix exec` to run a command directly
        #[arg(last = true, hide = true)]
        extra_args: Vec<String>,
    },
    /// Execute a command with profile environment set
    Exec {
        /// Profile name (positional; overrides the global --profile flag)
        profile: Option<String>,
        /// Print the command and variable names that would be set, without running
        #[arg(long)]
        dry_run: bool,
        /// Command and arguments to run (after --)
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Manage ChatGPT authentication for a profile
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// Show current key spend and budget information from the gateway
    Spend {
        /// Profile name (positional; overrides the global --profile flag)
        profile: Option<String>,
        /// Always fetch fresh data, bypassing the cache (result is still cached)
        #[arg(long, visible_alias = "refresh", conflicts_with = "max_age")]
        no_cache: bool,
        /// Use cached budget data only if younger than this (for example 60s, 5m, 1h; 0 always refreshes)
        #[arg(long, value_name = "DURATION", value_parser = parse_max_age)]
        max_age: Option<u64>,
    },
    /// Show profile, gateway connectivity, and budget status
    Status {
        /// Profile name (positional; overrides the global --profile flag)
        profile: Option<String>,
        /// Refresh spend data instead of using a cached response
        #[arg(long, visible_alias = "no-cache", conflicts_with = "max_age")]
        refresh: bool,
        /// Use cached budget data only if younger than this (for example 60s, 5m, 1h; 0 always refreshes)
        #[arg(long, value_name = "DURATION", value_parser = parse_max_age)]
        max_age: Option<u64>,
    },
    /// Check whether a configured run policy can execute right now
    Gate {
        /// Run policy to preflight
        #[arg(long, required = true)]
        policy: String,
    },
    /// Diagnose config, credentials, gateway capabilities, and cache access
    Doctor {
        /// Profile name (positional; overrides the global --profile flag)
        profile: Option<String>,
    },
    /// List model IDs exposed by the selected OpenAI-compatible gateway
    Models {
        /// Profile name (positional; overrides the global --profile flag)
        profile: Option<String>,
        /// Filter model IDs by a case-insensitive substring
        #[arg(long, value_name = "TEXT")]
        filter: Option<String>,
    },
    /// Show locally observed usage; no upstream history is requested
    Usage {
        /// Profile name (positional; overrides the global --profile flag)
        profile: Option<String>,
        #[command(flatten)]
        date_range: UsageRangeArgs,
        /// Filter usage to an exact model ID
        #[arg(long)]
        model: Option<String>,
        /// Filter local events to an exact API protocol
        #[arg(long)]
        protocol: Option<String>,
        /// Filter local events to an exact logical tool name
        #[arg(long)]
        tool: Option<String>,
        /// Filter local events to an exact run ID
        #[arg(long = "run")]
        run_id: Option<String>,
    },
    /// Send a one-shot text request to the configured AI gateway
    Ask {
        /// Model name, alias, or raw model ID (defaults to the configured model)
        #[arg(long)]
        model: Option<String>,
        /// Optional system message sent before the user content
        #[arg(long)]
        system: Option<String>,
        /// Explicit file to include as context (may be repeated)
        #[arg(long = "file", value_name = "PATH")]
        files: Vec<PathBuf>,
        /// User instruction
        prompt: Option<String>,
    },
    /// Run a configured reusable prompt preset, or list available presets
    Prompt {
        /// Preset name
        #[arg(
            value_name = "NAME",
            required_unless_present = "list",
            conflicts_with = "list"
        )]
        name: Option<String>,
        /// List configured presets without printing their prompt text
        #[arg(long, conflicts_with_all = ["name", "model", "files"])]
        list: bool,
        /// Override the preset model (alias or raw model ID)
        #[arg(long, requires = "name")]
        model: Option<String>,
        /// Explicit file to include as context (may be repeated)
        #[arg(long = "file", value_name = "PATH", requires = "name")]
        files: Vec<PathBuf>,
    },
    /// Run a command with profile credentials and durable run history
    Run {
        /// Apply a declarative run policy with locally enforceable constraints
        #[arg(long)]
        policy: Option<String>,
        /// Optional human-readable run name
        #[arg(long)]
        name: Option<String>,
        /// Workflow label for this invocation
        #[arg(long)]
        workflow: Option<String>,
        /// External task or ticket identifier
        #[arg(long)]
        task_id: Option<String>,
        /// Repeatable run tag
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Lower the policy's maximum run duration, such as 30m or 2h
        #[arg(long, requires = "policy")]
        duration: Option<String>,
        /// Narrow the policy's allowed model IDs or aliases (repeatable)
        #[arg(long = "allow-model", requires = "policy")]
        allow_models: Vec<String>,
        /// Validate and describe policy enforcement without running the child
        #[arg(long, requires = "policy")]
        dry_run: bool,
        /// Command and arguments to run (after --)
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Inspect durable run history
    Runs {
        /// Show only the newest N records
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[command(subcommand)]
        action: Option<RunsAction>,
    },
    /// Upgrade aix to the latest release using the method it was installed with
    #[command(alias = "update")]
    Upgrade {
        /// Show how aix was installed and what an upgrade would do, without changing anything
        #[arg(long)]
        check: bool,
        /// Reinstall even when the installed version is already the latest
        /// (installer-script and .deb/.rpm installs only)
        #[arg(long, conflicts_with = "check")]
        force: bool,
    },
    /// Manage the local response cache
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
    /// Run an AI tool using configured launch wiring or the legacy credential-format fallback.
    /// Usage: aix <tool> [PROFILE] [--dry-run] [-- TOOL_ARGS...]
    /// For the logical `opencode` tool, profiles may select native
    /// `tool_configs.opencode.config_file` and `cli_config_file` sources.
    /// Profile-selected sources are checked against OpenCode v2.0.20; see the
    /// Home Manager option descriptions for path and replacement behavior.
    #[command(external_subcommand)]
    Tool(Vec<String>),
}

#[derive(Args, Default)]
pub struct UsageRangeArgs {
    /// Number of calendar days to include, ending today (for example, 7d)
    #[arg(long, conflicts_with_all = ["start", "end"])]
    pub since: Option<String>,
    /// Inclusive start date in YYYY-MM-DD format (requires --end)
    #[arg(long, requires = "end", conflicts_with = "since")]
    pub start: Option<String>,
    /// Inclusive end date in YYYY-MM-DD format (requires --start)
    #[arg(long, requires = "start", conflicts_with = "since")]
    pub end: Option<String>,
}

impl Command {
    pub fn supports_json(&self) -> bool {
        matches!(
            self,
            Self::Current { .. }
                | Self::Profiles
                | Self::Policies
                | Self::Policy { .. }
                | Self::Spend { .. }
                | Self::Status { .. }
                | Self::Doctor { .. }
                | Self::Gate { .. }
                | Self::Models { .. }
                | Self::Usage { .. }
                | Self::Ask { .. }
                | Self::Prompt { .. }
                | Self::Runs { .. }
                | Self::Auth {
                    action: AuthAction::Status { .. }
                }
        )
    }

    /// The `command` field of this command's JSON envelope.
    pub fn json_name(&self) -> &'static str {
        match self {
            Self::Init { .. } => "init",
            Self::Use { .. } => "use",
            Self::Current { .. } => "current",
            Self::Profiles => "profiles",
            Self::Policies => "policies",
            Self::Policy { .. } => "policy show",
            Self::Env { .. } => "env",
            Self::Shell { .. } => "shell",
            Self::Exec { .. } => "exec",
            Self::Config { .. } => "config",
            Self::Auth {
                action: AuthAction::Status { .. },
            } => "auth status",
            Self::Auth {
                action: AuthAction::Login { .. },
            } => "auth login",
            Self::Auth {
                action: AuthAction::Logout { .. },
            } => "auth logout",
            Self::Spend { .. } => "spend",
            Self::Status { .. } => "status",
            Self::Gate { .. } => "gate",
            Self::Doctor { .. } => "doctor",
            Self::Models { .. } => "models",
            Self::Usage { .. } => "usage",
            Self::Ask { .. } => "ask",
            Self::Prompt { .. } => "prompt",
            Self::Run { .. } => "run",
            Self::Runs {
                action: Some(RunsAction::Show { .. }),
                ..
            } => "runs show",
            Self::Runs { .. } => "runs",
            Self::Upgrade { .. } => "upgrade",
            Self::Cache { .. } => "cache",
            Self::Tool(_) => "tool",
        }
    }
}

#[derive(Subcommand)]
pub enum AuthAction {
    /// Sign a ChatGPT account into a profile using the system browser
    Login {
        /// Profile name (positional; overrides global --profile and AIX_PROFILE)
        profile: Option<String>,
    },
    /// Show local authentication state without making network requests
    Status {
        /// Profile name (positional; overrides global --profile and AIX_PROFILE)
        profile: Option<String>,
    },
    /// Revoke and clear ChatGPT credentials for a profile
    Logout {
        /// Profile name (positional; overrides global --profile and AIX_PROFILE)
        profile: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Shell {
    Sh,
    Bash,
    Zsh,
    Fish,
    Nu,
    Powershell,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum UseFormat {
    Json,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum CurrentFormat {
    Short,
}

#[derive(Subcommand)]
pub enum RunsAction {
    /// Show one complete run record
    Show { run_id: String },
}

#[derive(Subcommand)]
pub enum PolicyAction {
    /// Show the complete configuration for one policy
    Show { name: String },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Print the resolved config file path
    Path,
    /// Validate the config file
    Validate,
}

#[derive(Subcommand)]
pub enum CacheAction {
    /// Delete all cached response files
    Clear,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum EnvFormat {
    Sh,
    Json,
    Nu,
    Fish,
    Powershell,
    Cmd,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_global_timeout_units() {
        let cli = Cli::try_parse_from(["aix", "--timeout", "750ms", "profiles"]).unwrap();
        assert_eq!(cli.timeout, std::time::Duration::from_millis(750));

        let cli = Cli::try_parse_from(["aix", "profiles", "--timeout", "5s"]).unwrap();
        assert_eq!(cli.timeout, std::time::Duration::from_secs(5));
    }

    #[test]
    fn global_timeout_defaults_to_thirty_seconds() {
        let cli = Cli::try_parse_from(["aix", "profiles"]).unwrap();
        assert_eq!(cli.timeout, std::time::Duration::from_secs(30));
    }

    #[test]
    fn timeout_must_be_a_positive_supported_duration() {
        for timeout in ["", "0s", "-1s", "750", "1msx"] {
            assert!(
                Cli::try_parse_from(["aix", "--timeout", timeout, "profiles"]).is_err(),
                "accepted invalid timeout {timeout:?}"
            );
        }
    }

    #[test]
    fn parses_budget_max_age_and_refresh_aliases() {
        for (value, seconds) in [("0", 0), ("90", 90), ("60s", 60), ("5m", 300), ("1h", 3600)] {
            let cli = Cli::try_parse_from(["aix", "status", "--max-age", value]).unwrap();
            let Command::Status { max_age, .. } = cli.command else {
                panic!("expected status");
            };
            assert_eq!(max_age, Some(seconds), "{value}");
        }
        for value in ["", "-1", "5x", "1mo", "s"] {
            assert!(
                Cli::try_parse_from(["aix", "spend", "--max-age", value]).is_err(),
                "accepted invalid max age {value:?}"
            );
        }
        assert!(Cli::try_parse_from(["aix", "spend", "--refresh"]).is_ok());
        assert!(Cli::try_parse_from(["aix", "status", "--no-cache"]).is_ok());
        assert!(Cli::try_parse_from(["aix", "spend", "--no-cache", "--max-age", "60s"]).is_err());
    }

    #[test]
    fn parses_non_interactive_global_flag_before_or_after_command() {
        assert!(
            Cli::try_parse_from(["aix", "--non-interactive", "profiles"])
                .unwrap()
                .non_interactive
        );
        assert!(
            Cli::try_parse_from(["aix", "profiles", "--non-interactive"])
                .unwrap()
                .non_interactive
        );
    }
}
