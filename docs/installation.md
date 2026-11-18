# Installation

aix is a single binary named `aix`. Pick one of the methods below; after
installing, continue with [Configuration](configuration.md).

- [Installer scripts](#installer-scripts) — prebuilt binary, recommended for most users
- [Cargo](#cargo) — build from crates.io
- [Manual download](#manual-download) — place the binary yourself
- [Nix](#nix) — flake, overlay, or home-manager module
- [Nightly builds](#nightly-builds) — prebuilt binaries from `main`

## Releases

Releases are versioned with [semver](https://semver.org/) and tagged `vX.Y.Z`,
starting at `v1.0.0`. They are automated with
[release-plz](https://release-plz.dev/); see the [changelog](../CHANGELOG.md)
for what changed in each version. Every [GitHub Release](https://github.com/get-aix/aix/releases)
includes prebuilt binaries for:

| Platform | Asset name |
|----------|-----------|
| Linux x86_64 | `aix-x86_64-unknown-linux-gnu` |
| Linux aarch64 | `aix-aarch64-unknown-linux-gnu` |
| macOS arm64 | `aix-aarch64-apple-darwin` |
| macOS x86_64 | `aix-x86_64-apple-darwin` |
| Windows x86_64 | `aix-x86_64-pc-windows-msvc.exe` |

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

If you use [cargo-binstall](https://github.com/cargo-bins/cargo-binstall), you
can install a prebuilt binary instead of compiling:

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

## Nightly builds

A rolling [`nightly` prerelease](https://github.com/get-aix/aix/releases/tag/nightly)
is rebuilt from `main` with binaries for the same platforms. Use it if you want
unreleased changes; download the asset for your platform from that release page.
Stable releases are recommended otherwise.
