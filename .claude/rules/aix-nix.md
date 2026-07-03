---
paths:
  - "modules/home/aix.nix"
  - "modules/home/pi-coding-agent/**"
  - "configurations/nixos/wslstation/**"
  - "configurations/home/wsl.nix"
---

# aix Nix integration — conventions

## Role of Nix in this project

Nix is responsible for:
- Installing the compiled Rust CLI into the user profile.
- Generating `~/.config/aix/config.toml` from home-manager options.
- Injecting secrets via `agenix` or `sops` — the CLI itself is secret-store-agnostic.
- Wiring systemd user services or shell aliases if needed.

Nix is **not** responsible for CLI business logic. If you find logic in a Nix file that belongs in Rust, flag it rather than expanding it.

## Module structure

`modules/home/aix.nix` exposes home-manager options:
- `programs.aix.enable`
- `programs.aix.profiles` — attrset of profile definitions → written to config.toml
- `programs.aix.secretSource` — enum: `env | agenix | sops | cmd`
- `programs.aix.package` — the Rust derivation (override point)

Do not add options that duplicate CLI flags. Options are for stable deployment config, not ad-hoc overrides.

## Config generation

The module renders `config.toml` from the `profiles` attrset using `builtins.toJSON` or `lib.generators.toTOML`.
The output path must be `$XDG_CONFIG_HOME/aix/config.toml` (default `~/.config/aix/config.toml`).

## Secret handling

Secrets must never appear in the Nix store (world-readable).
Use `agenix` or `sops-nix` to decrypt at activation time into a path outside the store.
Pass the path to the CLI via `AIX_API_KEY` or `--secret-cmd cat /run/secrets/aix_key`.

## WSL considerations

- The WSL host config lives in `configurations/nixos/wslstation/`.
- Do not add WSL-specific hacks to generic home-manager modules — gate them with `lib.optionalAttrs pkgs.stdenv.isLinux`.
- Preserve `wsl.nix` imports; do not remove or rename existing module references without checking dependents.

## Formatting

Run `alejandra` (preferred) or `nixfmt` on any Nix file before committing:
```sh
alejandra modules/home/aix.nix
```

## What NOT to change here

- Do not modify `configurations/nixos/wslstation/hardware-configuration.nix`.
- Do not touch other `modules/home/` files (vim, git, etc.) unless the task explicitly targets them.
- Do not add new `imports` to `configurations/home/wsl.nix` without user approval.
