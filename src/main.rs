use std::process::ExitCode;

mod app;
mod cache;
mod cli;
mod commands;
mod config;
mod error;
mod gateway;
mod output;
mod secrets;

#[tokio::main]
async fn main() -> ExitCode {
    app::run().await
}
