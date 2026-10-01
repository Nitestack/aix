use crate::inference;
use color_eyre::Result;
use std::path::PathBuf;

pub async fn run(
    selected_profile: Option<String>,
    config_path: Option<PathBuf>,
    json_output: bool,
    requested_model: Option<String>,
    system: Option<String>,
    files: Vec<PathBuf>,
    prompt: Option<String>,
) -> Result<()> {
    let messages = inference::collect_messages(system, prompt, &files)?;
    let cfg = inference::load_config(config_path.as_deref())?;
    let result =
        inference::execute(&cfg, selected_profile, requested_model.as_deref(), messages).await?;
    inference::print_result("ask", json_output, result)
}
