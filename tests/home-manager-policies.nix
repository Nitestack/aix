{
  lib,
  pkgs,
  aixModule,
}:

let
  render =
    {
      runPolicies ? { },
    }:
    let
      evaluated = lib.evalModules {
        specialArgs = { inherit pkgs; };
        modules = [
          aixModule
          (
            { lib, ... }:
            {
              options = {
                assertions = lib.mkOption {
                  type = lib.types.listOf lib.types.anything;
                  default = [ ];
                };
                home.packages = lib.mkOption {
                  type = lib.types.listOf lib.types.package;
                  default = [ ];
                };
                xdg.configFile = lib.mkOption {
                  type = lib.types.attrsOf lib.types.anything;
                  default = { };
                };
              };
            }
          )
          (
            { ... }:
            {
              programs.aix = {
                enable = true;
                package = pkgs.hello;
                endpoint.baseUrl = "https://gateway.example";
                profiles.work.apiKey = "test-key";
                inherit runPolicies;
              };
            }
          )
        ];
      };
    in
    assert lib.all (assertion: assertion.assertion) evaluated.config.assertions;
    evaluated.config.xdg.configFile."aix/aix.toml".source;
in
assert
  !(builtins.tryEval (render {
    runPolicies.invalid = {
      maxBudget = 0;
      maxDuration = "1h";
    };
  })).success;
{
  configured = render {
    runPolicies = {
      implement = {
        profile = "work";
        maxBudget = 3.0;
        maxDuration = "2h";
        allowedModels = [
          "smart"
          "fast"
        ];
        tags = [ "phase:implement" ];
      };
      research = {
        maxDuration = "1h";
      };
    };
  };

  legacy = render { };
}
