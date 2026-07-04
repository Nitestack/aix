{
  config,
  lib,
  pkgs,
  ...
}:

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
          Uses lib.types.str (not path) so Nix does not copy runtime paths into the store.
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

  mkProfile =
    _name: profile:
    {
      api_key = profile.apiKey;
    }
    // lib.optionalAttrs (profile.label != null) { label = profile.label; };

  mkEndpoint =
    ep:
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
            type = lib.types.enum [
              "anthropic"
              "openai"
              "both"
            ];
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
      type = lib.types.attrsOf (
        lib.types.submodule {
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
    ];

    home.packages = [ cfg.package ];

    xdg.configFile."aix/aix.toml".source = (pkgs.formats.toml { }).generate "aix.toml" configAttrs;
  };
}
