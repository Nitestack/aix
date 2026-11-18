//! `aix upgrade`: upgrade aix through whatever installed it.
//!
//! 1. [`detect`] works out the install method from the binary's location.
//! 2. [`plan`] turns that into one action: run a package manager, install a
//!    release `.deb`/`.rpm`, re-run the installer script, or print guidance
//!    (Nix, source builds, and anything aix can't upgrade safely by itself).
//! 3. [`execute`] carries it out, or `--check` only describes it.

mod detect;
mod nix;
mod release;

use color_eyre::eyre::{bail, WrapErr};
use color_eyre::Result;
use detect::{CargoSource, InstallMethod};
use release::{needs_upgrade, Releases, Version, REPO_URL};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const CRATE_NAME: &str = "aix-rs";
const HOMEBREW_FORMULA: &str = "get-aix/tap/aix";
const WINGET_ID: &str = "get-aix.aix";

pub async fn run(check: bool, force: bool, timeout: Duration) -> Result<()> {
    let exe = current_exe()?;
    remove_leftover_binary(&exe);

    let method = detect::detect(&exe, &detect::SystemProbe);
    let current = Version::current();
    println!("aix {current} at {}", exe.display());
    println!("Installed via: {}", method.describe());

    let plan = plan(&method, &Env::system());
    if check {
        return report(&plan, current, timeout).await;
    }
    execute(plan, &exe, current, force, timeout).await
}

/// One external command. `elevate` prefixes sudo/doas when not root;
/// `handoff` runs it after aix exits (Windows package managers can't replace
/// a running exe).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Step {
    program: PathBuf,
    args: Vec<OsString>,
    elevate: bool,
    handoff: bool,
}

impl Step {
    fn new(program: PathBuf, args: &[&str]) -> Self {
        Self {
            program,
            args: args.iter().map(OsString::from).collect(),
            elevate: false,
            handoff: false,
        }
    }

    fn display(&self) -> String {
        let program = self
            .program
            .file_stem()
            .unwrap_or(self.program.as_os_str())
            .to_string_lossy();
        let mut words = vec![program.into_owned()];
        words.extend(
            self.args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned()),
        );
        let line = words.join(" ");
        if self.elevate {
            format!("sudo {line}")
        } else {
            line
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PackageKind {
    Deb,
    Rpm,
}

#[derive(Debug, PartialEq, Eq)]
enum Plan {
    /// A package manager that checks for and installs the new version itself.
    PackageManager { step: Step, note: Option<String> },
    /// Download the release `.deb`/`.rpm` and install it with `installer`
    /// (the package path is appended to its arguments).
    ReleasePackage { kind: PackageKind, installer: Step },
    /// Re-run the installer script into `dir`.
    Script { dir: PathBuf },
    /// aix can't (or shouldn't) do it; tell the user how.
    Guidance(String),
}

/// What [`plan`] needs to know about the machine.
/// Looks a program up on PATH.
type FindProgram = Box<dyn Fn(&str) -> Option<PathBuf>>;

struct Env {
    windows: bool,
    root: bool,
    arch: &'static str,
    find: FindProgram,
    cargo_home: Option<PathBuf>,
}

impl Env {
    fn system() -> Self {
        Self {
            windows: cfg!(windows),
            root: is_root(),
            arch: std::env::consts::ARCH,
            find: Box::new(|name| which::which(name).ok()),
            cargo_home: std::env::var_os("CARGO_HOME")
                .map(PathBuf::from)
                .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().join(".cargo"))),
        }
    }

    fn find(&self, name: &str) -> Option<PathBuf> {
        (self.find)(name)
    }

