use crate::cache::Cache;
use crate::cli::CacheAction;
use crate::config::CacheConfig;
use color_eyre::Result;

pub fn run(action: CacheAction) -> Result<()> {
    match action {
        CacheAction::Clear => {
            let cache = Cache::from_config(&CacheConfig::default());
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
