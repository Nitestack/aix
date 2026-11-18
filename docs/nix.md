# Nix and home-manager

The flake exposes a home-manager module, an overlay, and a package. Nix is only
the deployment mechanism: the module generates the same config file the CLI
reads on any platform, so everything in [Configuration](configuration.md)
applies. For the overlay and `nix run`, see [Installation](installation.md#nix).

## home-manager module

Add the flake input, import the module, and declare your config declaratively. Nix installs the binary and generates `~/.config/aix/aix.toml` at activation time.

```nix
# flake.nix
inputs.aix.url = "github:get-aix/aix";
```

```nix
# home-manager config
imports = [ inputs.aix.homeManagerModules.aix ];

programs.aix = {
  enable = true;
  defaultProfile = "work";

  endpoint = {
    # Any secret source: { env = "VAR"; }, { file = "/run/secrets/..."; },
    # { command = "pass show ..."; }, or a plain string (stored in Nix store).
    baseUrl = { file = "/run/secrets/aix/base-url"; };
  };

  profiles.work = {
    label  = "Work";
    apiKey = { file = "/run/secrets/aix/work-key"; };
  };
};
```

Then rebuild:

```bash
nixos-rebuild switch --flake .#your-host
# or, standalone home-manager:
home-manager switch --flake .#your-user
```

## Option mapping

Module options mirror the config file with camelCase names. Each feature page
shows the Home Manager form next to the TOML:

| Config file | Home Manager option | Docs |
|---|---|---|
| `[models]`, `[profiles.<name>.models]` | `programs.aix.models`, `programs.aix.profiles.<name>.models` | [Model defaults and aliases](configuration.md#model-defaults-and-aliases) |
| `auth = { type = "chatgpt" }` | `programs.aix.profiles.<name>.auth = "chatgpt"` | [ChatGPT authentication](chatgpt.md) |
| `[prompts.<name>]` | `programs.aix.prompts` | [Reusable prompt presets](inference.md#reusable-prompt-presets) |
| `[tools.<name>]` | `programs.aix.tools.<name>` | [Named tool launch configuration](tools.md#named-tool-launch-configuration) |
| `[tools.<name>.chatgpt]` | `programs.aix.tools.<name>.chatgpt` | [Codex app-server](tools.md#codex-app-server-with-chatgpt-plan-authentication) |
| `[profiles.<name>.tool_configs.codex]` | `programs.aix.profiles.<name>.toolConfigs.codex.configDir` | [Codex](tools.md#profile-specific-codex-configuration) |
| `[profiles.<name>.tool_configs.pi]` | `programs.aix.profiles.<name>.toolConfigs.pi.agentDir` | [Pi](tools.md#profile-specific-pi-configuration) |
| `[profiles.<name>.tool_configs.opencode]` | `programs.aix.profiles.<name>.toolConfigs.opencode` | [OpenCode](tools.md#profile-specific-opencode-configuration) |
| `[run_policies.<name>]` | `programs.aix.runPolicies.<name>` | [Run policies](runs.md#run-policies) |
| `[cache]` | `programs.aix.cache` | [Cache configuration](gateway.md#cache-configuration) |

The authoritative option list, with types and descriptions, is
[`nix/home-manager.nix`](../nix/home-manager.nix).

If you use the Nix deployment with `file` secret sources pointing to
`/run/secrets/aix/*`, those paths are safe to commit; see
[Security notes](security.md#never-commit-real-keys).
