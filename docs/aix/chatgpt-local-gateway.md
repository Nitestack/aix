# ChatGPT Responses local gateway

Compatible ChatGPT-authenticated tools can opt into an aix-owned local Responses
gateway instead of receiving the ChatGPT access token directly. The first
supported integration is Codex app-server over stdio.

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

With `transport = "local_gateway"`, aix adds process-local Codex `-c`
overrides for the dynamic loopback URL and a custom Responses provider. The
configured `access_token_env` carries only a random per-launch local bearer.
Aix obtains or refreshes the real OAuth access token for each upstream
inference request and sends it only to `https://api.openai.com/v1`. No Codex
login is needed, and aix does not write Codex's `config.toml`, `auth.json`, or a
separate `CODEX_HOME`.

Codex requests use the shared SIWC compatibility adapter and local usage event
store. The local counters describe observed requests and tokens; they are not
ChatGPT plan billing or USD cost estimates. For account-level plan usage, use
ChatGPT **Settings → Usage**.

## Codex compatibility check

The Codex CLI config schema and `codex app-server --listen stdio:// -c ...`
override syntax were checked with **Codex CLI 0.157.0**. The CLI-only check
confirmed the app-server mode and one-shot overrides parse without modifying
the existing Codex config or auth files. The live inference, local-tool, and
token-renewal smoke test remains **pending**: this environment has only an
expired aix test-profile auth record. A Codex-owned login is not a substitute
for an eligible aix ChatGPT profile, so no live request was attempted.

## Reproducible live smoke test

Use an eligible ChatGPT profile already signed in through aix. Configure a
no-budget run policy that allows the exact model to test; for example:

Replace `MODEL_ID` below with an exact model ID enabled for that account.

```toml
[run_policies.codex-smoke]
profile = "personal"
max_duration = "5m"
allowed_models = ["MODEL_ID"]
```

Keep the Codex binding above, including `command = "codex"`,
`access_token_env = "ACCESS_TOKEN"`, and clearing `OPENAI_API_KEY` and
`CODEX_API_KEY`. Then run:

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

To verify refresh, run the same command with `--require-refresh` when the aix
access token is naturally within its refresh window (or expired). The script
compares the redacted `aix auth status` expiry before and after the turn and
requires it to advance. Refresh is not forced by editing or copying the user's
auth record.

Tools other than OpenCode and Codex app-server remain on direct token handoff
unless a future adapter explicitly supports the local transport. Omitting
`transport` preserves existing behavior.
