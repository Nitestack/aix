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

    #[command(subcommand)]
    pub command: Command,
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
    /// Show spend and budget info for the selected profile (LiteLLM only)
    Spend {
        /// Profile name (positional; overrides the global --profile flag)
        profile: Option<String>,
        /// Always fetch fresh data, bypassing the cache (result is still cached)
        #[arg(long)]
        no_cache: bool,
    },
    /// Show profile, gateway connectivity, and budget status
    Status {
        /// Profile name (positional; overrides the global --profile flag)
        profile: Option<String>,
        /// Refresh spend data instead of using a cached response
        #[arg(long)]
        refresh: bool,
    },
    /// Check whether a configured run policy can execute right now
    ///
    /// Does not create a probe key to test key-generation permission. Actual lease creation may
    /// still fail if the parent credential lacks that permission.
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
    /// Show historical usage from the LiteLLM daily activity endpoint
    Usage {
        /// Profile name (positional; overrides the global --profile flag)
        profile: Option<String>,
        #[command(flatten)]
        date_range: UsageRangeArgs,
        /// Filter usage to an exact model ID returned by LiteLLM
        #[arg(long)]
        model: Option<String>,
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
        /// Apply a declarative run policy (always uses a scoped lease)
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
        /// Create and use a temporary LiteLLM virtual key for this run
        #[arg(long)]
        lease: bool,
        /// Maximum spend in USD (required with --lease)
        #[arg(long)]
        budget: Option<f64>,
        /// Lease duration, such as 30m or 2h (default: 2h)
        #[arg(long)]
        duration: Option<String>,
        /// Restrict the lease to a model ID or configured alias (repeatable)
        #[arg(long = "allow-model")]
        allow_models: Vec<String>,
        /// Validate and describe a lease without creating it or running the child
        #[arg(long)]
        dry_run: bool,
        /// Command and arguments to run (after --)
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Create, inspect, or revoke an exported LiteLLM credential lease
    Lease {
        #[command(subcommand)]
        action: LeaseAction,
    },
    /// List locally recorded exported credential leases
    Leases,
    /// Inspect durable run history
    Runs {
        /// Show only the newest N records
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[command(subcommand)]
        action: Option<RunsAction>,
    },
    /// Manage the local response cache
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
    /// Run an AI tool using configured launch wiring or the legacy credential-format fallback.
    /// Usage: aix <tool> [PROFILE] [--dry-run] [-- TOOL_ARGS...]
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
                | Self::Leases
                | Self::Lease {
                    action: LeaseAction::Show { .. }
                }
                | Self::Runs { .. }
        )
    }
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
pub enum LeaseAction {
    /// Create a temporary gateway credential and write it to a secret file
    Create {
        /// Maximum spend in USD
        #[arg(long, required = true)]
        budget: f64,
        /// Lease duration, such as 30m or 2h (default: 2h)
        #[arg(long)]
        duration: Option<String>,
        /// Restrict the lease to a model ID or configured alias (repeatable)
        #[arg(long = "allow-model")]
        allow_models: Vec<String>,
        /// Attach a metadata tag (repeatable)
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// New secret file path (Unix mode 0600; elsewhere inherits ACLs and may use a non-atomic fallback)
        #[arg(long, required = true, value_name = "PATH")]
        output: PathBuf,
    },
    /// Show one locally recorded lease without contacting the gateway
    Show { lease_id: String },
    /// Revoke a locally recorded lease by its non-secret ID
    Revoke { lease_id: String },
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
