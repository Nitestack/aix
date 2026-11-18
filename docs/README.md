# aix documentation

Start with [Installation](installation.md), then [Configuration](configuration.md).

## Setup

- [Installation](installation.md) — Homebrew, winget, Scoop, .deb/.rpm, AUR, installer scripts, Cargo, prebuilt binaries, Nix, nightly builds
- [Configuration](configuration.md) — config file location and format, secret sources, model defaults and aliases
- [Nix and home-manager](nix.md) — declarative config with the home-manager module
- [ChatGPT authentication](chatgpt.md) — Sign in with ChatGPT profiles

## Using aix

- [Running AI tools](tools.md) — `aix claude`, `aix codex`, `aix opencode`, `aix pi`, and any other tool; `[tools.<name>]` wiring; emitted environment variables
- [Shell integration](shell-integration.md) — `aix use`, `aix env`, `aix exec`, `aix shell`
- [Gateway commands](gateway.md) — `aix models`, `aix spend`, `aix status`, `aix doctor`, `aix usage`, cache
- [One-shot inference](inference.md) — `aix ask` and `aix prompt` presets
- [Managed runs](runs.md) — `aix run`, run history, run policies

## Reference

- [Command reference](commands.md) — every command and the global options
- [JSON output and exit codes](json-output.md)
- [Security notes](security.md)

## Design and maintainer docs

- [Releasing](releasing.md) — release flow, package-manager publishing, required secrets
- [Architecture](aix/architecture.md)
- [Current behavior](aix/current-behavior.md)
- [Migration notes](aix/migration.md)
- [ChatGPT Responses local gateway](aix/chatgpt-local-gateway.md)
