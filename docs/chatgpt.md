# ChatGPT authentication

A profile can authenticate with Sign in with ChatGPT instead of a gateway API key. aix stores the OAuth credentials in its own owner-only auth store and hands a tool only what its explicit binding allows.

## Configure a ChatGPT profile

ChatGPT authentication does not use `[endpoint]` or an API key. Add a profile
with the explicit `auth` object:

```toml
default_profile = "personal"

[profiles.personal]
label = "Personal ChatGPT"
auth = { type = "chatgpt" }
```

## Sign in, check status, sign out

Then manage the local sign-in:

```sh
aix auth login personal   # opens the system browser; requires an interactive session
aix auth status personal  # reads local state only; makes no network requests
aix auth logout personal  # revokes remotely when possible and always clears local tokens
```

The profile can also be declared in Home Manager as
`programs.aix.profiles.personal.auth = "chatgpt";`. Existing profiles with
`api_key` remain API-key profiles and need no migration.

## Which tools receive credentials

ChatGPT credentials are handed to a child only when the selected profile and
named tool have an explicit `[tools.<name>.chatgpt]` binding. The binding names
the one environment variable that receives the current short-lived access
token. `aix env`, `aix shell`, `aix exec`, `aix ask`, and unconfigured tools do
not receive OAuth credentials. `aix run -- <configured-tool>` uses the same
binding. Declarative run policies remain available for profile, duration, model,
and tag constraints; they do not create gateway keys or enforce spend caps.

The refresh token, retained ID token, and issued OAuth client ID stay in aix's
owner-only auth store; they are never passed to the child or written to run
history. `--dry-run` prints the executable, effective arguments, and variable
names, and does not start a gateway or refresh a token. Direct transport obtains
the access token before the child starts, so a long-lived direct-mode harness
must be restarted to receive a renewed token. The supported Codex app-server
local-gateway mode instead keeps a stable local bearer and obtains a current
upstream token for each inference request. This integration covers Codex
app-server only, not the Codex TUI or other Codex modes.

## Tool setup and storage

- [Codex app-server with ChatGPT plan authentication](tools.md#codex-app-server-with-chatgpt-plan-authentication)
- [OpenCode v2 with ChatGPT plan authentication](tools.md#opencode-v2-with-chatgpt-plan-authentication)
- [Named tool launch configuration](tools.md#named-tool-launch-configuration) for the `[tools.<name>.chatgpt]` binding
- [ChatGPT OAuth credentials](security.md#chatgpt-oauth-credentials): where the auth store lives and how it is protected
