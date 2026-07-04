# Nix Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `modules/home/aix.nix` (home-manager module) and update `flake.nix` with `overlays.default` and `homeManagerModules.aix/default`.

**Architecture:** Two-file change. The module generates `~/.config/aix/aix.toml` via `xdg.configFile` using `pkgs.formats.toml`. The flake wrapper (`homeManagerModules.aix`) sets the package default using `self.packages.${pkgs.system}.aix-rs` so consumers get the bundled binary automatically. `overlays.default` exposes the same package for users who don't use the HM module.

**Tech Stack:** Nix, home-manager module system (`lib.types`, `lib.mkOption`, `lib.mkIf`, `lib.mkEnableOption`), `pkgs.formats.toml`, `alejandra` formatter.

---

### Task 1: Create `modules/home/aix.nix`

**Files:**
- Create: `modules/home/aix.nix`

- [ ] **Step 1: Create the module file**

Create `modules/home/aix.nix` with the full module content:

```nix
{ config, lib, pkgs, ... }:

let
  cfg = config.programs.aix;

  secretSourceType = lib.types.oneOf [
    lib.types.str
    (lib.types.submodule {
      options.env = lib.mkOption {
        type = lib.types.str;
        description = "Environment variable name containing the secret.";
      };
    })
    (lib.types.submodule {
      options.file = lib.mkOption {
        type = lib.types.str;
        description = ''
          Path to a file containing the secret (e.g. /run/secrets/aix/key).
          Trailing newline is stripped automatically by the CLI.
          Use lib.types.str (not path) so Nix does not copy runtime paths into the store.
        '';
      };
    })
    (lib.types.submodule {
      options.command = lib.mkOption {
        type = lib.types.str;
        description = "Shell command whose stdout becomes the secret. Trailing newline is stripped.";
      };
    })
  ];

  mkProfile = _name: profile:
    { api_key = profile.apiKey; }
    // lib.optionalAttrs (profile.label != null) { label = profile.label; };

  mkEndpoint = ep:
    {
      base_url = ep.baseUrl;
      api_format = ep.apiFormat;
    }
    // lib.optionalAttrs (ep.gateway != null) { gateway = ep.gateway; }
    // lib.optionalAttrs (ep.provider != null) { provider = ep.provider; };

  configAttrs =
    lib.optionalAttrs (cfg.defaultProfile != null) { default_profile = cfg.defaultProfile; }
    // {
      endpoint = mkEndpoint cfg.endpoint;
      profiles = lib.mapAttrs mkProfile cfg.profiles;
    };

in
{
  options.programs.aix = {
    enable = lib.mkEnableOption "aix AI gateway profile manager";

    package = lib.mkOption {
      type = lib.types.package;
      defaultText = lib.literalExpression "inputs.aix.packages.\${pkgs.system}.default";
      description = ''
        The aix package to install. Set automatically when importing via
        homeManagerModules.aix. Override to pin a specific version.
      '';
    };

    defaultProfile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "work";
      description = ''
        Profile used when --profile is absent and stdin/stdout are not both TTYs.
        Omit to always use the interactive picker in TTY sessions.
      '';
    };

    endpoint = lib.mkOption {
      description = "Gateway endpoint shared by all profiles.";
      type = lib.types.submodule {
        options = {
          baseUrl = lib.mkOption {
            type = secretSourceType;
            example = lib.literalExpression ''{ file = "/run/secrets/aix/base-url"; }'';
            description = "Gateway base URL. Accepts any secret source.";
          };

          apiFormat = lib.mkOption {
            type = lib.types.enum [ "anthropic" "openai" "both" ];
            example = "anthropic";
            description = ''
              Wire format emitted to downstream tools.
              anthropic: ANTHROPIC_API_KEY + ANTHROPIC_BASE_URL.
              openai:    OPENAI_API_KEY + OPENAI_BASE_URL.
              both:      all four variables.
            '';
          };

          gateway = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
            example = "litellm";
            description = "Optional gateway hint. Metadata only — does not affect runtime behaviour.";
          };

          provider = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
            description = "Optional provider hint. Metadata only — does not affect runtime behaviour.";
          };
        };
      };
    };

    profiles = lib.mkOption {
      description = "Named API profiles. At least one must be defined when enable = true.";
      default = { };
      type = lib.types.attrsOf (lib.types.submodule {
        options = {
          label = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
            example = "Work";
            description = "Human-readable label shown in the interactive profile picker.";
          };

          apiKey = lib.mkOption {
            type = secretSourceType;
            example = lib.literalExpression ''{ file = "/run/secrets/aix/work-key"; }'';
            description = ''
              API key for this profile. Accepts any secret source.
              Avoid direct string values — they end up in the world-readable Nix store.
              Prefer file (sops-nix / agenix), env, or command.
            '';
          };
        };
      });
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.profiles != { };
        message = "programs.aix.profiles must define at least one profile when programs.aix.enable = true.";
      }
    ];

    home.packages = [ cfg.package ];

    xdg.configFile."aix/aix.toml".source =
      (pkgs.formats.toml { }).generate "aix.toml" configAttrs;
  };
}
```

