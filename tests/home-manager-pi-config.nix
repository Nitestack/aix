{
  lib,
  pkgs,
  aixModule,
}:

let
  render =
    {
      piAgentDir ? null,
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
                profiles.work = {
                  apiKey = {
                    env = "AIX_WORK_KEY";
                  };
                  toolConfigs = lib.optionalAttrs (piAgentDir != null) {
                    pi.agentDir = piAgentDir;
                  };
                };
                tools.pi.apiFormat = "openai";
              };
            }
          )
        ];
      };
    in
    assert builtins.all (assertion: assertion.assertion) evaluated.config.assertions;
    evaluated.config.xdg.configFile."aix/aix.toml".source;
in
{
  configured = render { piAgentDir = "~/.config/pi/work-agent"; };
  legacy = render { };
}
