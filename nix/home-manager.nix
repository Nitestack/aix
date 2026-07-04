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
    {
      api_key = encodeSecretSource profile.apiKey;
    }
    // lib.optionalAttrs (profile.label != null) {
      label = encodeSecretSource profile.label;
    };

  mkEndpoint =
    ep:
    {
      base_url = encodeSecretSource ep.baseUrl;
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
            apply = validateSecretSource;
            example = lib.literalExpression ''{ file = "/run/secrets/aix/base-url"; }'';
            description = "Gateway base URL. Accepts any secret source.";
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

            apiKey = lib.mkOption {
              type = secretSourceType;
              apply = validateSecretSource;
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
