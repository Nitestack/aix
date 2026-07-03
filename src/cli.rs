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
    /// Run the claude CLI with profile environment set
    Claude {
        /// Profile name (positional; overrides the global --profile flag)
        profile: Option<String>,
        /// Print the command and variable names that would be set, without running
        #[arg(long)]
        dry_run: bool,
        /// Arguments to pass to claude (after --)
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Run the pi CLI with profile environment set
    Pi {
        /// Profile name (positional; overrides the global --profile flag)
        profile: Option<String>,
        /// Print the command and variable names that would be set, without running
        #[arg(long)]
        dry_run: bool,
        /// Arguments to pass to pi (after --)
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Print the resolved config file path
    Path,
    /// Validate the config file
    Validate,
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
