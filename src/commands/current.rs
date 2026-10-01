use crate::cli::CurrentFormat;
use crate::config;
use crate::output;
use color_eyre::Result;
use serde::Serialize;
use std::path::PathBuf;

pub fn run(config_path: Option<PathBuf>, json: bool, _format: Option<CurrentFormat>) -> Result<()> {
    let config_path = config::find_config_path(config_path.as_deref())?;
    let selected = match config_path {
        Some(path) => {
            let cfg = config::load(&path)?;
            let env_profile = std::env::var("AIX_PROFILE").ok();
            let selected = env_profile
                .filter(|name| cfg.profiles.contains_key(name))
                .map(|name| (name, CurrentSource::Env))
                .or_else(|| {
                    cfg.default_profile
                        .filter(|name| cfg.profiles.contains_key(name))
                        .map(|name| (name, CurrentSource::Default))
                });
            match selected {
                Some((name, source)) => {
                    let label = if json {
                        let profile = &cfg.profiles[&name];
                        Some(match &profile.label {
                            Some(value) => value.resolve()?,
                            None => name.clone(),
                        })
                    } else {
                        None
                    };
                    Some(CurrentProfile {
                        name,
                        label,
                        source,
                    })
                }
                None => None,
            }
        }
        None => None,
    };

    if json {
        let data = CurrentData {
            name: selected.as_ref().map(|profile| profile.name.clone()),
            label: selected.as_ref().and_then(|profile| profile.label.clone()),
            source: selected
                .as_ref()
                .map(|profile| profile.source)
                .unwrap_or(CurrentSource::None),
        };
        output::print_json("current", data)?;
    } else if let Some(profile) = selected {
        println!("{}", profile.name);
    } else {
        println!("none");
    }

    Ok(())
}

struct CurrentProfile {
    name: String,
    label: Option<String>,
    source: CurrentSource,
}

#[derive(Serialize)]
struct CurrentData {
    name: Option<String>,
    label: Option<String>,
    source: CurrentSource,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum CurrentSource {
    Env,
    Default,
    None,
}
