# Codex local gateway: live smoke test

Manual check for the Codex app-server ChatGPT `local_gateway` transport (see
[Running AI tools](../../docs/tools.md#codex-app-server-with-chatgpt-plan-authentication)).
It needs a real, eligible ChatGPT account, so it is not part of CI.

## Setup

Use an eligible ChatGPT profile already signed in through aix, with the Codex
binding from the user docs:

```toml
[profiles.personal]
auth = { type = "chatgpt" }

[tools.codex]
command = "codex"
api_format = "openai"

[tools.codex.chatgpt]
transport = "local_gateway"
access_token_env = "ACCESS_TOKEN"
prepend_args = ["app-server", "--listen", "stdio://"]
clear_env = ["OPENAI_API_KEY", "CODEX_API_KEY"]
```

Add a run policy that allows the exact model and bounds the run duration.
Replace `MODEL_ID` with a model ID enabled for that account:

```toml
[run_policies.codex-smoke]
profile = "personal"
max_duration = "5m"
allowed_models = ["MODEL_ID"]
```

## Run

```sh
python3 tests/manual/codex_local_gateway_smoke.py \
  --config ~/.config/aix/aix.toml \
  --profile personal \
  --policy codex-smoke \
  --model MODEL_ID
```

The script uses a temporary empty `CODEX_HOME`, wraps the Codex executable to
check that its bearer is the 64-character local token and that standard API
credentials are absent, then speaks the app-server protocol to request
inference and a dynamic local function call. It places a temporary recording
HTTPS proxy in the environment. The proxy records hostnames only and rejects
unexpected destinations **for connections that honor proxy environment
variables**; it never records request contents or tokens. If the proxy does not
observe `api.openai.com:443`, the script reports `not_observed` rather than
claiming that all external traffic was restricted. This is observation, not a
network sandbox: direct sockets can bypass an HTTP proxy.
The script also requires a successful `openai_responses` usage event carrying
the selected model, run ID, and policy attribution. It prints a short
pass/fail summary and does not save auth material in the repository.

## Token refresh

To verify refresh, run the same command with `--require-refresh` when the aix
access token is naturally within its refresh window (or expired). The script
compares the redacted `aix auth status` expiry before and after the turn and
requires it to advance. Refresh is not forced by editing or copying the user's
auth record.

Last checked with Codex CLI 0.157.0 (config and `app-server -c` override syntax only).
