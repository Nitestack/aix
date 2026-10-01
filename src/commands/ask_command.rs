//! Text-only inference delegated to a command that manages its own subscription login.

use super::env::append_custom_vars;
use super::launch::{command_with_env, native_clear_vars, LaunchEnv};
use crate::config::Profile;
use crate::error::AixError;
use serde_json::Value;
use std::io::Write;
use std::process::Stdio;

pub fn run(
    profile: &Profile,
    profile_name: &str,
    model: &str,
    messages: &[Value],
) -> Result<String, AixError> {
    let backend = profile
        .ask
        .as_ref()
        .ok_or(AixError::NativeAskNotConfigured)?;
    let system = messages
        .iter()
        .find(|message| message["role"] == "system")
        .and_then(|message| message["content"].as_str())
        .unwrap_or("You are a helpful assistant.");
    let input = messages
        .iter()
        .filter(|message| message["role"] != "system")
        .filter_map(|message| message["content"].as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let args = backend
        .args
        .iter()
        .map(|arg| arg.replace("{model}", model).replace("{system}", system))
        .collect::<Vec<_>>();
    let mut env = LaunchEnv {
        vars: vec![("AIX_PROFILE".to_string(), profile_name.to_string())],
        clear_vars: native_clear_vars(),
        remove_vars: Vec::new(),
        profile_name: profile_name.to_string(),
    };
    append_custom_vars(&mut env.vars, &profile.env)?;
    let mut command = command_with_env(&backend.command, &args, &env)?;
    // Backend diagnostics may contain credentials or prompt contents. Report only its status.
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|source| AixError::ProcessSpawn {
        program: backend.command.clone(),
        source,
    })?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    // Drain stdout while feeding stdin: large context and output must not deadlock each other.
    let (written, output) = std::thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(input.as_bytes()));
        let output = child.wait_with_output();
        (writer.join().expect("stdin writer panicked"), output)
    });
    let output = output.map_err(|source| AixError::ProcessWait {
        program: backend.command.clone(),
        source,
    })?;
    if !output.status.success() {
        return Err(AixError::AskCommandFailed {
            program: backend.command.clone(),
            exit_code: output.status.code(),
        });
    }
    written.map_err(|source| AixError::ProcessWait {
        program: backend.command.clone(),
        source,
    })?;
    let text = String::from_utf8(output.stdout).map_err(|_| AixError::AskCommandOutput)?;
    if text.trim().is_empty() {
        return Err(AixError::AskCommandOutput);
    }
    Ok(text)
}
