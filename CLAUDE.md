# aix — Claude Code project context

## Repo overview

This is a Nix configuration repository (NixOS + home-manager) for a WSL workstation.
The `aix` tool is being extracted from inline Nix shell scripts into a standalone **Rust CLI**.

Current layout (evolving):
```
docs/aix/          — design docs for the Rust extraction
modules/home/        — home-manager modules (Nix)
configurations/      — NixOS host configurations
tools/aix/         — future home of the Rust CLI source
```

## Ground rules

- **Do not touch unrelated Nix modules.** Changes outside `modules/home/aix.nix`, `modules/home/pi-coding-agent/`, or the Rust paths require explicit user approval.
- Prefer small commits and small diffs. One logical change per commit.
- Run `cargo fmt && cargo clippy` before claiming Rust work is done.
- Run `nixfmt` or `alejandra` on any Nix files you edit.
- No hardcoded secrets, API keys, or URLs tied to any specific deployment.

## Architecture constraints

1. **Rust CLI must not depend on Nix paths directly.** The CLI must work on any OS; Nix is the deployment mechanism, not a runtime dependency.
2. **Nix generates config; CLI reads it.** Business logic lives in Rust. Nix produces config files consumed by the CLI.
3. **Secret resolution is backend-agnostic.** The CLI accepts secrets via env vars, a config file, or a secret-store flag. It must not assume `pass`, `agenix`, or any specific Nix secret tool.
4. **LiteLLM gateway compatibility is endpoint + profile config.** No provider-specific logic baked into command routing — use named profiles resolved at runtime.

## WSL / Nix workflow

The primary dev environment is WSL2 + NixOS. Cross-platform support (macOS, plain Linux) is a goal of the Rust extraction.
When adding dependencies, prefer crates that compile without OS-specific build tooling.

## Scoped rules

Additional path-scoped guidance lives in `.claude/rules/`:
- `aix-rust.md` — Rust CLI conventions (active for `tools/aix/**`, `crates/aix/**`, `Cargo.*`)
- `aix-nix.md` — Nix integration conventions (active for Nix module paths)
