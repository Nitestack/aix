# Installation

aix is a single binary named `aix`. Pick one of the methods below; after
installing, continue with [Configuration](configuration.md).

- [Package managers](#package-managers): Homebrew, winget, Scoop, .deb/.rpm, AUR
- [Installer scripts](#installer-scripts): prebuilt binary, no package manager needed
- [Cargo](#cargo): prebuilt via cargo-binstall, or build from crates.io
- [Manual download](#manual-download) — place the binary yourself
- [Nix](#nix) — flake, overlay, or home-manager module
- [Nightly builds](#nightly-builds) — prebuilt binaries from `main`

To upgrade later, run `aix upgrade`. It works out how aix was installed and
upgrades it the same way; see [Upgrading](#upgrading).

## Releases

Releases are versioned with [semver](https://semver.org/) and tagged `vX.Y.Z`,
starting at `v1.0.0`. They are automated with
[release-plz](https://release-plz.dev/); see the [changelog](../CHANGELOG.md)
for what changed in each version. Every [GitHub Release](https://github.com/get-aix/aix/releases)
includes prebuilt binaries for:

| Platform | Asset name |
|----------|-----------|
| Linux x86_64 (static, any distro) | `aix-x86_64-unknown-linux-musl` |
| Linux aarch64 (static, any distro) | `aix-aarch64-unknown-linux-musl` |
| Linux x86_64 (glibc) | `aix-x86_64-unknown-linux-gnu` |
| Linux aarch64 (glibc) | `aix-aarch64-unknown-linux-gnu` |
| macOS arm64 | `aix-aarch64-apple-darwin` |
| macOS x86_64 | `aix-x86_64-apple-darwin` |
| Windows x86_64 | `aix-x86_64-pc-windows-msvc.exe` |
| Debian/Ubuntu package | `aix_<version>_amd64.deb`, `aix_<version>_arm64.deb` |
| Fedora/RHEL/openSUSE package | `aix-<version>-1.x86_64.rpm`, `aix-<version>-1.aarch64.rpm` |

The static `musl` builds are what the installer scripts and package managers
use. The `gnu` builds are kept for existing download links. Each release also
includes a `SHA256SUMS` file. The installer scripts verify against it
automatically. To check a manual download yourself:

```sh
sha256sum --check --ignore-missing SHA256SUMS
```

## Package managers

The packages below are updated automatically when a release is published.

### Homebrew (macOS, Linux)

```sh
brew tap get-aix/tap
brew install aix
```

`aix upgrade` (or `brew upgrade`) keeps it up to date. The one-liner `brew install get-aix/tap/aix`
does the same thing. The formula lives in
[get-aix/homebrew-tap](https://github.com/get-aix/homebrew-tap).

### winget (Windows)

```powershell
winget install get-aix.aix
```

### Scoop (Windows)

```powershell
scoop bucket add get-aix https://github.com/get-aix/scoop-bucket
scoop install get-aix/aix
```

### Debian, Ubuntu, Fedora, RHEL, openSUSE (.deb / .rpm)

Each release includes `.deb` and `.rpm` packages for x86_64 and aarch64. They
install the static binary to `/usr/bin/aix`. Download the package for your
system from the [latest release](https://github.com/get-aix/aix/releases/latest),
then install it:

```sh
# Debian, Ubuntu, Mint, …
sudo apt install ./aix_<version>_amd64.deb      # arm64: aix_<version>_arm64.deb

# Fedora, RHEL, Rocky, Alma, …
sudo dnf install ./aix-<version>-1.x86_64.rpm   # arm64: aix-<version>-1.aarch64.rpm

# openSUSE
sudo zypper install ./aix-<version>-1.x86_64.rpm
```

With the GitHub CLI, you can download the latest `.deb` in one step:
`gh release download --repo get-aix/aix --pattern '*_amd64.deb'`.

These packages don't come from an apt or dnf repository, so `apt upgrade` and
`dnf upgrade` won't update aix. Run `aix upgrade` instead: it downloads the new
release's package, verifies its checksum, and installs it with apt, dnf, zypper,
or rpm. You can also install the new package by hand, the same way as above.

### AUR (Arch Linux)

The package is [`aix-rs-bin`](https://aur.archlinux.org/packages/aix-rs-bin)
(prebuilt static binary). Install it with any AUR helper:

```sh
yay -S aix-rs-bin    # or: paru -S aix-rs-bin
```

It conflicts with the unrelated AUR packages `aix` and `aix-bin`, which also
install a `/usr/bin/aix`.

## Installer scripts

The quickest way to install. The script detects your platform, downloads the
right binary from the latest stable release, and adds the install directory to
your `PATH`.

**Linux / macOS:**

```sh
curl -fsSL https://github.com/get-aix/aix/releases/latest/download/install.sh | sh
```

Installs to `~/.local/bin` by default. Adds the directory to your shell rc file (`~/.zshrc`, `~/.bashrc`, `~/.config/fish/config.fish`, or `~/.profile`). Restart your terminal or source the rc file after installing.

**Windows (PowerShell):**

```powershell
irm https://github.com/get-aix/aix/releases/latest/download/install.ps1 | iex
```

Installs to `%LOCALAPPDATA%\Programs\aix` by default. Adds the directory to the user `Path` environment variable. Restart your terminal after installing.

**Custom install location:**

Set `AIX_INSTALL_DIR` before running either script to override the default destination:

```sh
AIX_INSTALL_DIR=/usr/local/bin curl -fsSL https://github.com/get-aix/aix/releases/latest/download/install.sh | sh
```

```powershell
$env:AIX_INSTALL_DIR = 'C:\Tools\aix'; irm https://github.com/get-aix/aix/releases/latest/download/install.ps1 | iex
```

## Cargo

The crate is published on crates.io as [`aix-rs`](https://crates.io/crates/aix-rs);
the installed binary is still called `aix`:

```bash
cargo install aix-rs
```

With [cargo-binstall](https://github.com/cargo-bins/cargo-binstall), you get the
prebuilt release binary instead of compiling:

```bash
cargo binstall aix-rs
```

To build from a checkout of this repository:

```bash
cargo install --path .
```

Or build without installing:

```bash
cargo build --release
# binary is at target/release/aix
```

## Manual download

To place the binary yourself, download the asset for your platform (see the
[table above](#releases)) from the [latest release](https://github.com/get-aix/aix/releases/latest),
make it executable, and put it on your `PATH` as `aix` (`aix.exe` on Windows).

## Nix

Three paths depending on how much you want Nix to manage.

### home-manager module (recommended for NixOS / nix-darwin)

Add the flake input, import the module, and declare your config declaratively.
Nix installs the binary and generates `~/.config/aix/aix.toml` at activation
time. See [Nix and home-manager](nix.md) for the full setup and option mapping.

### Overlay (install binary only, manage config yourself)

Apply `overlays.default` to add `pkgs.aix-rs` to your package set, then add it to `home.packages` or `environment.systemPackages`. The overlay only supports the four default systems (`x86_64-linux`, `aarch64-linux`, `x86_64-darwin`, `aarch64-darwin`).

```nix
nixpkgs.overlays = [ inputs.aix.overlays.default ];
home.packages = [ pkgs.aix-rs ];
```

### One-off / try it out

```bash
nix run github:get-aix/aix -- --help
```

## Upgrading

```sh
aix upgrade           # upgrade to the latest release
aix upgrade --check   # show what it would do, change nothing
```

`aix upgrade` looks at where the running binary lives to work out how it was
installed, then upgrades it the same way:

| Installed with | What `aix upgrade` runs |
|---|---|
| Homebrew | `brew upgrade get-aix/tap/aix` |
| Scoop | `scoop update aix` |
| winget | `winget upgrade --id get-aix.aix --exact` |
| `.deb` / `.rpm` | Downloads the new release package, verifies it against `SHA256SUMS`, and installs it with `apt`, `dpkg`, `dnf`, `zypper`, `yum`, or `rpm` (using `sudo` when needed) |
| AUR | `paru -S --needed aix-rs-bin`, or `yay` if paru isn't installed |
| `cargo binstall` | `cargo binstall aix-rs` |
| `cargo install` | `cargo install --locked aix-rs` |
| Installer script or manual download | Re-runs the installer script into the same directory |
| Nix | Nothing; prints how to upgrade through Nix (see below) |
| Source checkout (`target/…`) | Nothing; prints how to pull and rebuild |

Package managers decide for themselves whether a newer version exists. For
installer-script and `.deb`/`.rpm` installs, aix compares its version with the
latest release first and stops if you're already up to date; pass `--force` to
reinstall anyway. `aix update` is accepted as an alias.

The package managers run with their own confirmation prompts turned off,
because running `aix upgrade` already says you want the upgrade. `sudo` still
asks for your password when it needs it.

A few cases need you to step in, and `aix upgrade` prints what to run:

- **Nix.** aix never modifies a Nix-managed binary, because Nix pins the version
  and would undo the change at the next rebuild. Update the flake input
  (`nix flake update aix`) and rebuild, or run `nix profile upgrade` for
  `nix profile` installs. See [Nix and home-manager](nix.md#upgrading).
- **A directory you can't write to**, such as `/usr/local/bin` after a manual
  download: run `sudo aix upgrade` (on Windows, a terminal opened with "Run as
  administrator").
- **AUR without an AUR helper:** rebuild the package with `makepkg -si`.
- **`cargo install` from a local path or git URL:** rebuild from that source, or
  switch to the released crate with `cargo install --locked aix-rs`.

On Windows, a running `aix.exe` can't be overwritten. For installer-script and
Cargo installs, aix renames itself to `aix.exe.old` before upgrading and deletes
that file the next time `aix upgrade` runs. Scoop and winget upgrades run in a
new window that starts as soon as `aix upgrade` exits.

`aix upgrade` exits 0 when it upgraded, found nothing newer, or only printed
instructions. It exits non-zero only when an upgrade was attempted and failed.

## Nightly builds

A rolling [`nightly` prerelease](https://github.com/get-aix/aix/releases/tag/nightly)
is rebuilt from `main` with binaries for the same platforms. Use it if you want
unreleased changes; download the asset for your platform from that release page.
Stable releases are recommended otherwise.
