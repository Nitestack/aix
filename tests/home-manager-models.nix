{
  lib,
  pkgs,
  aixModule,
}:

let
  render =
    {
      models ? { },
      workModels ? { },
      tools ? { },
      native ? false,
    }:
    let
      nativeModules = [
        ../nix/chatgpt-example.nix
        ../nix/anthropic-example.nix
      ];
      evaluated = lib.evalModules {
        specialArgs = { inherit pkgs; };
        modules = lib.optionals native nativeModules ++ [
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
                endpoint.baseUrl =
                  if native then
                    null
                  else
                    {
                      env = "AIX_GATEWAY_URL";
                    };
                inherit models;
                inherit tools;
                profiles = lib.optionalAttrs (!native) {
                  work = {
                    apiKey = {
                      env = "AIX_TEST_API_KEY";
                    };
                    models = workModels;
                  };
                };
              };
            }
          )
        ];
      };
    in
    assert builtins.all (entry: entry.assertion) evaluated.config.assertions;
    evaluated.config.xdg.configFile."aix/aix.toml".source;
in
{
  configured = render {
    models = {
      default = "gateway/model-default";
      aliases.fast = "gateway/model-fast";
    };
    tools = {
      review = {
        command = "review-agent";
        apiFormat = "both";
        env = {
          REVIEW_MODE = "review";
          REVIEW_TOKEN = {
            env = "AIX_REVIEW_TOKEN";
          };
        };
      };
      claude = {
        apiFormat = "anthropic";
      };
    };
    workModels = {
      default = "company/model-default";
      aliases.fast = "company/model-fast";
    };
  };

  legacy = render { };

  native = render { native = true; };
}
