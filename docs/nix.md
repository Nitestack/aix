# Nix and home-manager

The flake exposes a home-manager module, an overlay, and a package. Nix is only
the deployment mechanism: the module generates the same config file the CLI
reads on any platform, so everything in [Configuration](configuration.md)
applies. For the overlay and `nix run`, see [Installation](installation.md#nix).

## Stable and nightly refs

The aix flake builds from source. Choose a Git ref in the input URL to select a
stable release or the rolling `main` branch:

```nix
# Stable: use the tag of the release you want (v1.0.1 is an example).
inputs.aix.url = "github:get-aix/aix/v1.0.1";

# Nightly: follow the tip of main.
# inputs.aix.url = "github:get-aix/aix/main";
```

The `flake.lock` file pins the exact commit, including for `main`; the input
does not move until you update the lock. To advance a nightly input, run
`nix flake update aix` and rebuild. To advance a stable input, first change the
tag in `flake.nix` to the new release, then update the lock and rebuild.

An unqualified URL such as `github:get-aix/aix` uses the repository's default
branch (currently `main`), not the latest versioned release. Specify a release
tag for stable installs and `/main` for nightly so the selected channel is
clear. The nightly flake builds from source; it does not install the prebuilt
binaries attached to the GitHub nightly release.

## home-manager module

Add the stable or nightly flake input shown above, import the module, and
declare your config declaratively. Nix installs the binary and generates
`~/.config/aix/aix.toml` at activation time.

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

## Upgrading

`aix upgrade` doesn't modify a Nix-installed aix. Your `flake.lock` pins the
exact commit, so Nix would undo any change at the next rebuild. Running it
prints these steps instead. `aix upgrade set` also leaves Nix inputs alone:
select stable or nightly by changing the ref in `flake.nix`.

To move a nightly input to the latest `main`, update the lock and rebuild:

```bash
nix flake update aix    # the name of the aix input in your flake.nix
nixos-rebuild switch --flake .#your-host
# or, standalone home-manager:
home-manager switch --flake .#your-user
```

For stable, change the input URL to the desired release tag before running
`nix flake update aix` and rebuilding. On Nix older than 2.19, use
`nix flake lock --update-input aix` instead.

For a `nix profile install`, run `nix profile upgrade aix` (`nix profile list`
shows the entry's name) to refresh its configured ref. A `main` ref moves with
nightly; a release-tag ref stays on that stable version. For a one-off run of
the latest nightly source, use `nix run --refresh github:get-aix/aix/main -- --help`.

## Option mapping

Module options mirror the config file with camelCase names. Each feature page
shows the Home Manager form next to the TOML:

| Config file | Home Manager option | Docs |
|---|---|---|
| `[models]`, `[profiles.<name>.models]` | `programs.aix.models`, `programs.aix.profiles.<name>.models` | [Model defaults and aliases](configuration.md#model-defaults-and-aliases) |
| `auth = { type = "chatgpt" }` | `programs.aix.profiles.<name>.auth = "chatgpt"` | [ChatGPT authentication](chatgpt.md) |
| `[prompts.<name>]` | `programs.aix.prompts` | [Reusable prompt presets](inference.md#reusable-prompt-presets) |
| `[tools.<name>]` | `programs.aix.tools.<name>` | [Named tool launch configuration](tools.md#named-tool-launch-configuration) |
| `[tools.<name>] local_gateway` | `programs.aix.tools.<name>.localGateway` | [Local gateway for API-key tools](tools.md#local-gateway-for-api-key-tools) |
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
