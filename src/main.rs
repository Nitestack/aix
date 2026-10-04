use std::process::ExitCode;

mod app;
mod auth;
mod cache;
mod cli;
mod commands;
mod config;
mod duration;
mod error;
mod gateway;
mod inference;
mod lease_registry;
mod local_gateway;
mod output;
mod run_history;
mod secrets;
mod usage_event;
mod usage_observer;

#[tokio::main]
async fn main() -> ExitCode {
    app::run().await
}
