<p align="center"><img src="assets/logo.png" width="160" alt="aix"></p>

<p align="center">One config for your AI gateway profiles. Launch any AI tool with the right credentials.</p>

<p align="center">
  <a href="https://github.com/get-aix/aix/actions/workflows/ci.yml"><img src="https://github.com/get-aix/aix/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://crates.io/crates/aix-rs"><img src="https://img.shields.io/crates/v/aix-rs" alt="crates.io"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-AGPL--3.0--only-blue" alt="License: AGPL-3.0-only"></a>
</p>

> [!NOTE]
> **Not IBM AIX.** This project has no relation to IBM's AIX operating system.

**aix** (AI eXecute) is a profile-aware CLI for AI tools and LiteLLM-compatible
AI gateways. You describe your profiles once (which key, which gateway, where
each secret comes from) and aix launches Claude Code, Codex, OpenCode, Pi, or
any other tool with the matching environment variables.

## Why

If you use more than one AI tool, or more than one key, the setup drifts:
Claude Code reads `ANTHROPIC_*`, most other tools read `OPENAI_*`, some
LiteLLM-aware tools read `LITELLM_*`, and the key itself lives in a password
manager, a secrets file, or an environment variable that differs between work
and personal machines. aix replaces that with named profiles:

```
config file → profile selection → secret resolution → env vars → your tool
```

## 30-second example

```toml
# ~/.config/aix/aix.toml
default_profile = "work"

[endpoint]
base_url = { env = "AIX_BASE_URL" }

[profiles.work]
label   = "Work"
api_key = { command = "pass show aix/work-key" }

[profiles.personal]
label   = "Personal"
api_key = { file = "/run/secrets/aix/personal" }
```

```sh
aix claude              # Claude Code with ANTHROPIC_API_KEY/ANTHROPIC_BASE_URL for "work"
aix codex personal      # Codex with OPENAI_API_KEY/OPENAI_BASE_URL for "personal"
aix opencode work --dry-run   # show what would run; never prints secrets

aix status work         # gateway reachability, auth, and current-key budget
aix models work --filter gpt
```

## Features

- **Launch any tool with a profile.** `aix <tool> [PROFILE]` works with any
  binary on `$PATH`. Optional `[tools.<name>]` entries choose the executable,
  credential format (`anthropic`, `openai`, or `both`), and extra variables.
- **Backend-agnostic secrets.** Keys and gateway URLs can be a literal, an
  environment variable, a file, or the output of a command (`pass`,
  1Password CLI, Bitwarden CLI, ...).
- **Shell integration.** `aix use work` switches the active profile in bash,
  zsh, fish, Nushell, and PowerShell. `aix env`, `aix exec`, and `aix shell`
  cover the cases where you want the variables directly.
- **Gateway utilities.** List models, show current key spend and budget, check
  gateway status, and run read-only diagnostics with `aix doctor`.
- **One-shot inference.** Pipe text into `aix ask`, or define reusable
  `[prompts.<name>]` presets for `aix prompt`.
- **Sign in with ChatGPT.** Profiles can use ChatGPT plan credentials for Codex
  app-server and OpenCode v2 through a local bridge, so the tool never sees the
  real token.
- **Per-profile tool configs.** Select a separate Codex home, Pi agent
  directory, or OpenCode config per profile.
- **Local, metadata-only run history.** `aix run` gives a command a run ID and
  records its lifecycle without storing arguments, prompts, output, or secrets.
- **Scriptable.** A stable `--json` envelope and documented exit codes.
- **Declarative on Nix.** A home-manager module generates the config. The CLI
  itself has no Nix dependency and runs on Linux, macOS, and Windows.

API-key values are not logged or written to disk by aix. OAuth credentials are
the exception: they are retained in aix's owner-only local auth store. See
[Security notes](docs/security.md).

## Installation

| Platform | Command |
|----------|---------|
| macOS / Linux ([Homebrew](https://brew.sh)) | `brew install get-aix/tap/aix` |
| Windows ([winget](https://learn.microsoft.com/windows/package-manager/)) | `winget install get-aix.aix` |
| Windows ([Scoop](https://scoop.sh)) | `scoop bucket add get-aix https://github.com/get-aix/scoop-bucket`<br>`scoop install get-aix/aix` |
| Arch Linux ([AUR](https://aur.archlinux.org/packages/aix-rs-bin)) | `yay -S aix-rs-bin` |
| Any, via Rust ([crates.io](https://crates.io/crates/aix-rs)) | `cargo binstall aix-rs` or `cargo install aix-rs` |
| Nix | `nix run github:get-aix/aix -- --help`, or the [home-manager module](docs/nix.md) |

Or use the installer script, which needs no package manager:

```sh
# Linux / macOS
curl -fsSL https://github.com/get-aix/aix/releases/latest/download/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://github.com/get-aix/aix/releases/latest/download/install.ps1 | iex
```

The crate is called `aix-rs`, but every method installs a binary named `aix`.
Prebuilt binaries cover Linux x86_64/aarch64 (static), macOS arm64/x86_64, and
Windows x86_64. See [Installation](docs/installation.md) for checksums, manual
downloads, custom install locations, and [nightly builds](https://github.com/get-aix/aix/releases/tag/nightly).
Release notes are in the [changelog](CHANGELOG.md).

## Quick start

1. Create `aix.toml` in the platform config directory: `~/.config/aix/` on
   Linux, `~/Library/Application Support/aix/` on macOS, `%APPDATA%\aix\` on
   Windows (see [Config path resolution](docs/configuration.md#config-path-resolution)).
   Start from the [example above](#30-second-example) or the
   [full example](docs/configuration.md#full-example-all-secret-types-all-options).
2. Check it:

   ```sh
   aix config path       # the file aix found
   aix config validate
   aix doctor
   ```

3. Enable profile switching in your shell (zsh shown; see
   [Shell integration](docs/shell-integration.md) for other shells):

   ```sh
   eval "$(command aix init zsh)"
   aix use work
   aix current
   ```

4. Launch a tool:

   ```sh
   aix claude
   aix codex -- --help
   ```

## Documentation

| | |
|---|---|
| [Installation](docs/installation.md) | Installer scripts, Cargo, prebuilt binaries, Nix, nightly builds |
| [Configuration](docs/configuration.md) | Config location and format, secret sources, models and aliases |
| [Running AI tools](docs/tools.md) | `aix <tool>`, `[tools.<name>]`, emitted variables, Codex/Pi/OpenCode specifics |
| [Shell integration](docs/shell-integration.md) | `aix use`, `aix env`, `aix exec`, `aix shell` |
| [Gateway commands](docs/gateway.md) | `models`, `spend`, `status`, `doctor`, `usage`, cache |
| [One-shot inference](docs/inference.md) | `aix ask` and `aix prompt` presets |
| [Managed runs](docs/runs.md) | `aix run`, run history, run policies |
| [ChatGPT authentication](docs/chatgpt.md) | Sign in with ChatGPT profiles |
| [Nix and home-manager](docs/nix.md) | Declarative configuration |
| [Command reference](docs/commands.md) | All commands and global options |
| [JSON output and exit codes](docs/json-output.md) | Scripting contract |
| [Security notes](docs/security.md) | How secrets and credentials are handled |

## License

[AGPL-3.0-only](LICENSE)
