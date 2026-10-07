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
      workToolConfigs ? { },
      tools ? { },
      chatgpt ? false,
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
                endpoint = lib.optionalAttrs (!chatgpt) {
                  baseUrl = {
                    env = "AIX_GATEWAY_URL";
                  };
                };
                inherit models;
                inherit tools;
                profiles =
                  if chatgpt then
                    {
                      personal = {
                        auth = "chatgpt";
                        label = "Personal ChatGPT";
                        models = workModels;
                      };
                    }
                  else
                    {
                      work = {
                        apiKey = {
                          env = "AIX_TEST_API_KEY";
                        };
                        models = workModels;
                        toolConfigs = workToolConfigs;
                      };
                    };
              };
            }
          )
        ];
      };
    in
    assert builtins.all (assertion: assertion.assertion) evaluated.config.assertions;
    evaluated.config.xdg.configFile."aix/aix.toml".source;
in
assert
  !(builtins.tryEval (
    builtins.readFile (render {
      workToolConfigs.unknown.configDir = "/tmp/unknown";
    })
  )).success;
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
        localGateway = true;
        env = {
          REVIEW_MODE = "review";
          REVIEW_TOKEN = {
            env = "AIX_REVIEW_TOKEN";
          };
        };
      };
      codex = {
        command = "codex";
        apiFormat = "openai";
        chatgpt = {
          transport = "local_gateway";
          accessTokenEnv = "ACCESS_TOKEN";
          prependArgs = [
            "app-server"
            "--listen"
            "stdio://"
          ];
          clearEnv = [
            "OPENAI_API_KEY"
            "CODEX_API_KEY"
          ];
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
    workToolConfigs.codex.configDir = "/profiles/work/codex";
    workToolConfigs.opencode = {
      configFile = "/run/secrets/aix/opencode/work.json";
      cliConfigFile = "../opencode/work-cli.json";
    };
  };

  legacy = render { };

  chatgpt = render {
    chatgpt = true;
    workModels = {
      default = "openai/gpt-codex";
      aliases.fast = "openai/gpt-codex-mini";
    };
  };
}
