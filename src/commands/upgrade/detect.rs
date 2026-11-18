//! Work out how the running `aix` binary was installed.
//!
//! Detection looks only at where the binary lives and at what the local
//! package databases say about that path. Everything that touches the system
//! goes through [`Probe`] so the rules can be tested without a real install.

use std::path::{Path, PathBuf};

/// Who owns a file under `/usr` according to the system package database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SystemOwner {
    /// `pacman -Qo` (Arch; the AUR package is `aix-rs-bin`).
    Pacman(String),
    /// `dpkg-query -S` (Debian, Ubuntu, …).
    Dpkg(String),
    /// `rpm -qf` (Fedora, RHEL, openSUSE, …).
    Rpm(String),
}

/// How a `cargo install`-style install was recorded in `.crates2.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CargoSource {
    /// Installed from crates.io with `cargo install aix-rs`.
    Registry,
    /// Installed with `cargo binstall aix-rs`.
    Binstall,
    /// Installed from a local checkout or a git URL; the source string is kept
    /// so the guidance can show it.
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InstallMethod {
    /// Lives in the Nix store; managed declaratively, never touched by aix.
    Nix {
        store_path: PathBuf,
    },
    Homebrew,
    Scoop,
    Winget,
    /// `.deb` from a GitHub release, installed with apt/dpkg.
    Deb,
    /// `.rpm` from a GitHub release, installed with dnf/zypper/rpm.
    Rpm,
    /// Arch package (the AUR's `aix-rs-bin`).
    Aur {
        package: String,
    },
    /// `cargo install` / `cargo binstall` into `root` (the dir that holds `bin/`).
    Cargo {
        root: PathBuf,
        source: CargoSource,
    },
    /// Built in a source checkout (`target/debug` or `target/release`).
    SourceBuild,
    /// A plain binary: the installer scripts or a manual download.
    Standalone {
        dir: PathBuf,
        writable: bool,
    },
}

impl InstallMethod {
    pub(crate) fn describe(&self) -> &'static str {
        match self {
            Self::Nix { .. } => "Nix",
            Self::Homebrew => "Homebrew",
            Self::Scoop => "Scoop",
            Self::Winget => "winget",
            Self::Deb => ".deb package",
            Self::Rpm => ".rpm package",
            Self::Aur { .. } => "AUR (pacman)",
            Self::Cargo {
                source: CargoSource::Binstall,
                ..
            } => "cargo binstall",
            Self::Cargo {
                source: CargoSource::Registry,
                ..
            } => "cargo install",
            Self::Cargo {
                source: CargoSource::Other(_),
                ..
            } => "cargo install (local or git source)",
            Self::SourceBuild => "source build",
            Self::Standalone { .. } => "installer script or manual download",
        }
    }
}

/// System lookups needed by [`detect`].
pub(crate) trait Probe {
    fn system_owner(&self, path: &Path) -> Option<SystemOwner>;
    fn read_to_string(&self, path: &Path) -> Option<String>;
    fn cargo_home(&self) -> Option<PathBuf>;
    fn is_writable_dir(&self, dir: &Path) -> bool;
}

pub(crate) fn detect(exe: &Path, probe: &dyn Probe) -> InstallMethod {
    // Compare on a normalised, lower-case, forward-slash copy so that Windows
    // paths are matched by the same rules (and testable on any OS).
    let norm = normalize(exe);

    if let Some(store_path) = nix_store_root(exe) {
        return InstallMethod::Nix { store_path };
    }
    if norm.contains("/cellar/aix/") {
        return InstallMethod::Homebrew;
    }
    if norm.contains("/apps/aix/") && norm.contains("scoop") {
        return InstallMethod::Scoop;
    }
    if norm.contains("/winget/packages/") || norm.contains("/winget/links/") {
        return InstallMethod::Winget;
    }
    if is_cargo_target_dir(&norm) {
        return InstallMethod::SourceBuild;
    }
    if let Some(method) = detect_cargo(exe, probe) {
        return method;
    }
    if norm.starts_with("/usr/") {
        match probe.system_owner(exe) {
            Some(SystemOwner::Pacman(package)) => return InstallMethod::Aur { package },
            Some(SystemOwner::Dpkg(_)) => return InstallMethod::Deb,
            Some(SystemOwner::Rpm(_)) => return InstallMethod::Rpm,
            None => {}
        }
    }

    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let writable = probe.is_writable_dir(&dir);
    InstallMethod::Standalone { dir, writable }
}

