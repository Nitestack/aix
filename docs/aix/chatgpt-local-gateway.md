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
the existing Codex config or auth files. A live inference, local-tool, and
token-renewal smoke test still requires an eligible ChatGPT account and was
not run in this environment.

Tools other than OpenCode and Codex app-server remain on direct token handoff
unless a future adapter explicitly supports the local transport. Omitting
`transport` preserves existing behavior.