    /// The first available program among `names`.
    fn first(&self, names: &[&'static str]) -> Option<(&'static str, PathBuf)> {
        names
            .iter()
            .find_map(|name| self.find(name).map(|path| (*name, path)))
    }
}

fn plan(method: &InstallMethod, env: &Env) -> Plan {
    match method {
        InstallMethod::Nix { store_path } => {
            Plan::Guidance(nix::guidance(store_path, &nix::origin(store_path)))
        }
        InstallMethod::Homebrew => match env.find("brew") {
            Some(brew) => Plan::PackageManager {
                step: Step::new(brew, &["upgrade", HOMEBREW_FORMULA]),
                note: None,
            },
            None => missing_tool("brew", &format!("brew upgrade {HOMEBREW_FORMULA}")),
        },
        InstallMethod::Scoop => match env.find("scoop") {
            Some(scoop) => Plan::PackageManager {
                step: Step {
                    handoff: env.windows,
                    ..Step::new(scoop, &["update", "aix"])
                },
                note: None,
            },
            None => missing_tool("scoop", "scoop update aix"),
        },
        InstallMethod::Winget => match env.find("winget") {
            Some(winget) => Plan::PackageManager {
                step: Step {
                    handoff: env.windows,
                    ..Step::new(winget, &["upgrade", "--id", WINGET_ID, "--exact"])
                },
                note: None,
            },
            None => missing_tool(
                "winget",
                &format!("winget upgrade --id {WINGET_ID} --exact"),
            ),
        },
        InstallMethod::Deb => plan_release_package(PackageKind::Deb, env),
        InstallMethod::Rpm => plan_release_package(PackageKind::Rpm, env),
        InstallMethod::Aur { package } => plan_aur(package, env),
        InstallMethod::Cargo { root, source } => plan_cargo(root, source, env),
        InstallMethod::SourceBuild => Plan::Guidance(format!(
            "\
This aix was built from a source checkout, so there is nothing to download.
Update the checkout and rebuild:

    git pull
    cargo build --release

To switch to released builds instead, see {REPO_URL}#installation
"
        )),
        InstallMethod::Standalone { dir, writable } => {
            if *writable {
                Plan::Script { dir: dir.clone() }
            } else {
                let retry = if env.windows {
                    "run `aix upgrade` again from a terminal opened with \"Run as administrator\""
                } else {
                    "run `sudo aix upgrade`"
                };
                Plan::Guidance(format!(
                    "\
aix is a standalone binary in {}, which your user can't write to.
To upgrade it in place, {retry}.
",
                    dir.display()
                ))
            }
        }
    }
}

fn missing_tool(tool: &str, command: &str) -> Plan {
    Plan::Guidance(format!(
        "\
aix looks like it was installed with {tool}, but `{tool}` isn't on your PATH.
Once it is, upgrade with:

    {command}
"
    ))
}

fn plan_release_package(kind: PackageKind, env: &Env) -> Plan {
    if release_arch(kind, env.arch).is_none() {
        return Plan::Guidance(format!(
            "No release package is published for this architecture ({}).\n",
            env.arch
        ));
    }
    let candidates: &[(&str, &[&str])] = match kind {
        PackageKind::Deb => &[("apt", &["install", "-y"]), ("dpkg", &["-i"])],
        PackageKind::Rpm => &[
            ("dnf", &["install", "-y"]),
            (
                "zypper",
                &["--non-interactive", "install", "--allow-unsigned-rpm"],
            ),
            ("yum", &["install", "-y"]),
            ("rpm", &["-U"]),
        ],
    };
    let installer = candidates
        .iter()
        .find_map(|(name, args)| env.find(name).map(|path| Step::new(path, args)));
    match installer {
        Some(installer) => Plan::ReleasePackage {
            kind,
            installer: Step {
                elevate: !env.root,
                ..installer
            },
        },
        None => Plan::Guidance(format!(
            "Download the new package from {REPO_URL}/releases/latest and install it \
             with your package manager.\n"
        )),
    }
}

fn plan_aur(package: &str, env: &Env) -> Plan {
    let manual = format!(
        "\
    git clone https://aur.archlinux.org/{package}.git
    cd {package}
    makepkg -si
"
    );
    if env.root {
        return Plan::Guidance(format!(
            "AUR packages must not be built as root. As your normal user, run:\n\n{manual}"
        ));
    }
    match env.first(&["paru", "yay"]) {
        Some((_, helper)) => Plan::PackageManager {
            step: Step::new(helper, &["-S", "--needed", "--noconfirm", package]),
            note: None,
        },
        None => Plan::Guidance(format!(
            "aix comes from the AUR package {package}, and no AUR helper (paru or yay) \
             was found. Rebuild the package by hand:\n\n{manual}"
        )),
    }
}

fn plan_cargo(root: &Path, source: &CargoSource, env: &Env) -> Plan {
    let Some(cargo) = env.find("cargo") else {
        return missing_tool("cargo", &format!("cargo install --locked {CRATE_NAME}"));
    };
    let mut root_args: Vec<&str> = Vec::new();
    let root_text = root.to_string_lossy();
    if env.cargo_home.as_deref() != Some(root) {
        root_args.extend(["--root", &root_text]);
    }

    match source {
        CargoSource::Binstall if env.find("cargo-binstall").is_some() => {
            let mut args = vec!["binstall", "--no-confirm"];
            args.extend(&root_args);
            args.push(CRATE_NAME);
            Plan::PackageManager {
                step: Step::new(cargo, &args),
                note: None,
            }
        }
        CargoSource::Binstall | CargoSource::Registry => {
            let mut args = vec!["install", "--locked"];
            args.extend(&root_args);
            args.push(CRATE_NAME);
            let note = matches!(source, CargoSource::Binstall).then(|| {
                "cargo-binstall is no longer on your PATH; building from source with \
                 cargo install instead."
                    .to_string()
            });
            Plan::PackageManager {
                step: Step::new(cargo, &args),
                note,
            }
        }
        CargoSource::Other(source) => Plan::Guidance(format!(
            "\
aix was installed with cargo from {source}, not from crates.io.
Rebuild it from that source, or switch to the released crate:

    cargo install --locked {CRATE_NAME}
"
        )),
    }
}

/// Architecture name used in release package file names.
fn release_arch(kind: PackageKind, arch: &str) -> Option<&'static str> {
    match (kind, arch) {
        (PackageKind::Deb, "x86_64") => Some("amd64"),
        (PackageKind::Deb, "aarch64") => Some("arm64"),
        (PackageKind::Rpm, "x86_64") => Some("x86_64"),
        (PackageKind::Rpm, "aarch64") => Some("aarch64"),
        _ => None,
    }
}

