use crate::commands::GatewayRequestOptions;
use crate::inference;
use color_eyre::Result;
use std::io::IsTerminal;
use std::path::PathBuf;

pub async fn run(
    options: GatewayRequestOptions,
    config_path: Option<PathBuf>,
    json_output: bool,
    requested_model: Option<String>,
    system: Option<String>,
    files: Vec<PathBuf>,
    prompt: Option<String>,
) -> Result<()> {
    inference::validate_input_shape(
        prompt.is_some(),
        !files.is_empty(),
        std::io::stdin().is_terminal(),
    )?;
    let cfg = inference::load_config(config_path.as_deref())?;
    let options = inference::prepare_request_options(&cfg, options)?;
    let messages = inference::collect_messages(system, prompt, &files)?;
    let result = inference::execute(&cfg, options, requested_model.as_deref(), messages).await?;
    inference::print_result("ask", json_output, result)
}
