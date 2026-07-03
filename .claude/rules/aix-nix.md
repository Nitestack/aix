---
paths:
  - "modules/home/aix.nix"
  - "nix/**"
  - "flake.nix"
  - "flake.lock"
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

- Do not add WSL-specific hacks to generic home-manager modules — gate them with `lib.optionalAttrs pkgs.stdenv.isLinux`.

## Formatting

Run `alejandra` (preferred) or `nixfmt` on any Nix file before committing:
```sh
alejandra modules/home/aix.nix
```

## What NOT to change here

- Do not touch other `modules/home/` files unrelated to `aix` unless the task explicitly targets them.
- Do not modify `nix/` example configs by hand — they are generated from deployment configs.
- Do not add new flake inputs to `flake.nix` without user approval.
