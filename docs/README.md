# aix documentation

Start with [Installation](installation.md), then [Configuration](configuration.md).

## Setup

- [Installation](installation.md) — Homebrew, winget, Scoop, .deb/.rpm, AUR, installer scripts, Cargo, prebuilt binaries, Nix, nightly builds, and upgrading with `aix upgrade`
- [Configuration](configuration.md) — config file location and format, secret sources, model defaults and aliases
- [Nix and home-manager](nix.md) — declarative config with the home-manager module
- [ChatGPT authentication](chatgpt.md) — sign in with ChatGPT profiles

## Using aix

- [Running AI tools](tools.md) — `aix claude`, `aix codex`, `aix opencode`, `aix pi`, and any other tool; `[tools.<name>]` wiring; emitted environment variables
- [Shell integration](shell-integration.md) — `aix use`, `aix env`, `aix exec`, `aix shell`
- [Gateway commands](gateway.md) — `aix models`, `aix spend`, `aix status`, `aix doctor`, `aix usage`, cache
- [One-shot inference](inference.md) — `aix ask` and `aix prompt` presets
- [Managed runs](runs.md) — `aix run`, run history, run policies, `aix gate`

## Reference

- [Command reference](commands.md) — every command and the global options
- [JSON output and exit codes](json-output.md)
- [Security notes](security.md)

## Working on aix

- [CONTRIBUTING.md](../CONTRIBUTING.md) — development setup, PR checklist, manual verification
- [ARCHITECTURE.md](../ARCHITECTURE.md) — module boundaries and invariants
- [Releasing](releasing.md) — release flow, package-manager publishing, required secrets
- [`agents/`](agents) — guidance for coding agents working in this repo
- [`tests/manual/`](../tests/manual) — scripted checks that need real tools or accounts