fn package_asset(kind: PackageKind, version: Version, arch: &str) -> Option<String> {
    let arch = release_arch(kind, arch)?;
    Some(match kind {
        PackageKind::Deb => format!("aix_{version}_{arch}.deb"),
        PackageKind::Rpm => format!("aix-{version}-1.{arch}.rpm"),
    })
}

fn script_name() -> &'static str {
    if cfg!(windows) {
        "install.ps1"
    } else {
        "install.sh"
    }
}

async fn report(plan: &Plan, current: Version, timeout: Duration) -> Result<()> {
    let would = match plan {
        Plan::Guidance(text) => {
            println!();
            print!("{text}");
            return Ok(());
        }
        Plan::PackageManager { step, .. } => format!("run `{}`", step.display()),
        Plan::ReleasePackage { installer, .. } => format!(
            "download the new release package and install it with `{}`",
            installer.display()
        ),
        Plan::Script { dir } => format!(
            "re-run {} with AIX_INSTALL_DIR={}",
            script_name(),
            dir.display()
        ),
    };
    match Releases::new(timeout)?.latest_version().await {
        Ok(latest) if needs_upgrade(current, latest) => {
            println!("Latest release: {latest} (upgrade available)");
        }
        Ok(latest) => println!("Latest release: {latest} (you're up to date)"),
        Err(error) => println!("Latest release: unknown ({error})"),
    }
    println!("`aix upgrade` would {would}.");
    Ok(())
}

async fn execute(
    plan: Plan,
    exe: &Path,
    current: Version,
    force: bool,
    timeout: Duration,
) -> Result<()> {
    match plan {
        Plan::Guidance(text) => {
            println!();
            print!("{text}");
            Ok(())
        }
        Plan::PackageManager { step, note } => {
            if let Some(note) = note {
                println!("{note}");
            }
            if step.handoff {
                return hand_off(&step);
            }
            let aside = MovedAside::new(exe)?;
            run_step(&step)?;
            aside.commit();
            Ok(())
        }
        Plan::ReleasePackage { kind, installer } => {
            let releases = Releases::new(timeout)?;
            let Some(latest) = check_latest(&releases, current, force).await? else {
                return Ok(());
            };
            let asset = package_asset(kind, latest, std::env::consts::ARCH)
                .expect("plan only picks release packages for supported architectures");
            println!("Downloading {asset}...");
            let bytes = releases.download_verified(latest, &asset).await?;
            let dir = readable_temp_dir()?;
            let path = dir.path().join(&asset);
            std::fs::write(&path, bytes)
                .wrap_err_with(|| format!("could not write {}", path.display()))?;
            let mut step = installer;
            step.args.push(path.into_os_string());
            run_step(&step)?;
            print_done(current, latest);
            Ok(())
        }
        Plan::Script { dir } => {
            let releases = Releases::new(timeout)?;
            let Some(latest) = check_latest(&releases, current, force).await? else {
                return Ok(());
            };
            let url = format!("{REPO_URL}/releases/latest/download/{}", script_name());
            println!("Re-running the installer from {url}");
            let script = releases.download(&url).await?;
            let file = tempfile::Builder::new()
                .prefix("aix-install-")
                .suffix(if cfg!(windows) { ".ps1" } else { ".sh" })
                .tempfile()
                .wrap_err("could not create a temporary file for the installer")?;
            std::fs::write(file.path(), script).wrap_err("could not save the installer")?;

            let mut step = if cfg!(windows) {
                Step::new(
                    PathBuf::from("powershell"),
                    &["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"],
                )
            } else {
                Step::new(PathBuf::from("sh"), &[])
            };
            step.args.push(file.path().as_os_str().to_owned());

            let aside = MovedAside::new(exe)?;
            run_step_with(&step, |command| {
                command.env("AIX_INSTALL_DIR", &dir);
            })?;
            aside.commit();
            print_done(current, latest);
            Ok(())
        }
    }
}

