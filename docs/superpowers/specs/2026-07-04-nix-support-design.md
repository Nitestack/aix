# Nix support — design spec

**Date:** 2026-07-04

## Goal

Add first-class Nix support so NixOS / home-manager users can install and configure `aix` entirely through Nix. The CLI must continue to work without Nix; the Nix layer is optional.

---

## Flake outputs

### Existing (unchanged)

| Output | Description |
|--------|-------------|
| `packages.${system}.aix-rs` | The compiled CLI derivation |
| `packages.${system}.default` | Alias for `aix-rs` |
| `apps.${system}.default` | `nix run .` entry point |
| `devShells.${system}.default` | Dev shell with Rust toolchain |
| `checks.${system}.aix-rs` | `nix flake check` build verification |

### New (system-independent, outside `eachDefaultSystem`)

| Output | Description |
|--------|-------------|
| `overlays.default` | Adds `pkgs.aix-rs` to nixpkgs; primary path for users who manage config themselves |
| `homeManagerModules.aix` | Optional home-manager integration |
| `homeManagerModules.default` | Alias for `homeManagerModules.aix` |

### Installation paths

1. **No home-manager** — apply `overlays.default`, add `pkgs.aix-rs` to `home.packages` or `environment.systemPackages`; manage `~/.config/aix/aix.toml` yourself.
2. **With home-manager module** — import `homeManagerModules.aix`; module installs the binary and generates the config file from declarative options.
3. **One-off** — `nix run github:Nitestack/aix`.

---

## Home-manager module (`modules/home/aix.nix`)

### Secret source type

Each secret field (`endpoint.baseUrl`, `profiles.<name>.apiKey`) accepts a `secretSourceType`:

```nix
secretSourceType = lib.types.oneOf [
  lib.types.str                                                          # direct literal — world-readable in Nix store
  (lib.types.submodule { options.env     = mkOption { type = str; }; }) # { env = "VAR_NAME"; }
  (lib.types.submodule { options.file    = mkOption { type = str; }; }) # { file = "/run/secrets/..."; }
  (lib.types.submodule { options.command = mkOption { type = str; }; }) # { command = "pass show ..."; }
];
```

`file` uses `lib.types.str` (not `lib.types.path`) so Nix does not attempt to copy runtime paths like `/run/secrets/...` into the store.

**Security note:** `direct` string values are embedded in the Nix store, which is world-readable. Use `file`, `env`, or `command` for real secrets. For sops-nix / agenix deployments, `file` pointing to a `/run/secrets/...` path is the recommended approach — the path reference is store-safe; only the file contents are sensitive.

### Option tree

```
programs.aix
├── enable            lib.mkEnableOption
│                     Installs the binary and writes the config file.
├── package           lib.types.package
│                     No default in the module file; set to the flake's
│                     aix-rs derivation by the homeManagerModules wrapper.
│                     Override to pin a different version.
├── defaultProfile    lib.types.nullOr lib.types.str   default: null
│                     Profile used when --profile is absent and
│                     stdin/stdout are not both TTYs.
├── endpoint          lib.types.submodule
│   ├── baseUrl       secretSourceType                  (required)
│   ├── apiFormat     lib.types.enum ["anthropic" "openai" "both"]  (required)
│   ├── gateway       lib.types.nullOr lib.types.str   default: null  (metadata only)
│   └── provider      lib.types.nullOr lib.types.str   default: null  (metadata only)
└── profiles          lib.types.attrsOf profileSubmodule   default: {}
    └── <name>
        ├── label     lib.types.nullOr lib.types.str   default: null
        └── apiKey    secretSourceType                  (required)
```

### Assertions

One `assertions` entry is added when the module is enabled:

- At least one profile must be defined. Mirrors the CLI's `NoProfilesConfigured` error, but caught at `nixos-rebuild` time rather than at runtime.

### Config generation

`xdg.configFile."aix/aix.toml".source` is set to the result of:

```nix
(pkgs.formats.toml {}).generate "aix.toml" configAttrs
```

where `configAttrs` is the Nix attrset produced by:

1. Transforming camelCase option names → snake_case TOML keys:
   - `endpoint.baseUrl` → `endpoint.base_url`
   - `endpoint.apiFormat` → `endpoint.api_format`
   - `profiles.<n>.apiKey` → `profiles.<n>.api_key`
2. Stripping `null` values so optional keys are omitted entirely.
3. Passing secret source values through as-is — `{ env = "VAR"; }` in Nix serializes to a TOML table `[endpoint.base_url]\nenv = "VAR"`, which is semantically identical to the inline form `base_url = { env = "VAR" }` and parsed identically by the `toml` crate's `#[serde(untagged)]` deserializer.

### Module behaviour

When `programs.aix.enable = true`:

- `home.packages = [ cfg.package ]` — installs the binary.
- `xdg.configFile."aix/aix.toml".source = ...` — writes the config.

### Flake wrapper

`homeManagerModules.aix` is not the raw module file — it is a wrapper:

```nix
homeManagerModules.aix = { pkgs, lib, ... }: {
  imports = [ ./modules/home/aix.nix ];
  config.programs.aix.package = lib.mkDefault self.packages.${pkgs.system}.aix-rs;
};
```

This means users who import via the flake get the bundled binary automatically. Users who import `./modules/home/aix.nix` directly must set `programs.aix.package` themselves.

---

## Consumer example

```nix
# flake.nix (consumer)
inputs.aix.url = "github:Nitestack/aix";

# home-manager config
imports = [ inputs.aix.homeManagerModules.aix ];

programs.aix = {
  enable = true;
  defaultProfile = "swtb";
  endpoint = {
    baseUrl    = { file = "/run/secrets/aihub/base-url"; };
    apiFormat  = "anthropic";
  };
  profiles.swtb  = { label = "SWTB";  apiKey = { file = "/run/secrets/aihub/swtb"; }; };
  profiles.work  = { label = "Work";  apiKey = { file = "/run/secrets/aihub/work"; }; };
};
```

---

## Files changed

| File | Change |
|------|--------|
| `modules/home/aix.nix` | New — home-manager module |
| `flake.nix` | Add `overlays.default`, `homeManagerModules.aix/default` |
