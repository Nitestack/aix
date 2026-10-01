{
  lib,
  pkgs,
  aixModule,
}:

let
  render =
    {
      prompts ? { },
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
                inherit prompts;
                profiles.work.apiKey = "test-key";
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
    prompts = {
      summarize = {
        prompt = "Summarize the supplied material clearly and concisely.";
      };
      diagnose = {
        prompt = "Analyze the supplied diagnostic output.";
        system = "Distinguish evidence from inference.";
        model = "smart";
      };
    };
  };

  legacy = render { };
}
