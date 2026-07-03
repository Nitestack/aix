use crate::cli::ConfigAction;
use crate::config;
use crate::error::AixError;
use color_eyre::Result;
use std::path::PathBuf;

pub fn run(action: ConfigAction, config_path: Option<PathBuf>) -> Result<()> {
    match action {
        ConfigAction::Path => match config::find_config_path(config_path.as_deref())? {
            Some(path) => {
                println!("{}", path.display());
                Ok(())
            }
            None => Err(AixError::NoConfigFile.into()),
        },
        ConfigAction::Validate => {
            let path =
                config::find_config_path(config_path.as_deref())?.ok_or(AixError::NoConfigFile)?;
            let cfg = config::load(&path)?;
            config::validate(&cfg)?;
            println!("Config is valid.");
            Ok(())
        }
    }
}
