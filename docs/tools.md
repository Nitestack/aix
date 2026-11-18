# Running AI tools

`aix <tool>` launches an AI coding tool or agent (Claude Code, Codex, OpenCode, Pi, aider, goose, or anything else on `$PATH`) with the selected profile's credentials in its environment. This page covers the launch syntax, `[tools.<name>]` configuration, which environment variables each launch receives, per-profile native tool configs, and the ChatGPT-plan integrations for Codex app-server and OpenCode.

`aix <tool>` works with any binary on `$PATH`; tools do not need to be configured.
If a matching `[tools.<name>]` entry exists, aix uses its executable, credential
format, and extra environment. Otherwise it preserves the legacy fallback:

- `aix claude ...` → Anthropic and LiteLLM variables
- `aix <anything else> ...` → OpenAI and LiteLLM variables

```bash
# aix <tool> [PROFILE] [--dry-run] [-- TOOL_ARGS...]

aix claude work -- --model claude-opus-4
aix opencode work -- --model gpt-4o
aix aider work -- --no-auto-commits
aix goose work -- session start
aix my-new-agent work -- --prompt "Hello"

# A configured logical name can launch a different executable.
aix review work -- --summary

# Interactive profile picker (when both stdin and stdout are TTYs)
aix claude -- chat

# Dry-run: print what would run, never print secrets
aix opencode work --dry-run
```

