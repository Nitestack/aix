use crate::config;
use crate::error::AixError;
use color_eyre::Result;
use serde::Serialize;
use std::path::PathBuf;

pub fn run(config_path: Option<PathBuf>, json: bool) -> Result<()> {
    let path = config::find_config_path(config_path.as_deref())?.ok_or(AixError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;

    let profiles = config::sorted_profiles(&cfg);

    if json {
        print_json(&profiles)?;
    } else {
        print_text(&profiles);
    }
    Ok(())
}

#[derive(Serialize)]
struct ProfileEntry<'a> {
    name: &'a str,
    label: &'a str,
}

fn print_json(profiles: &[(&str, &str)]) -> Result<()> {
    let entries: Vec<ProfileEntry> = profiles
        .iter()
        .map(|(name, label)| ProfileEntry { name, label })
        .collect();
    let mut out = serde_json::to_string_pretty(&entries)?;
    out.push('\n');
    print!("{out}");
    Ok(())
}

fn print_text(profiles: &[(&str, &str)]) {
    for (name, label) in profiles {
        if name == label {
            println!("{name}");
        } else {
            println!("{name}  ({label})");
        }
    }
}
