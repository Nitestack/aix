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

        # Resolve name/version from the workspace member's Cargo.toml so crane
        # doesn't warn about a missing version in the workspace root manifest.
        crateInfo = craneLib.crateNameFromCargoToml {
          cargoToml = ./tools/aix/Cargo.toml;
        };

        commonArgs = {
          inherit src;
          inherit (crateInfo) pname version;
          strictDeps = true;
        };

        # Vendor + compile deps separately so they are cached across rebuilds.
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # The Rust CLI.  Package attribute is "aix-rs"; binary name is "aix"
        # (set by [[bin]] in tools/aix/Cargo.toml).
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

        # `nix flake check` (or `nix build .#checks.<system>.aix-rs`) builds
        # the package to verify it compiles.
        checks = {
          aix-rs = aix-rs;
        };
      }
    );
}
