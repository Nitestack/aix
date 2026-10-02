use std::process::ExitCode;

mod app;
mod cache;
mod cli;
mod commands;
mod config;
mod duration;
mod error;
mod gateway;
mod inference;
mod output;
mod run_history;
mod secrets;

#[tokio::main]
async fn main() -> ExitCode {
    app::run().await
}
