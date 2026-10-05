{
  description = "aix — Rust CLI for LiteLLM-compatible AI gateway profiles";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        craneLib = crane.mkLib pkgs;

        # Only Rust-relevant files — avoids dirty rebuilds from doc/Nix changes.
        src = craneLib.cleanCargoSource ./.;

        # Resolve name/version from Cargo.toml so crane
        # doesn't warn about a missing version in the workspace root manifest.
        crateInfo = craneLib.crateNameFromCargoToml {
          cargoToml = ./Cargo.toml;
        };

        commonArgs = {
          inherit src;
          inherit (crateInfo) pname version;
          strictDeps = true;
        };

        # Vendor + compile deps separately so they are cached across rebuilds.
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # The Rust CLI.  Package attribute is "aix-rs"; binary name is "aix"
        # (set by [[bin]] in Cargo.toml).
        aix-rs = craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
            pname = "aix-rs";
            # version is already set via crateInfo in commonArgs.
          }
        );

        # Nix-generated example config for the wslstation deployment.
        # Profiles and secret paths mirror configurations/nixos/wslstation/sops.nix.
        # No real secret values appear here — everything is a file reference.
        exampleWslstationConfig = pkgs.writeText "aix-wslstation.toml" (
          builtins.readFile ./nix/wslstation-config.toml
        );

        homeManagerConfigs = import ./tests/home-manager-models.nix {
          inherit pkgs;
          lib = pkgs.lib;
          aixModule = ./nix/home-manager.nix;
        };

        homeManagerPromptConfigs = import ./tests/home-manager-prompts.nix {
          inherit pkgs;
          lib = pkgs.lib;
          aixModule = ./nix/home-manager.nix;
        };

        homeManagerPolicyConfigs = import ./tests/home-manager-policies.nix {
          inherit pkgs;
          lib = pkgs.lib;
          aixModule = ./nix/home-manager.nix;
        };
      in
      {
        packages = {
          aix-rs = aix-rs;
          default = aix-rs;
          example-wslstation-config = exampleWslstationConfig;
        };

        # `nix run` launches the aix binary.
        apps.default = flake-utils.lib.mkApp {
          drv = aix-rs;
          # pname is "aix-rs" but the binary is "aix".
          exePath = "/bin/aix";
        };

        # devShell: `nix develop` drops into a shell with the Rust toolchain and
        # all check derivations' inputs available so `cargo test` works locally.
        devShells.default = craneLib.devShell {
          checks = self.checks.${system};
          packages = [
            pkgs.rust-analyzer
          ];
        };

        # `nix flake check` builds the package and validates the generated
        # Home Manager model, tool, prompt, and run-policy config.
        checks = {
          aix-rs = aix-rs;
          aix-home-manager-models = pkgs.runCommand "aix-home-manager-models-check" { } ''
            grep -Fxq '[models]' ${homeManagerConfigs.configured}
            grep -Fxq 'default = "gateway/model-default"' ${homeManagerConfigs.configured}
            grep -Fxq '[models.aliases]' ${homeManagerConfigs.configured}
            grep -Fxq 'fast = "gateway/model-fast"' ${homeManagerConfigs.configured}
            grep -Fxq '[profiles.work.models]' ${homeManagerConfigs.configured}
            grep -Fxq 'default = "company/model-default"' ${homeManagerConfigs.configured}
            grep -Fxq '[profiles.work.models.aliases]' ${homeManagerConfigs.configured}
            grep -Fxq 'fast = "company/model-fast"' ${homeManagerConfigs.configured}
            grep -Fxq '[tools.review]' ${homeManagerConfigs.configured}
            grep -Fxq 'command = "review-agent"' ${homeManagerConfigs.configured}
            grep -Fxq 'api_format = "both"' ${homeManagerConfigs.configured}
            grep -Fxq 'local_gateway = true' ${homeManagerConfigs.configured}
            grep -Fxq '[tools.review.env]' ${homeManagerConfigs.configured}
            grep -Fxq 'REVIEW_MODE = "review"' ${homeManagerConfigs.configured}
            grep -q 'AIX_REVIEW_TOKEN' ${homeManagerConfigs.configured}
            grep -Fxq '[tools.codex.chatgpt]' ${homeManagerConfigs.configured}
            grep -Fxq 'transport = "local_gateway"' ${homeManagerConfigs.configured}
            grep -Fxq 'access_token_env = "ACCESS_TOKEN"' ${homeManagerConfigs.configured}
            grep -q 'app-server' ${homeManagerConfigs.configured}
            grep -q 'OPENAI_API_KEY' ${homeManagerConfigs.configured}
            grep -Fxq '[tools.claude]' ${homeManagerConfigs.configured}
            grep -Fxq 'api_format = "anthropic"' ${homeManagerConfigs.configured}
            ! grep -q 'models' ${homeManagerConfigs.legacy}
            ! grep -q 'tools' ${homeManagerConfigs.legacy}
            grep -Fxq '[profiles.personal.auth]' ${homeManagerConfigs.chatgpt}
            grep -Fxq 'type = "chatgpt"' ${homeManagerConfigs.chatgpt}
            grep -Fxq 'label = "Personal ChatGPT"' ${homeManagerConfigs.chatgpt}
            ! grep -q '^\[endpoint\]' ${homeManagerConfigs.chatgpt}
            ! grep -q 'api_key' ${homeManagerConfigs.chatgpt}
            ! grep -Eq 'access_token|refresh_token|id_token' ${homeManagerConfigs.chatgpt}
            touch $out
          '';
          aix-home-manager-prompts = pkgs.runCommand "aix-home-manager-prompts-check" { } ''
            grep -Fxq '[prompts.diagnose]' ${homeManagerPromptConfigs.configured}
            grep -Fxq 'prompt = "Analyze the supplied diagnostic output."' ${homeManagerPromptConfigs.configured}
            grep -Fxq 'system = "Distinguish evidence from inference."' ${homeManagerPromptConfigs.configured}
            grep -Fxq 'model = "smart"' ${homeManagerPromptConfigs.configured}
            grep -Fxq '[prompts.summarize]' ${homeManagerPromptConfigs.configured}
            grep -Fxq 'prompt = "Summarize the supplied material clearly and concisely."' ${homeManagerPromptConfigs.configured}
            ! grep -q 'prompts' ${homeManagerPromptConfigs.legacy}
            touch $out
          '';
          aix-home-manager-policies = pkgs.runCommand "aix-home-manager-policies-check" { } ''
            grep -Fxq '[run_policies.implement]' ${homeManagerPolicyConfigs.configured}
            grep -Fxq 'profile = "work"' ${homeManagerPolicyConfigs.configured}
            grep -q '^max_budget = 3' ${homeManagerPolicyConfigs.configured}
            grep -Fxq 'max_duration = "2h"' ${homeManagerPolicyConfigs.configured}
            grep -q 'allowed_models' ${homeManagerPolicyConfigs.configured}
            grep -q 'phase:implement' ${homeManagerPolicyConfigs.configured}
            grep -Fxq '[run_policies.research]' ${homeManagerPolicyConfigs.configured}
            grep -Fxq 'max_duration = "1h"' ${homeManagerPolicyConfigs.configured}
            test "$(grep -A2 -F '[run_policies.research]' ${homeManagerPolicyConfigs.configured} | grep -c '^max_budget =')" -eq 0
            ! grep -q 'run_policies' ${homeManagerPolicyConfigs.legacy}
            touch $out
          '';
        };
      }
    )
    // {
      # Adds pkgs.aix-rs to nixpkgs — primary install path for users who
      # manage their config file themselves without the home-manager module.
      # Only available for the systems built by this flake (eachDefaultSystem).
      # Applying this overlay on an unsupported system is a hard eval error.
      overlays.default = final: prev: {
        aix-rs = self.packages.${prev.stdenv.hostPlatform.system}.aix-rs;
      };

      # Optional home-manager integration. Imports the module and wires the
      # package default so consumers don't need to set programs.aix.package.
      homeManagerModules.aix =
        { pkgs, lib, ... }:
        {
          imports = [ ./nix/home-manager.nix ];
          config.programs.aix.package = lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.aix-rs;
        };
      homeManagerModules.default = self.homeManagerModules.aix;
    };
}
