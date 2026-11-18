# Command reference

Run `aix --help` or `aix <command> --help` for the full flag list of your
installed version. Most commands that need a profile accept it as an optional
positional argument; see [Global options](#global-options) for how a profile is selected.

| Command | What it does | Docs |
|---|---|---|
| `aix <tool> [PROFILE] [-- ARGS]` | Launch an AI tool (`claude`, `codex`, `opencode`, `pi`, ...) with profile credentials | [Running AI tools](tools.md) |
| `aix init <shell>` | Print the shell wrapper that makes `aix use` work | [Shell integration](shell-integration.md#profile-switching) |
| `aix use PROFILE` / `aix use --clear` | Select or clear the active shell profile | [Shell integration](shell-integration.md#profile-switching) |
| `aix current` | Show the effective profile without opening the picker | [Shell integration](shell-integration.md#profile-switching) |
| `aix profiles` | List configured profiles | [Listing profiles](#listing-profiles) |
| `aix env [PROFILE]` | Print credential variables for the current shell | [Shell integration](shell-integration.md#explicit-credential-export-with-aix-env) |
| `aix exec [PROFILE] -- CMD` | Run a command with profile credentials | [Shell integration](shell-integration.md#aix-exec--one-shot-command) |
| `aix shell [PROFILE]` | Open an interactive shell with profile credentials | [Shell integration](shell-integration.md#aix-shell--interactive-shell-session) |
| `aix run [...] -- CMD` | Run a command with credentials and local run history | [Managed runs](runs.md) |
| `aix runs` / `aix runs show RUN_ID` | Inspect run history | [Managed runs](runs.md) |
| `aix policies` / `aix policy show NAME` | List or inspect run policies | [Managed runs](runs.md#run-policies) |
| `aix gate --policy NAME` | Check whether a run policy can execute right now | [Managed runs](runs.md#run-policies) |
| `aix ask "..."` | Send a one-shot request to the gateway | [One-shot inference](inference.md) |
| `aix prompt NAME` / `aix prompt --list` | Run or list configured prompt presets | [Prompt presets](inference.md#reusable-prompt-presets) |
| `aix models [PROFILE]` | List model IDs from the gateway | [Model discovery](gateway.md#model-discovery) |
| `aix spend [PROFILE]` | Show current key spend and budget | [Spend and budget](gateway.md#spend-and-budget) |
| `aix status [PROFILE]` | Profile, gateway connectivity, and budget overview | [Gateway status](gateway.md#gateway-status) |
| `aix doctor [PROFILE]` | Read-only diagnostics for config, secrets, gateway, cache | [Diagnostics](gateway.md#diagnostics) |
| `aix usage [PROFILE]` | Locally observed request and token usage | [Local usage](gateway.md#local-usage) |
| `aix auth login\|status\|logout [PROFILE]` | Manage ChatGPT sign-in for a profile | [ChatGPT authentication](chatgpt.md) |
| `aix config path` / `aix config validate` | Print the config path or validate the config | [Configuration](configuration.md#config-path-resolution) |
| `aix cache clear` | Delete cached gateway responses | [Cache](gateway.md#clearing-the-cache) |
| `aix upgrade [--check] [--force]` | Upgrade aix the same way it was installed (alias: `aix update`) | [Upgrading](installation.md#upgrading) |

## Global options

`--profile` selects a profile and can be used before or after a command. Profile resolution is positional profile, then `--profile`/`AIX_PROFILE`, then `default_profile`; without `--non-interactive`, a TTY-only picker remains the final fallback.

`--non-interactive` disables the profile picker. If a command needs a profile and none resolves from its positional profile, `--profile`/`AIX_PROFILE`, or `default_profile`, it fails immediately with a selection error instead of prompting or waiting for stdin. This is useful for scripts and CI. Without this flag, existing TTY detection still controls whether the picker is available.

`--timeout DURATION` sets the maximum duration for each HTTP request made by `aix`. It accepts milliseconds, seconds, or minutes (for example, `750ms`, `5s`, or `2m`) and defaults to `30s`. It does not limit subprocesses started by `aix exec`, `aix run`, or a named tool.

Both flags are global and can appear before or after regular subcommands.

`--config <PATH>` selects the config file (see [Config path resolution](configuration.md#config-path-resolution)), and `--json` emits the stable JSON envelope for supported commands (see [JSON output and exit codes](json-output.md)).

## Listing profiles

```bash
# Human-readable list
aix profiles

# JSON envelope with { name, label } objects in data (no secrets)
aix profiles --json

# The global flag is also accepted before the command
aix --json profiles
```
