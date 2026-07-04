use crate::cache::Cache;
use crate::cli::CacheAction;
use crate::config;
use color_eyre::Result;
use std::path::PathBuf;

pub fn run(action: CacheAction, config_path: Option<PathBuf>) -> Result<()> {
    match action {
        CacheAction::Clear => {
            let cache_cfg = match config::find_config_path(config_path.as_deref())? {
                Some(p) => config::load(&p)?.cache,
                None => config::CacheConfig::default(),
            };
            let cache = Cache::from_config(&cache_cfg);
            let count = cache
                .clear()
                .map_err(|e| color_eyre::eyre::eyre!("failed to clear cache: {e}"))?;
            if count == 0 {
                println!("Cache already empty.");
            } else {
                println!("Cleared {count} cached file(s).");
            }
            Ok(())
        }
    }
}
