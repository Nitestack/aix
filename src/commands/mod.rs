pub mod ask;
pub mod auth;
use std::time::Duration;

pub mod cache;
pub mod config;
pub mod current;
pub mod doctor;
pub mod env;
pub mod exec;
pub mod gate;
pub mod init;
pub mod launch;
pub mod leases;
pub mod models;
pub mod policies;
pub mod profiles;
pub mod prompt;
pub mod run;
pub(crate) mod run_lease;
pub mod runs;
pub mod shell;
pub mod spend;
pub mod status;
pub mod usage;

#[derive(Clone, Debug)]
pub(crate) struct ProfileSelection {
    pub(crate) profile: Option<String>,
    pub(crate) non_interactive: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct GatewayRequestOptions {
    pub(crate) selection: ProfileSelection,
    pub(crate) timeout: Duration,
}
pub mod use_profile;
