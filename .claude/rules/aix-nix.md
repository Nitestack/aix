---
paths:
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

`nix/home-manager.nix` exposes home-manager options:
- `programs.aix.enable`
- `programs.aix.package` — the Rust derivation (override point)
- `programs.aix.defaultProfile` — optional default profile name
- `programs.aix.endpoint` — submodule: `baseUrl`, `apiFormat`, `gateway`, `provider`
- `programs.aix.profiles` — attrset of profile submodules (`label`, `apiKey`)

Each secret field (`baseUrl`, `apiKey`) accepts a `secretSourceType`: a plain string (direct), `{ env = "VAR"; }`, `{ file = "/run/secrets/..."; }`, or `{ command = "..."; }`.

Do not add options that duplicate CLI flags. Options are for stable deployment config, not ad-hoc overrides.

## Config generation

The module renders `~/.config/aix/aix.toml` via `xdg.configFile` using `pkgs.formats.toml.generate`.
camelCase Nix option names are transformed to snake_case TOML keys (e.g. `endpoint.baseUrl` → `endpoint.base_url`).
Secret source values (`{ env = "VAR"; }`, `{ file = "/path"; }`, `{ command = "..."; }`) pass through as TOML subtables.

## Secret handling

Secrets must never appear in the Nix store (world-readable).
Use `agenix` or `sops-nix` to decrypt at activation time into a path outside the store.
Pass the path to the CLI via `AIX_API_KEY` or `--secret-cmd cat /run/secrets/aix_key`.

## WSL considerations

- Do not add WSL-specific hacks to generic home-manager modules — gate them with `lib.optionalAttrs pkgs.stdenv.isLinux`.

## Formatting

Run `nixfmt` (`nixfmt-rfc-style`) on any Nix file before committing:
```sh
nixfmt nix/home-manager.nix
```

## What NOT to change here

- Do not modify `nix/wslstation-config.toml` by hand — it is generated from deployment configs.
- Do not add new flake inputs to `flake.nix` without user approval.
