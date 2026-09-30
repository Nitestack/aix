use crate::config;
use crate::error::AixError;
use crate::output;
use color_eyre::Result;
use serde::Serialize;
use std::path::PathBuf;

pub fn run(config_path: Option<PathBuf>, json: bool) -> Result<()> {
    let path = config::find_config_path(config_path.as_deref())?.ok_or(AixError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;

    let profiles = config::sorted_profiles(&cfg)?;

    if json {
        let entries: Vec<ProfileEntry<'_>> = profiles
            .iter()
            .map(|(name, label)| ProfileEntry { name, label })
            .collect();
        output::print_json("profiles", entries)?;
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

fn print_text(profiles: &[(&str, String)]) {
    for (name, label) in profiles {
        if *name == label.as_str() {
            println!("{name}");
        } else {
            println!("{name}  ({label})");
        }
    }
}