/// `Some(latest)` when there is something to install.
async fn check_latest(
    releases: &Releases,
    current: Version,
    force: bool,
) -> Result<Option<Version>> {
    let latest = releases
        .latest_version()
        .await
        .wrap_err("could not look up the latest aix release")?;
    if force || needs_upgrade(current, latest) {
        if !needs_upgrade(current, latest) {
            println!("Reinstalling {latest} (--force).");
        } else {
            println!("Upgrading {current} → {latest}.");
        }
        return Ok(Some(latest));
    }
    if latest == current {
        println!("aix {current} is already the latest release.");
    } else {
        println!(
            "aix {current} is newer than the latest stable release ({latest}); nothing to do. \
             Pass --force to install {latest} anyway."
        );
    }
    Ok(None)
}

fn print_done(current: Version, installed: Version) {
    if current == installed {
        println!("Reinstalled aix {installed}.");
    } else {
        println!("Upgraded aix {current} → {installed}.");
    }
}

fn run_step(step: &Step) -> Result<()> {
    run_step_with(step, |_| {})
}

fn run_step_with(step: &Step, configure: impl FnOnce(&mut Command)) -> Result<()> {
    println!("Running: {}", step.display());
    let mut command = match elevation(step) {
        Some(sudo) => {
            let mut command = Command::new(sudo);
            command.arg(&step.program);
            command
        }
        None => Command::new(&step.program),
    };
    command.args(&step.args);
    configure(&mut command);
    let status = command
        .status()
        .wrap_err_with(|| format!("could not start `{}`", step.display()))?;
    if !status.success() {
        bail!("`{}` failed ({status})", step.display());
    }
    Ok(())
}

fn elevation(step: &Step) -> Option<PathBuf> {
    if !step.elevate {
        return None;
    }
    ["sudo", "doas"]
        .iter()
        .find_map(|name| which::which(name).ok())
}

/// Windows package managers can't replace a running `aix.exe`, so start the
/// upgrade in a new console that waits for this process to exit.
fn hand_off(step: &Step) -> Result<()> {
    let quote = |text: &str| format!("'{}'", text.replace('\'', "''"));
    let args: Vec<String> = step
        .args
        .iter()
        .map(|arg| quote(&arg.to_string_lossy()))
        .collect();
    let script = format!(
        "Wait-Process -Id {pid} -ErrorAction SilentlyContinue; & {program} {args}; \
         Write-Host ''; Read-Host 'Done. Press Enter to close'",
        pid = std::process::id(),
        program = quote(&step.program.to_string_lossy()),
        args = args.join(" "),
    );
    let mut command = Command::new("powershell");
    command.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]);
    new_console(&mut command);
    command
        .spawn()
        .wrap_err("could not start PowerShell for the upgrade")?;
    println!(
        "Windows can't replace aix.exe while it is running, so `{}` will run in a new \
         window as soon as this command exits.",
        step.display()
    );
    Ok(())
}

#[cfg(windows)]
fn new_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    command.creation_flags(CREATE_NEW_CONSOLE);
}

#[cfg(not(windows))]
fn new_console(_: &mut Command) {}

