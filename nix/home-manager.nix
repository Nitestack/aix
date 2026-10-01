{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.aix;

  rawSecretSourceType = lib.types.submodule {
    options = {
      value = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        visible = false;
        description = "Literal secret value. Stored in the Nix store when used.";
      };

      env = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Environment variable name containing the secret.";
      };

      file = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = ''
          Path to a file containing the secret (e.g. /run/secrets/aix/key).
          Trailing newline is stripped automatically by the CLI.
          Uses lib.types.str (not path) so Nix does not copy runtime paths into the store.
        '';
      };

      command = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Shell command whose stdout becomes the secret. Trailing newline is stripped.";
      };
    };
  };

  secretSourceType = lib.types.coercedTo lib.types.str (value: {
    inherit value;
  }) rawSecretSourceType;

  modelConfigType = lib.types.submodule {
    options = {
      default = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "gateway/model-fast";
        description = "Default raw model ID; aliases are not resolved here.";
      };

      aliases = lib.mkOption {
        type = lib.types.attrsOf lib.types.str;
        default = { };
        example = {
          fast = "gateway/model-fast";
          smart = "gateway/model-smart";
        };
        description = "Local model names mapped to raw model IDs.";
      };
    };
  };

  toolConfigType = lib.types.submodule {
    options = {
      command = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "review-agent";
        description = "Executable to launch; defaults to the tool name.";
      };

      apiFormat = lib.mkOption {
        type = lib.types.nullOr (
          lib.types.enum [
            "anthropic"
            "openai"
            "both"
          ]
        );
        default = null;
        example = "openai";
        description = "Credential variable format to provide to this tool.";
      };

      args = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        description = "Arguments prepended to user arguments. {model} expands to the profile model default.";
      };

      env = lib.mkOption {
        type = lib.types.attrsOf secretSourceType;
        default = { };
        description = ''
          Additional environment variables for this tool. Values accept the same
          secret sources as profile environment variables. Tool values override
          profile and generated credential variables.
        '';
      };
    };
  };

  hasModelConfig = models: models.default != null || models.aliases != { };

  mkModelConfig =
    models:
    lib.optionalAttrs (models.default != null) { default = models.default; }
    // lib.optionalAttrs (models.aliases != { }) { aliases = models.aliases; };

  mkTool =
    _name: tool:
    lib.optionalAttrs (tool.apiFormat != null) {
      api_format = tool.apiFormat;
    }
    // lib.optionalAttrs (tool.args != [ ]) { args = tool.args; }
    // lib.optionalAttrs (tool.command != null) {
      command = tool.command;
    }
    // lib.optionalAttrs (tool.env != { }) {
      env = lib.mapAttrs (_: encodeSecretSource) tool.env;
    };

  validateSecretSource =
    source:
    let
      populatedFields = builtins.filter (value: value != null) [
        source.value
        source.env
        source.file
        source.command
      ];
    in
    if builtins.length populatedFields == 1 then
      source
    else
      throw "Secret source must set exactly one of value, env, file, or command.";

  encodeSecretSource =
    source:
    if source.value != null then
      source.value
    else if source.env != null then
      { env = source.env; }
    else if source.file != null then
      { file = source.file; }
    else
      { command = source.command; };

  mkProfile =
    _name: profile:
    lib.optionalAttrs (profile.auth != "api_key") { auth = profile.auth; }
    // lib.optionalAttrs (profile.apiKey != null) {
      api_key = encodeSecretSource profile.apiKey;
    }
    // lib.optionalAttrs (profile.tools != { }) { tools = lib.mapAttrs mkTool profile.tools; }
    // lib.optionalAttrs (profile.ask != null) {
      ask = { inherit (profile.ask) command args; };
    }
    // lib.optionalAttrs (profile.label != null) {
      label = encodeSecretSource profile.label;
    }
    // lib.optionalAttrs (profile.baseUrl != null) {
      base_url = encodeSecretSource profile.baseUrl;
    }
    // lib.optionalAttrs (profile.env != { }) {
      env = lib.mapAttrs (_: encodeSecretSource) profile.env;
    }
    // lib.optionalAttrs (hasModelConfig profile.models) {
      models = mkModelConfig profile.models;
    };

  mkEndpoint =
    ep:
    lib.optionalAttrs (ep.baseUrl != null) {
      base_url = encodeSecretSource ep.baseUrl;
    }
    // lib.optionalAttrs (ep.gateway != null) { gateway = ep.gateway; }
    // lib.optionalAttrs (ep.provider != null) { provider = ep.provider; };

  configAttrs =
    lib.optionalAttrs (cfg.defaultProfile != null) { default_profile = cfg.defaultProfile; }
    // {
      endpoint = mkEndpoint cfg.endpoint;
      profiles = lib.mapAttrs mkProfile cfg.profiles;
    }
    // lib.optionalAttrs (hasModelConfig cfg.models) {
      models = mkModelConfig cfg.models;
    }
    // lib.optionalAttrs (cfg.tools != { }) {
      tools = lib.mapAttrs mkTool cfg.tools;
    }
    // lib.optionalAttrs (cfg.cache.ttlSecs != 3600 || cfg.cache.disabled) {
      cache = {
        ttl_secs = cfg.cache.ttlSecs;
        disabled = cfg.cache.disabled;
      };
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
      description = "Gateway endpoint shared by API-key profiles; optional for native subscription profiles.";
      default = { };
      type = lib.types.submodule {
        options = {
          baseUrl = lib.mkOption {
            type = lib.types.nullOr secretSourceType;
            default = null;
            apply = v: if v != null then validateSecretSource v else null;
            example = lib.literalExpression ''{ file = "/run/secrets/aix/base-url"; }'';
            description = "Gateway base URL. Accepts any secret source.";
          };

          gateway = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
            example = "litellm";
            description = ''
              Optional gateway hint.
              When set to a value other than "litellm", `aix spend` and `aix usage` will refuse to run.
              Omit or set to "litellm" to use those commands with the default LiteLLM-compatible gateway.
            '';
          };

          provider = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
            description = "Optional provider hint. Metadata only — does not affect runtime behaviour.";
          };
        };
      };
    };

    cache = lib.mkOption {
      description = "Cache settings for aix spend and aix status LiteLLM responses.";
      default = { };
      type = lib.types.submodule {
        options = {
          ttlSecs = lib.mkOption {
            type = lib.types.int;
            default = 3600;
            example = 600;
            description = ''
              Cache TTL in seconds. Cached entries older than this are re-fetched.
              Set to 0 to never expire cached entries.
            '';
          };
          disabled = lib.mkOption {
            type = lib.types.bool;
            default = false;
            description = "Disable the response cache entirely. Equivalent to always passing --no-cache.";
          };
        };
      };
    };

    models = lib.mkOption {
      description = "Default model ID and aliases shared by all profiles.";
      default = { };
      type = modelConfigType;
    };

    tools = lib.mkOption {
      description = "Generic launch wiring for named tools.";
      default = { };
      type = lib.types.attrsOf toolConfigType;
    };

    profiles = lib.mkOption {
      description = "Named API-key or native subscription profiles. At least one must be defined when enable = true.";
      default = { };
      type = lib.types.attrsOf (
        lib.types.submodule {
          options = {
            label = lib.mkOption {
              type = lib.types.nullOr secretSourceType;
              default = null;
              apply = v: if v != null then validateSecretSource v else null;
              example = lib.literalExpression ''{ file = "/run/secrets/aix/work-label"; }'';
              description = ''
                Display label shown in the interactive profile picker.
                Accepts a plain string, or { env = "VAR"; }, { file = "/path"; }, { command = "cmd"; }.
                Use a non-literal source to avoid leaking account names into the Nix store.
              '';
            };

            auth = lib.mkOption {
              type = lib.types.enum [
                "api_key"
                "native"
              ];
              default = "api_key";
              description = "Use gateway credentials or the launched tool's own subscription login.";
            };

            tools = lib.mkOption {
              type = lib.types.attrsOf toolConfigType;
              default = { };
              description = "Launch entries for this profile; each entry replaces a matching shared tool entry.";
            };

            ask = lib.mkOption {
              default = null;
              description = "Text-only inference command for native subscriptions. Receives user context on stdin and returns assistant text on stdout; args must contain {model} and {system}. JSON token usage is unavailable.";
              type = lib.types.nullOr (
                lib.types.submodule {
                  options = {
                    command = lib.mkOption { type = lib.types.str; };
                    args = lib.mkOption {
                      type = lib.types.listOf lib.types.str;
                      default = [ ];
                    };
                  };
                }
              );
            };

            apiKey = lib.mkOption {
              type = lib.types.nullOr secretSourceType;
              default = null;
              apply = v: if v != null then validateSecretSource v else null;
              example = lib.literalExpression ''{ file = "/run/secrets/aix/work-key"; }'';
              description = ''
                API key for this profile. Accepts any secret source.
                Avoid direct string values — they end up in the world-readable Nix store.
                Prefer file (sops-nix / agenix), env, or command.
              '';
            };

            baseUrl = lib.mkOption {
              type = lib.types.nullOr secretSourceType;
              default = null;
              apply = v: if v != null then validateSecretSource v else null;
              example = "https://local-ai.example.com";
              description = ''
                Optional gateway base URL override for this profile.
                When unset, the shared programs.aix.endpoint.baseUrl is used.
                Accepts any secret source.
              '';
            };

            env = lib.mkOption {
              type = lib.types.attrsOf secretSourceType;
              default = { };
              description = ''
                Additional environment variables injected for this profile.
                Values accept a plain string or { env = "VAR"; }, { file = "/path"; },
                or { command = "cmd"; }. Profile values are applied after aix's
                standard credential variables, so they can override them when necessary.
              '';
            };

            models = lib.mkOption {
              description = "Model defaults and aliases that override the shared programs.aix.models settings.";
              default = { };
              type = modelConfigType;
            };
          };
        }
      );
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.profiles != { };
        message = "programs.aix.profiles must define at least one profile when programs.aix.enable = true.";
      }
    ]
    ++ lib.mapAttrsToList (name: profile: {
      assertion =
        if profile.auth == "native" then
          profile.apiKey == null && profile.baseUrl == null
        else
          profile.apiKey != null && (profile.baseUrl != null || cfg.endpoint.baseUrl != null);
      message = "programs.aix.profiles.${name}: API-key profiles require apiKey and a base URL; native profiles must omit apiKey and baseUrl.";
    }) cfg.profiles
    ++ lib.mapAttrsToList (name: tool: {
      assertion = tool.apiFormat != null;
      message = "programs.aix.tools.${name} requires apiFormat.";
    }) cfg.tools;

    home.packages = [ cfg.package ];

    xdg.configFile."aix/aix.toml".source = (pkgs.formats.toml { }).generate "aix.toml" configAttrs;
  };
}