- [ ] **Step 2: Verify the file is valid Nix syntax**

```bash
nix-instantiate --parse modules/home/aix.nix
```

Expected: prints the parsed Nix AST with no errors. Any `error:` output means a syntax problem.

- [ ] **Step 3: Format with alejandra**

```bash
alejandra modules/home/aix.nix
```

Expected: file is reformatted (or unchanged if already clean). No error output.

- [ ] **Step 4: Commit**

```bash
git add modules/home/aix.nix
git commit -m "feat(nix): add home-manager module for aix"
```

---

### Task 2: Update `flake.nix` — add overlays and homeManagerModules

**Files:**
- Modify: `flake.nix`

The current `flake.nix` ends with:

```nix
  outputs =
    {
      self,
      nixpkgs,
      crane,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      ...per-system outputs...
    );
```

We need to merge system-independent outputs (`overlays`, `homeManagerModules`) into the top-level attrset. Do this by replacing the bare `flake-utils.lib.eachDefaultSystem (...)` with `(flake-utils.lib.eachDefaultSystem (...)) // { ... }`.

- [ ] **Step 1: Edit `flake.nix` to add the system-independent outputs**

The final `};` closing the `outputs` function currently looks like:

```nix
    );
}
```

Replace it with:

```nix
    )
    // {
      overlays.default = final: prev: {
        aix-rs = self.packages.${prev.system}.aix-rs;
      };

      homeManagerModules.aix =
        { pkgs, lib, ... }:
        {
          imports = [ ./modules/home/aix.nix ];
          config.programs.aix.package = lib.mkDefault self.packages.${pkgs.system}.aix-rs;
        };
      homeManagerModules.default = self.homeManagerModules.aix;
    };
}
```

The full bottom of `flake.nix` after the edit (from the `checks` block onward) should read:

```nix
        # `nix flake check` (or `nix build .#checks.<system>.aix-rs`) builds
        # the package to verify it compiles.
        checks = {
          aix-rs = aix-rs;
        };
      }
    )
    // {
      overlays.default = final: prev: {
        aix-rs = self.packages.${prev.system}.aix-rs;
      };

      homeManagerModules.aix =
        { pkgs, lib, ... }:
        {
          imports = [ ./modules/home/aix.nix ];
          config.programs.aix.package = lib.mkDefault self.packages.${pkgs.system}.aix-rs;
        };
      homeManagerModules.default = self.homeManagerModules.aix;
    };
}
```

- [ ] **Step 2: Format with alejandra**

```bash
alejandra flake.nix
```

Expected: file reformatted cleanly.

- [ ] **Step 3: Verify the flake evaluates**

```bash
nix flake show
```

Expected output includes the new keys (system will vary):

```
├───homeManagerModules
│   ├───aix: home-manager module
│   └───default: home-manager module
└───overlays
    └───default: Nixpkgs overlay
```

And the existing per-system outputs (`packages`, `apps`, `devShells`, `checks`) are still present.

- [ ] **Step 4: Run `nix flake check`**

```bash
nix flake check
```

Expected: exits 0. This builds `checks.${system}.aix-rs` (the Rust CLI) and verifies the flake schema is valid.

- [ ] **Step 5: Spot-check the overlay**

```bash
nix eval '.#overlays.default' --apply 'ov: "overlay is a function: ${builtins.typeOf ov}"'
```

Expected: `"overlay is a function: lambda"`

- [ ] **Step 6: Spot-check `homeManagerModules.aix`**

```bash
nix eval '.#homeManagerModules.aix' --apply 'mod: "module loaded: ${builtins.typeOf mod}"'
```

Expected: `"module loaded: lambda"`

- [ ] **Step 7: Commit**

```bash
git add flake.nix
git commit -m "feat(nix): add overlays.default and homeManagerModules to flake"
```
