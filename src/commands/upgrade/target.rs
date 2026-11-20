//! Persisted update target for standalone installations.

use super::release::Version;
use color_eyre::eyre::{bail, eyre, WrapErr};
use color_eyre::Result;
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) const STATE_FILE: &str = ".aix-update-state";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Stable,
    Nightly,
    Release(Version),
}

impl Target {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "stable" => Ok(Self::Stable),
            "nightly" => Ok(Self::Nightly),
            _ => {
                let Some(version) = value.strip_prefix("release:") else {
                    bail!("invalid aix update target {value:?}");
                };
                let version = Version::parse_stable_tag(version)
                    .ok_or_else(|| eyre!("invalid stable release tag {version:?}"))?;
                Ok(Self::Release(version))
            }
        }
    }

    pub(crate) fn as_state_value(&self) -> String {
        match self {
            Self::Stable => "stable".to_owned(),
            Self::Nightly => "nightly".to_owned(),
            Self::Release(version) => format!("release:v{version}"),
        }
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Self::Stable => "latest stable".to_owned(),
            Self::Nightly => "nightly".to_owned(),
            Self::Release(version) => format!("release v{version}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Installed {
    Unknown,
    Stable,
    Nightly(String),
    Release(Version),
}

impl Installed {
    fn parse(value: &str) -> Result<Self> {
        if value == "unknown" {
            return Ok(Self::Unknown);
        }
        if value == "stable" {
            return Ok(Self::Stable);
        }
        if let Some(build) = value.strip_prefix("nightly:") {
            if !is_build_id(build) {
                bail!("invalid nightly build ID in aix update state");
            }
            return Ok(Self::Nightly(build.to_owned()));
        }
        if let Some(tag) = value.strip_prefix("release:") {
            let version = Version::parse_stable_tag(tag)
                .ok_or_else(|| eyre!("invalid installed release in aix update state"))?;
            return Ok(Self::Release(version));
        }
        bail!("invalid installed build in aix update state");
    }

    pub(crate) fn as_state_value(&self) -> String {
        match self {
            Self::Unknown => "unknown".to_owned(),
            Self::Stable => "stable".to_owned(),
            Self::Nightly(build) => format!("nightly:{build}"),
            Self::Release(version) => format!("release:v{version}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UpgradeState {
    pub(crate) target: Target,
    pub(crate) installed: Option<Installed>,
}

impl Default for UpgradeState {
    fn default() -> Self {
        Self {
            target: Target::Stable,
            installed: None,
        }
    }
}

impl UpgradeState {
    pub(crate) fn read(dir: &Path) -> Result<Self> {
        let path = state_path(dir);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(error).wrap_err_with(|| format!("could not read {}", path.display()));
            }
        };

        let mut target = None;
        let mut installed = None;
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| eyre!("invalid line in {}", path.display()))?;
            match key {
                "target" if target.is_none() => target = Some(Target::parse(value)?),
                "installed" if installed.is_none() => installed = Some(Installed::parse(value)?),
                "target" | "installed" => {
                    bail!("duplicate {key} in {}", path.display());
                }
                _ => {
                    bail!("unknown key {key:?} in {}", path.display());
                }
            }
        }

        Ok(Self {
            target: target.unwrap_or(Target::Stable),
            installed,
        })
    }

    pub(crate) fn write(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)
            .wrap_err_with(|| format!("could not access install directory {}", dir.display()))?;
        let path = state_path(dir);
        let mut file = tempfile::NamedTempFile::new_in(dir)
            .wrap_err_with(|| format!("could not create update state in {}", dir.display()))?;
        writeln!(file, "target={}", self.target.as_state_value())
            .wrap_err("could not write aix update target")?;
        if let Some(installed) = &self.installed {
            writeln!(file, "installed={}", installed.as_state_value())
                .wrap_err("could not write installed aix build")?;
        }
        file.persist(&path)
            .map_err(|error| error.error)
            .wrap_err_with(|| format!("could not save {}", path.display()))?;
        Ok(())
    }
}

pub(crate) fn state_path(dir: &Path) -> PathBuf {
    dir.join(STATE_FILE)
}

fn is_build_id(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
