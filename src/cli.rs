use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "aix", version, about = "profile-aware wrapper for AI tools")]
pub struct Cli {
    /// Profile to use (overrides AIX_PROFILE env var)
    #[arg(long, short, global = true, env = "AIX_PROFILE")]
    pub profile: Option<String>,

    /// Path to config file (overrides AIX_CONFIG env var)
    #[arg(long, global = true, env = "AIX_CONFIG")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// List available profiles
    Profiles {
        /// Emit profiles as a JSON array (no secrets)
        #[arg(long)]
        json: bool,
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
        /// Output raw JSON instead of formatted text
        #[arg(long)]
        json: bool,
        /// Always fetch fresh data, bypassing the cache (result is still cached)
        #[arg(long)]
        no_cache: bool,
    },
    /// Manage the local response cache
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
    /// Run any AI tool binary with profile environment set.
    /// Uses Anthropic credentials for `claude`, OpenAI credentials for everything else.
    /// Usage: aix <tool> [PROFILE] [--dry-run] [-- TOOL_ARGS...]
    #[command(external_subcommand)]
    Tool(Vec<String>),
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