fn normalize(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

/// `/nix/store/<hash>-aix-rs-1.0.0/bin/aix` → `/nix/store/<hash>-aix-rs-1.0.0`.
fn nix_store_root(exe: &Path) -> Option<PathBuf> {
    let text = exe.to_string_lossy();
    let rest = text.strip_prefix("/nix/store/")?;
    let entry = rest.split('/').next().filter(|entry| !entry.is_empty())?;
    Some(Path::new("/nix/store").join(entry))
}

/// `…/target/debug/aix`, `…/target/release/aix`, or
/// `…/target/<triple>/release/aix`.
fn is_cargo_target_dir(norm: &str) -> bool {
    let segments: Vec<&str> = norm.split('/').collect();
    let Some(dir_segments) = segments.len().checked_sub(1) else {
        return false;
    };
    segments[..dir_segments]
        .iter()
        .enumerate()
        .filter(|(_, segment)| **segment == "target")
        .any(|(index, _)| {
            segments[index + 1..dir_segments]
                .iter()
                .take(2)
                .any(|segment| matches!(*segment, "debug" | "release"))
        })
}

/// A binary in `<root>/bin` with an `aix-rs` entry in `<root>/.crates2.json`.
/// That covers the default `~/.cargo` as well as `cargo install --root`.
fn detect_cargo(exe: &Path, probe: &dyn Probe) -> Option<InstallMethod> {
    let bin_dir = exe.parent()?;
    if !bin_dir
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("bin"))
    {
        return None;
    }
    let root = bin_dir.parent()?.to_path_buf();
    let crates2 = probe.read_to_string(&root.join(".crates2.json"))?;
    let source = crates2_source(&crates2)?;

    let source = if source.starts_with("registry+") || source.starts_with("sparse+") {
        let binstall_files = [Some(root.clone()), probe.cargo_home()];
        let binstalled = binstall_files.iter().flatten().any(|dir| {
            probe
                .read_to_string(&dir.join("binstall").join("crates-v1.json"))
                .is_some_and(|text| text.contains("\"aix-rs\""))
        });
        if binstalled {
            CargoSource::Binstall
        } else {
            CargoSource::Registry
        }
    } else {
        CargoSource::Other(source)
    };
    Some(InstallMethod::Cargo { root, source })
}

/// Pull the source out of a `.crates2.json` key such as
/// `"aix-rs 1.0.0 (registry+https://github.com/rust-lang/crates.io-index)"`.
fn crates2_source(crates2: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(crates2).ok()?;
    value.get("installs")?.as_object()?.keys().find_map(|key| {
        let rest = key.strip_prefix("aix-rs ")?;
        let open = rest.find('(')?;
        let close = rest.rfind(')')?;
        Some(rest[open + 1..close].to_string())
    })
}

/// The real system: runs package-manager queries and reads files.
pub(crate) struct SystemProbe;

impl Probe for SystemProbe {
    fn system_owner(&self, path: &Path) -> Option<SystemOwner> {
        let path = path.to_string_lossy();
        if let Some(name) = query("pacman", &["-Qqo", &path]) {
            return Some(SystemOwner::Pacman(name));
        }
        if let Some(line) = query("dpkg-query", &["-S", &path]) {
            // `aix: /usr/bin/aix` (possibly `aix:amd64: …`)
            let name = line.split(':').next().unwrap_or_default().trim();
            if !name.is_empty() {
                return Some(SystemOwner::Dpkg(name.to_string()));
            }
        }
        if let Some(name) = query("rpm", &["-qf", "--queryformat", "%{NAME}", &path]) {
            return Some(SystemOwner::Rpm(name));
        }
        None
    }

