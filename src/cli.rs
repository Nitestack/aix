use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "aix",
    version,
    about = "profile-aware AI gateway client and tool launcher"
)]
pub struct Cli {
    /// Profile to use (overrides AIX_PROFILE env var)
    #[arg(long, short, global = true, env = "AIX_PROFILE")]
    pub profile: Option<String>,

    /// Path to config file (overrides AIX_CONFIG env var)
    #[arg(long, global = true, env = "AIX_CONFIG")]
    pub config: Option<PathBuf>,

    /// Emit a stable JSON envelope when supported by the command
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
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
    /// Send a one-shot text request through the profile gateway or native ask command
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
    /// Run a command with profile credentials and durable run history
    Run {
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
                | Self::Spend { .. }
                | Self::Status { .. }
                | Self::Models { .. }
                | Self::Usage { .. }
                | Self::Ask { .. }
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
