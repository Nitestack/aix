# aix Nix integration conventions

Apply this guidance to changes in `nix/**`, `flake.nix`, and `flake.lock`.

## Role of Nix in this project

Nix is responsible for:
- Building the Rust CLI (`packages.default`) and installing it into the user profile.
- Generating `aix.toml` in the XDG config directory from home-manager options.
- Passing secret *references* (files, env vars, commands) through to the config. The secrets themselves come from whatever the user already runs (sops-nix, agenix, `pass`, ...); the module does not depend on any of them.

Nix is **not** responsible for CLI business logic. If you find logic in a Nix file that belongs in Rust, flag it rather than expanding it.

## Module structure

`nix/home-manager.nix` exposes `programs.aix.*`:
- `enable`, `package`
- `defaultProfile` — optional default profile name
- `endpoint` — `baseUrl`, `gateway`, `provider` (metadata only)
- `profiles.<name>` — `label`, `auth` (`api_key` or `chatgpt`), `apiKey`, `baseUrl`, `env`, `models`, `toolConfigs` (`codex.configDir`, `opencode.configFile`/`cliConfigFile`, `pi.agentDir`)
- `models` — shared default model and aliases
- `tools.<name>` — `command`, `apiFormat`, `localGateway`, `env`, `chatgpt` (`transport`, `accessTokenEnv`, `prependArgs`, `clearEnv`)
- `prompts.<name>` — `prompt`, `system`, `model`
- `runPolicies.<name>` — `profile`, `maxDuration`, `allowedModels`, `tags`
- `cache` — local response-cache settings

The option definitions in `nix/home-manager.nix` are the source of truth; keep this list and `docs/nix.md` in step with them.

Secret-valued fields (including `baseUrl`, `apiKey`, profile labels/env, and tool env) accept a `secretSourceType`: a plain string (direct), `{ env = "VAR"; }`, `{ file = "/run/secrets/..."; }`, or `{ command = "..."; }`.

Do not add options that duplicate CLI flags. Options are for stable deployment config, not ad-hoc overrides.

## Config generation

The module renders `aix.toml` under `xdg.configFile` using `pkgs.formats.toml.generate`.
Every rendering change needs a check in `tests/home-manager-*.nix`, wired into `checks` in `flake.nix` (run `nix flake check`).
camelCase Nix option names are transformed to snake_case TOML keys (e.g. `endpoint.baseUrl` → `endpoint.base_url`, `tools.review.apiFormat` → `tools.review.api_format`).
Secret source values (`{ env = "VAR"; }`, `{ file = "/path"; }`, `{ command = "..."; }`) pass through as TOML subtables.

## Secret handling

Secret values must never appear in the Nix store (world-readable). Use a runtime
secret source such as `{ file = "/run/secrets/aix/key"; }`, `{ env = "VAR"; }`,
or `{ command = "secret-cli read ..."; }`. The CLI resolves these from its
generated config; there is no standalone `--secret-cmd` flag.

## Formatting

Run `nixfmt` (`nixfmt-rfc-style`) on any Nix file before committing:
```sh
nixfmt nix/home-manager.nix
```

## What NOT to change here

- Do not add new flake inputs to `flake.nix` without user approval.