The `[tools]` map is otherwise generic launch wiring. ChatGPT-authenticated
OpenCode and Codex app-server entries can use their narrow SIWC bridge adapters
described in [Codex app-server](#codex-app-server-with-chatgpt-plan-authentication)
and [OpenCode v2](#opencode-v2-with-chatgpt-plan-authentication) below. Other tools do not get harness-specific adapters or workflows.

## Named tool launch configuration

An optional `[tools.<name>]` entry controls how a named tool is launched by
`aix <tool>` and `aix run -- <tool> ...`. The map key is the logical tool name;
`command` optionally selects a different executable, `api_format` selects
`anthropic`, `openai`, or `both`, and `env` adds tool-specific variables. Tool
environment values override profile values, which override generated
credentials for API-key profiles. A ChatGPT profile instead requires a
`chatgpt` binding. Other ChatGPT-bound tools receive the selected access token
in `access_token_env`; the special `opencode` binding instead starts an
ephemeral SIWC bridge and gives OpenCode only a random local bridge credential.
It never gives OpenCode the ChatGPT access, refresh, or ID token. Unconfigured
tools never receive a ChatGPT token.

For `aix <tool> ... --dry-run`, aix lists environment variable names without
resolving configured secret sources or refreshing ChatGPT credentials.

```toml
[tools.review]
command = "review-agent"
api_format = "openai"

[tools.review.env]
REVIEW_MODE = "review"
REVIEW_TOKEN = { env = "AIX_REVIEW_TOKEN" }
```

The equivalent Home Manager options use camelCase option names:

```nix
programs.aix.tools.review = {
  command = "review-agent";
  apiFormat = "openai";
  env = {
    REVIEW_MODE = "review";
    REVIEW_TOKEN = { env = "AIX_REVIEW_TOKEN"; };
  };
};
```

### Local gateway for API-key tools

Set `local_gateway = true` on a tool to route it through a gateway that aix
starts on `127.0.0.1` for that one launch and stops when the tool exits:

```toml
[tools.claude]
api_format = "anthropic"
local_gateway = true
```

```nix
programs.aix.tools.claude = {
  apiFormat = "anthropic";
  localGateway = true;
};
```

The tool receives a local `ANTHROPIC_BASE_URL` and/or `OPENAI_BASE_URL` (per
`api_format`) and a random per-launch token instead of the real API key. aix
forwards requests to the configured gateway with the real key. This gives you
two things:

- **Local usage tracking.** aix records request and token counts for each call
  (no prompts or responses) so `aix usage` can report them. See
  [Local usage](gateway.md#local-usage).
- **Model restrictions.** A run policy with `allowed_models` can only be
  enforced through a local gateway; see [Run policies](runs.md#run-policies).

The child process must be on `$PATH` and must honor the base-URL variables.
`--dry-run` shows the local-gateway wiring without starting anything.

## herdr pane labels

When aix launches a tool inside a herdr terminal pane (`HERDR_PANE_ID` is set
and stdout is a terminal), it labels the pane `<tool> · <profile>` and keeps
the label fresh while the tool runs. Nothing changes outside herdr.

## Environment variables emitted

For API-key profiles, `aix env` and `aix exec` always inject all seven variables.
These commands reject ChatGPT profiles instead of exporting OAuth credentials.

| Variable | Value |
|---|---|
| `AIX_PROFILE` | Selected profile name |
| `ANTHROPIC_API_KEY` | Resolved API key |
| `ANTHROPIC_BASE_URL` | Gateway base URL (bare) |
| `OPENAI_API_KEY` | Resolved API key |
| `OPENAI_BASE_URL` | Gateway base URL with `/v1` appended |
| `LITELLM_API_KEY` | Resolved API key |
| `LITELLM_BASE_URL` | Gateway base URL with `/v1` appended |

`LITELLM_API_KEY`/`LITELLM_BASE_URL` are aliases for the same credential and
gateway as `OPENAI_API_KEY`/`OPENAI_BASE_URL` — not a third distinct secret
— for tools that specifically look for a `LITELLM_*`-named variable (e.g.
LiteLLM-aware config formats). They are included for API-key launches,
regardless of tool name or selected API format.

Named-tool subcommands use a matching `[tools.<name>]` configuration when
present. Its `api_format` selects Anthropic, OpenAI, or both; otherwise the
legacy fallback is Anthropic for `claude` and OpenAI for other names. `aix run`
uses a matching tool entry too; an unconfigured API-key command receives both
formats. A ChatGPT profile requires an explicit `chatgpt` binding and receives
no generated API-key variables:

| Launch | Credential variables |
|---|---|
| Configured `aix <tool>` / `aix run -- <tool>` | Selected format from `api_format` |
| Unconfigured `aix claude ...` | Anthropic pair |
| Other unconfigured `aix <tool> ...` | OpenAI pair |
| Unconfigured `aix run -- CMD` | Both Anthropic and OpenAI pairs |
| Configured ChatGPT tool | Only the access token under `access_token_env`, plus configured profile/tool env |
| Unconfigured ChatGPT tool | Rejected; no token or API-key fallback |

`AIX_API_KEY` and `AIX_BASE_URL` are not generated by aix. Tools that previously
read those variables should switch to the `ANTHROPIC_*` or `OPENAI_*` equivalents.

## Per-profile native tool configuration

Some tools can use a different native configuration per aix profile. These settings live under `[profiles.<name>.tool_configs.<tool>]`.

### Profile-specific Codex configuration

Select a separate Codex home for launches using a named aix profile:

```toml
[profiles.work.tool_configs.codex]
config_dir = "~/.config/codex/work"
```

The directory must already exist and be accessible. Absolute paths and `~` are
supported; relative paths are resolved from the aix configuration file. If the
directory is unavailable, aix warns and launches Codex with its standard
configuration instead. The equivalent Home Manager option is
`programs.aix.profiles.work.toolConfigs.codex.configDir`.

This setting selects Codex's `CODEX_HOME`. That replaces the user configuration
directory, but `CODEX_HOME` also selects Codex's persistent state. The selected
home does not automatically share saved authentication, sessions, or history
from the standard Codex home. You may need to sign in again, and your existing
sessions and history will not automatically appear there. aix does not currently
share or migrate state between Codex homes. This behavior was checked with
`codex-cli 0.157.0`; other versions are unverified.

### Profile-specific Pi configuration

```toml
[profiles.work.tool_configs.pi]
agent_dir = "~/.config/pi/work-agent"

[tools.pi]
api_format = "openai"
```

This opt-in setting applies to `aix pi` and `aix run -- pi` with API-key
profiles. aix stages the selected native configuration without editing it,
preserves its default model, and shares Pi's existing auth, trust, sessions,
and package stores. Those stores are not isolated per profile. The equivalent
Home Manager option is `programs.aix.profiles.work.toolConfigs.pi.agentDir`.

To override saved native credentials, the launcher passes the selected key via
Pi's `--api-key` argument; it is visible to process-argument inspection. Native
provider switching remains available. Staging requires symlink support, which
may require Windows Developer Mode or privileges. See the
[Pi 1.0.4 proof and compatibility limits](../tests/manual/pi_agent_config_replacement.md).

### Profile-specific OpenCode configuration

For the logical `opencode` tool, a profile may select native OpenCode config files:

```toml
[profiles.work.tool_configs.opencode]
config_file = "~/.config/aix/opencode/work.json"
cli_config_file = "~/.config/aix/opencode/work-cli.json"
```

`config_file` is a native OpenCode application config file; the selected source
replaces global application config, while project config and instructions remain
native. `cli_config_file` is a native OpenCode `cli.json` file and replaces only
global terminal configuration. Absolute paths, `~/` paths, and paths relative to
the aix config file are supported. The equivalent Home Manager options are
`programs.aix.profiles.work.toolConfigs.opencode.configFile` and
`cliConfigFile`. The real-tool check is pinned to OpenCode v2.0.20; see the
[OpenCode profile config check](../tests/manual/aix_opencode_v2_profile_config.md).

## ChatGPT plan integrations

These integrations require a [ChatGPT-authenticated profile](chatgpt.md).

### Codex app-server with ChatGPT plan authentication

OpenAI documents ChatGPT-plan token sharing for Codex app-server using a custom
Responses provider. Opt into aix's local gateway to keep the real access token
inside aix and observe usage:

```toml
[tools.codex]
command = "codex"
api_format = "openai"

[tools.codex.chatgpt]
transport = "local_gateway"
access_token_env = "ACCESS_TOKEN"
prepend_args = ["app-server", "--listen", "stdio://"]
clear_env = ["OPENAI_API_KEY", "CODEX_API_KEY"]
```

The equivalent Home Manager settings use camelCase:

```nix
programs.aix.tools.codex = {
  command = "codex";
  apiFormat = "openai";
  chatgpt = {
    transport = "local_gateway";
    accessTokenEnv = "ACCESS_TOKEN";
    prependArgs = [ "app-server" "--listen" "stdio://" ];
    clearEnv = [ "OPENAI_API_KEY" "CODEX_API_KEY" ];
  };
};
```

The local-gateway transport injects one-shot Codex `-c` overrides for a unique
Responses provider, the ephemeral loopback base URL, and `env_key =
"ACCESS_TOKEN"`. That variable contains only the random local bearer; aix fetches
or refreshes the real access token for each request to the public OpenAI API.
No `codex login` is needed, and aix does not write Codex config, auth, or session
files or create a separate `CODEX_HOME`. Other ChatGPT-bound tools remain direct
by default; local-gateway mode is currently limited to Codex app-server over
stdio and the existing OpenCode adapter.

Dry-run displays `transport: local_gateway` and that a dynamic local Responses
provider/base URL will be used, but no actual URL or credential. Codex CLI
0.157.0 was checked for the `app-server --listen stdio:// -c ...` syntax. The
CLI-only check did not perform inference; a live inference/tool/refresh smoke
test still needs an eligible ChatGPT account. Local usage events report request
and token counts without inventing a USD cost; account-level plan usage remains
in ChatGPT **Settings → Usage**. See OpenAI's [Codex app-server guide](https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server)
and [preview limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)
for the current provider schema and SIWC request contract.

### OpenCode v2 with ChatGPT plan authentication

OpenCode v2 uses a process-local `aix-chatgpt` Responses provider rather than
its built-in `openai` provider. This keeps any saved OpenCode login untouched
and prevents it from overriding the selected aix profile. Configure an
effective default model for the profile and bind the logical `opencode` tool:

```toml
[profiles.personal]
auth = { type = "chatgpt" }

[profiles.personal.models]
default = "gpt-5-codex"

[tools.opencode]
command = "opencode"
api_format = "openai"

[tools.opencode.chatgpt]
access_token_env = "AIX_OPENCODE_BRIDGE_TOKEN"
```

Then run:

```bash
aix opencode personal
# Or select it for the shell first:
aix use personal
aix opencode
aix opencode personal --dry-run
```

The dry-run reports that the ephemeral bridge/provider would be used and lists
environment variable names only. It does not start a listener or read/refresh
ChatGPT tokens. On a real launch, aix starts a loopback-only bridge on an
ephemeral port, injects its process-local OpenCode config through
`OPENCODE_CONFIG_CONTENT`, and gives OpenCode only a random bearer credential
that is valid for that local bridge. The real ChatGPT access token stays in aix;
`AuthService` supplies a current token for each upstream request, including
after refresh. The only upstream destination is the public
`https://api.openai.com/v1` Responses API. OpenCode's config, credentials,
sessions, and other user state are not edited.

The bridge streams SSE, enforces `store: false` and `stream: true`, moves text
system input into `instructions` without dropping its content, and removes the
fields disallowed by the current [SIWC preview contract](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations).
Supported function/custom tools and their history are preserved. Unsupported
hosted Responses tools fail locally rather than being silently removed.
Only the profile's effective default model is configured; OpenCode model
catalog synchronization is not included.

The runtime provider shape is based on the OpenCode
[v2.0.22 provider configuration](https://opencode.ai/v2/docs/providers/). The
automated bridge tests use a fake upstream. A real-account, multi-turn live
smoke test against OpenCode v2.0.22 remains a manual release check; the host's
installed version may be checked with `opencode --version`.

No tool entries or harness adapters are preconfigured. API-key profiles retain
the legacy fallback (`claude` gets Anthropic variables; other names get OpenAI
variables), while an unconfigured `aix run` command receives both formats.