/// On Windows a running exe can be renamed but not overwritten. Moving it
/// aside lets the installer write the new `aix.exe`; the old copy is removed
/// the next time `aix upgrade` runs. A no-op elsewhere.
struct MovedAside {
    exe: PathBuf,
    aside: Option<PathBuf>,
}

impl MovedAside {
    fn new(exe: &Path) -> Result<Self> {
        let mut guard = Self {
            exe: exe.to_path_buf(),
            aside: None,
        };
        if cfg!(windows) {
            let aside = aside_path(exe);
            let _ = std::fs::remove_file(&aside);
            std::fs::rename(exe, &aside)
                .wrap_err_with(|| format!("could not move {} aside", exe.display()))?;
            guard.aside = Some(aside);
        }
        Ok(guard)
    }

    fn commit(mut self) {
        self.aside = None;
    }
}

impl Drop for MovedAside {
    /// The upgrade failed: put the old binary back if nothing replaced it.
    fn drop(&mut self) {
        if let Some(aside) = &self.aside {
            if !self.exe.exists() {
                let _ = std::fs::rename(aside, &self.exe);
            }
        }
    }
}

fn aside_path(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    exe.with_file_name(name)
}

fn remove_leftover_binary(exe: &Path) {
    if cfg!(windows) {
        let _ = std::fs::remove_file(aside_path(exe));
    }
}

fn current_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().wrap_err("could not locate the running aix binary")?;
    // Resolve symlinks (Homebrew's bin/aix, ~/.nix-profile/bin/aix, …) on Unix.
    // On Windows, canonicalize adds a `\\?\` prefix and winget links are
    // matched without resolving, so keep the path as is.
    if cfg!(windows) {
        Ok(exe)
    } else {
        Ok(std::fs::canonicalize(&exe).unwrap_or(exe))
    }
}

/// apt drops privileges to `_apt` to read local packages; a world-readable
/// temp dir avoids its "unsandboxed download" warning.
fn readable_temp_dir() -> Result<tempfile::TempDir> {
    let dir = tempfile::Builder::new()
        .prefix("aix-upgrade-")
        .tempdir()
        .wrap_err("could not create a temporary directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755))
            .wrap_err("could not set temporary directory permissions")?;
    }
    Ok(dir)
}

#[cfg(unix)]
fn is_root() -> bool {
    ::nix::unistd::geteuid().is_root()
}

