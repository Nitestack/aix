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
                endpoint.baseUrl = {
                  env = "AIX_GATEWAY_URL";
                };
                inherit models;
                profiles.work = {
                  apiKey = {
                    env = "AIX_TEST_API_KEY";
                  };
                  models = workModels;
                };
              };
            }
          )
        ];
      };
    in
    evaluated.config.xdg.configFile."aix/aix.toml".source;
in
{
  configured = render {
    models = {
      default = "gateway/model-default";
      aliases.fast = "gateway/model-fast";
    };
    workModels = {
      default = "company/model-default";
      aliases.fast = "company/model-fast";
    };
  };

  legacy = render { };
}