    fn read_to_string(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn cargo_home(&self) -> Option<PathBuf> {
        std::env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .or_else(|| directories::BaseDirs::new().map(|dirs| dirs.home_dir().join(".cargo")))
    }

    fn is_writable_dir(&self, dir: &Path) -> bool {
        tempfile::Builder::new()
            .prefix(".aix-upgrade-probe")
            .tempfile_in(dir)
            .is_ok()
    }
}

/// Run a query command; return its first stdout line if it succeeded.
fn query(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeProbe {
        owner: Option<SystemOwner>,
        files: HashMap<PathBuf, String>,
        cargo_home: Option<PathBuf>,
        writable: bool,
    }

    impl Probe for FakeProbe {
        fn system_owner(&self, _: &Path) -> Option<SystemOwner> {
            self.owner.clone()
        }
        fn read_to_string(&self, path: &Path) -> Option<String> {
            self.files.get(path).cloned()
        }
        fn cargo_home(&self) -> Option<PathBuf> {
            self.cargo_home.clone()
        }
        fn is_writable_dir(&self, _: &Path) -> bool {
            self.writable
        }
    }

    fn detect_with(exe: &str, probe: &FakeProbe) -> InstallMethod {
        detect(Path::new(exe), probe)
    }

    const REGISTRY_CRATES2: &str = r#"{"installs":{"aix-rs 1.0.0 (registry+https://github.com/rust-lang/crates.io-index)":{"bins":["aix"]}}}"#;

    #[test]
    fn nix_store_binaries_are_nix() {
        let method = detect_with(
            "/nix/store/abc123-aix-rs-1.0.0/bin/aix",
            &FakeProbe::default(),
        );
        assert_eq!(
            method,
            InstallMethod::Nix {
                store_path: PathBuf::from("/nix/store/abc123-aix-rs-1.0.0")
            }
        );
    }

    #[test]
    fn homebrew_cellar_on_macos_and_linux() {
        for exe in [
            "/opt/homebrew/Cellar/aix/1.0.0/bin/aix",
            "/usr/local/Cellar/aix/1.0.0/bin/aix",
            "/home/linuxbrew/.linuxbrew/Cellar/aix/1.0.0/bin/aix",
        ] {
            assert_eq!(
                detect_with(exe, &FakeProbe::default()),
                InstallMethod::Homebrew,
                "{exe}"
            );
        }
    }

    #[test]
    fn scoop_and_winget_paths() {
        assert_eq!(
            detect_with(
                r"C:\Users\me\scoop\apps\aix\current\aix.exe",
                &FakeProbe::default()
            ),
            InstallMethod::Scoop
        );
        assert_eq!(
            detect_with(
                r"C:\Users\me\AppData\Local\Microsoft\WinGet\Packages\get-aix.aix_Microsoft.Winget.Source_8wekyb3d8bbwe\aix.exe",
                &FakeProbe::default()
            ),
            InstallMethod::Winget
        );
        assert_eq!(
            detect_with(
                r"C:\Users\me\AppData\Local\Microsoft\WinGet\Links\aix.exe",
                &FakeProbe::default()
            ),
            InstallMethod::Winget
        );
    }

    #[test]
    fn source_builds_are_recognised() {
        for exe in [
            "/home/me/aix/target/debug/aix",
            "/home/me/aix/target/release/aix",
            "/home/me/aix/target/x86_64-unknown-linux-musl/release/aix",
            r"C:\src\aix\target\release\aix.exe",
        ] {
            assert_eq!(
                detect_with(exe, &FakeProbe::default()),
                InstallMethod::SourceBuild,
                "{exe}"
            );
        }
        // A directory merely called `target` is not a cargo target dir.
        assert!(matches!(
            detect_with("/srv/target/bin/aix", &FakeProbe::default()),
            InstallMethod::Standalone { .. }
        ));
    }

    #[test]
    fn cargo_install_from_registry() {
        let mut probe = FakeProbe::default();
        probe.files.insert(
            PathBuf::from("/home/me/.cargo/.crates2.json"),
            REGISTRY_CRATES2.to_string(),
        );
        assert_eq!(
            detect_with("/home/me/.cargo/bin/aix", &probe),
            InstallMethod::Cargo {
                root: PathBuf::from("/home/me/.cargo"),
                source: CargoSource::Registry
            }
        );
    }

    #[test]
    fn cargo_binstall_is_told_apart_from_cargo_install() {
        let mut probe = FakeProbe {
            cargo_home: Some(PathBuf::from("/home/me/.cargo")),
            ..FakeProbe::default()
        };
        probe.files.insert(
            PathBuf::from("/home/me/.cargo/.crates2.json"),
            REGISTRY_CRATES2.to_string(),
        );
        probe.files.insert(
            PathBuf::from("/home/me/.cargo/binstall/crates-v1.json"),
            r#"{"name":"aix-rs","version_req":"*"}"#.to_string(),
        );
        assert_eq!(
            detect_with("/home/me/.cargo/bin/aix", &probe),
            InstallMethod::Cargo {
                root: PathBuf::from("/home/me/.cargo"),
                source: CargoSource::Binstall
            }
        );
    }

    #[test]
    fn cargo_install_from_a_checkout_keeps_the_source() {
        let mut probe = FakeProbe::default();
        probe.files.insert(
            PathBuf::from("/home/me/.cargo/.crates2.json"),
            r#"{"installs":{"aix-rs 1.0.0 (path+file:///home/me/aix)":{}}}"#.to_string(),
        );
        assert_eq!(
            detect_with("/home/me/.cargo/bin/aix", &probe),
            InstallMethod::Cargo {
                root: PathBuf::from("/home/me/.cargo"),
                source: CargoSource::Other("path+file:///home/me/aix".to_string())
            }
        );
    }

    #[test]
    fn cargo_bin_without_an_aix_entry_is_standalone() {
        let mut probe = FakeProbe {
            writable: true,
            ..FakeProbe::default()
        };
        probe.files.insert(
            PathBuf::from("/home/me/.cargo/.crates2.json"),
            r#"{"installs":{"ripgrep 14.0.0 (registry+x)":{}}}"#.to_string(),
        );
        assert_eq!(
            detect_with("/home/me/.cargo/bin/aix", &probe),
            InstallMethod::Standalone {
                dir: PathBuf::from("/home/me/.cargo/bin"),
                writable: true
            }
        );
    }

    #[test]
    fn system_packages_under_usr() {
        let cases = [
            (
                SystemOwner::Pacman("aix-rs-bin".into()),
                InstallMethod::Aur {
                    package: "aix-rs-bin".into(),
                },
            ),
            (SystemOwner::Dpkg("aix".into()), InstallMethod::Deb),
            (SystemOwner::Rpm("aix".into()), InstallMethod::Rpm),
        ];
        for (owner, expected) in cases {
            let probe = FakeProbe {
                owner: Some(owner),
                ..FakeProbe::default()
            };
            assert_eq!(detect_with("/usr/bin/aix", &probe), expected);
        }
    }

    #[test]
    fn unowned_binaries_are_standalone() {
        let probe = FakeProbe {
            writable: false,
            ..FakeProbe::default()
        };
        assert_eq!(
            detect_with("/usr/local/bin/aix", &probe),
            InstallMethod::Standalone {
                dir: PathBuf::from("/usr/local/bin"),
                writable: false
            }
        );
        let probe = FakeProbe {
            writable: true,
            ..FakeProbe::default()
        };
        assert_eq!(
            detect_with("/home/me/.local/bin/aix", &probe),
            InstallMethod::Standalone {
                dir: PathBuf::from("/home/me/.local/bin"),
                writable: true
            }
        );
    }
}