#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with(programs: &'static [&'static str]) -> Env {
        Env {
            windows: false,
            root: false,
            arch: "x86_64",
            find: Box::new(move |name| {
                programs
                    .contains(&name)
                    .then(|| PathBuf::from("/bin").join(name))
            }),
            cargo_home: Some(PathBuf::from("/home/me/.cargo")),
        }
    }

    fn command_line(plan: Plan) -> String {
        match plan {
            Plan::PackageManager { step, .. } => step.display(),
            Plan::ReleasePackage { installer, .. } => installer.display(),
            other => panic!("expected a command, got {other:?}"),
        }
    }

    #[test]
    fn package_managers_upgrade_themselves() {
        let env = env_with(&["brew", "scoop", "winget"]);
        assert_eq!(
            command_line(plan(&InstallMethod::Homebrew, &env)),
            "brew upgrade get-aix/tap/aix"
        );
        assert_eq!(
            command_line(plan(&InstallMethod::Scoop, &env)),
            "scoop update aix"
        );
        assert_eq!(
            command_line(plan(&InstallMethod::Winget, &env)),
            "winget upgrade --id get-aix.aix --exact"
        );
    }

    #[test]
    fn windows_package_managers_hand_off() {
        let env = Env {
            windows: true,
            ..env_with(&["scoop"])
        };
        let Plan::PackageManager { step, .. } = plan(&InstallMethod::Scoop, &env) else {
            panic!("expected a package manager plan");
        };
        assert!(step.handoff);
    }

    #[test]
    fn missing_package_manager_prints_the_command() {
        let Plan::Guidance(text) = plan(&InstallMethod::Homebrew, &env_with(&[])) else {
            panic!("expected guidance");
        };
        assert!(text.contains("brew upgrade get-aix/tap/aix"));
    }

    #[test]
    fn release_packages_prefer_the_high_level_tool_and_elevate() {
        let env = env_with(&["apt", "dpkg"]);
        assert_eq!(
            command_line(plan(&InstallMethod::Deb, &env)),
            "sudo apt install -y"
        );
        let env = env_with(&["rpm", "zypper"]);
        assert_eq!(
            command_line(plan(&InstallMethod::Rpm, &env)),
            "sudo zypper --non-interactive install --allow-unsigned-rpm"
        );
        let env = Env {
            root: true,
            ..env_with(&["dnf"])
        };
        assert_eq!(
            command_line(plan(&InstallMethod::Rpm, &env)),
            "dnf install -y"
        );
    }

    #[test]
    fn release_package_names_match_the_published_assets() {
        let v = Version::parse("1.2.3").unwrap();
        assert_eq!(
            package_asset(PackageKind::Deb, v, "x86_64").as_deref(),
            Some("aix_1.2.3_amd64.deb")
        );
        assert_eq!(
            package_asset(PackageKind::Deb, v, "aarch64").as_deref(),
            Some("aix_1.2.3_arm64.deb")
        );
        assert_eq!(
            package_asset(PackageKind::Rpm, v, "aarch64").as_deref(),
            Some("aix-1.2.3-1.aarch64.rpm")
        );
        assert_eq!(package_asset(PackageKind::Rpm, v, "riscv64"), None);
    }

    #[test]
    fn aur_uses_a_helper_and_never_builds_as_root() {
        let aur = InstallMethod::Aur {
            package: "aix-rs-bin".into(),
        };
        assert_eq!(
            command_line(plan(&aur, &env_with(&["yay", "paru"]))),
            "paru -S --needed --noconfirm aix-rs-bin"
        );
        assert!(
            matches!(plan(&aur, &env_with(&[])), Plan::Guidance(t) if t.contains("makepkg -si"))
        );
        let root = Env {
            root: true,
            ..env_with(&["paru"])
        };
        assert!(matches!(plan(&aur, &root), Plan::Guidance(t) if t.contains("as root")));
    }

    #[test]
    fn cargo_installs_reuse_their_tool_and_root() {
        let home = PathBuf::from("/home/me/.cargo");
        let binstall = InstallMethod::Cargo {
            root: home.clone(),
            source: CargoSource::Binstall,
        };
        assert_eq!(
            command_line(plan(&binstall, &env_with(&["cargo", "cargo-binstall"]))),
            "cargo binstall --no-confirm aix-rs"
        );
        // binstall gone: fall back to building from crates.io.
        assert_eq!(
            command_line(plan(&binstall, &env_with(&["cargo"]))),
            "cargo install --locked aix-rs"
        );
        let custom_root = InstallMethod::Cargo {
            root: PathBuf::from("/opt/tools"),
            source: CargoSource::Registry,
        };
        assert_eq!(
            command_line(plan(&custom_root, &env_with(&["cargo"]))),
            "cargo install --locked --root /opt/tools aix-rs"
        );
        let local = InstallMethod::Cargo {
            root: home,
            source: CargoSource::Other("path+file:///src/aix".into()),
        };
        assert!(
            matches!(plan(&local, &env_with(&["cargo"])), Plan::Guidance(t) if t.contains("path+file:///src/aix"))
        );
    }

    #[test]
    fn standalone_binaries_rerun_the_script_when_writable() {
        let dir = PathBuf::from("/home/me/.local/bin");
        assert_eq!(
            plan(
                &InstallMethod::Standalone {
                    dir: dir.clone(),
                    writable: true
                },
                &env_with(&[])
            ),
            Plan::Script { dir: dir.clone() }
        );
        assert!(matches!(
            plan(&InstallMethod::Standalone { dir, writable: false }, &env_with(&[])),
            Plan::Guidance(t) if t.contains("sudo aix upgrade")
        ));
    }

    #[test]
    fn nix_and_source_builds_only_get_guidance() {
        let nix = InstallMethod::Nix {
            store_path: PathBuf::from("/nix/store/abc-aix-rs-1.0.0"),
        };
        assert!(matches!(
            plan(&nix, &env_with(&["brew"])),
            Plan::Guidance(_)
        ));
        assert!(matches!(
            plan(&InstallMethod::SourceBuild, &env_with(&["cargo"])),
            Plan::Guidance(t) if t.contains("cargo build --release")
        ));
    }

    #[test]
    fn moved_aside_name_keeps_the_original_name() {
        assert_eq!(
            aside_path(Path::new("/x/aix.exe")),
            PathBuf::from("/x/aix.exe.old")
        );
    }
}
