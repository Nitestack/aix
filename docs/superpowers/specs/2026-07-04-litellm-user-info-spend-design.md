# LiteLLM user info and spend commands

**Date:** 2026-07-04
**Status:** Approved

## Summary

Add two top-level commands — `aix info` and `aix spend` — that query LiteLLM's
management API using the active profile's credentials. Both produce
human-readable output by default and raw JSON with `--json`.

## Commands

```
aix info  [profile] [--json]
aix spend [profile] [--json] [--limit N]
```

Both accept the global `--profile` / `-p` flag or a positional profile name,
consistent with `aix env`, `aix shell`, and `aix exec`.

`--limit N` on `aix spend` caps the number of log entries returned (default 50).
LiteLLM's `/spend/logs` can be large; a default cap keeps output readable without
requiring pagination logic in v1.

## Architecture

### New dependency: `reqwest` + `tokio`

`reqwest` provides async HTTP; `tokio` is the async runtime. `main()` gains
`#[tokio::main]`. Existing sync commands are unaffected — tokio does not require
everything to be async.

No OpenAI SDK (e.g. `async-openai`) is used: the target endpoints (`/user/info`,
`/spend/logs`) are LiteLLM-specific and outside the OpenAI spec. Raw reqwest is
sufficient and avoids unnecessary indirection.

### `src/client.rs`

A `LiteLlmClient` struct (already referenced in the Rust conventions but not yet
created):

```rust
pub struct LiteLlmClient {
    base_url: String,
    api_key: String,
    inner: reqwest::Client,
}
```

Two async methods:

| Method | Endpoint | Notes |
|---|---|---|
| `user_info()` | `GET /user/info` | No query params; returns the caller's own row |
| `spend_logs(limit)` | `GET /spend/logs` | `?limit=N`; returns per-request log entries |

Both return `serde_json::Value`. Typed structs are deferred — LiteLLM's response
shape is not versioned and may vary across deployments.

### `src/commands/info.rs`

1. Resolve profile + secrets (same pattern as existing commands)
2. Assert `endpoint.gateway == litellm`; error otherwise
3. Call `client.user_info().await`
4. Format and print

Human-readable output fields (best-effort; missing fields are silently skipped):

- User ID
- Teams
- Total spend
- Budget limit / remaining
- Number of keys

### `src/commands/spend.rs`

1. Resolve profile + secrets
2. Assert `endpoint.gateway == litellm`
3. Call `client.spend_logs(limit).await`
4. Format and print

Human-readable output per log entry:

- Timestamp
- Model
- Cost
- Request ID (truncated)

### `src/cli.rs`

Two new `Command` variants:

```rust
/// Show user info and budget for the selected profile (LiteLLM only)
Info {
    profile: Option<String>,
    #[arg(long)]
    json: bool,
},
/// Show recent spend logs for the selected profile (LiteLLM only)
Spend {
    profile: Option<String>,
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = "50")]
    limit: u32,
},
```

## Data flow

```
aix info work
  └─ config::load + validate
  └─ resolve base_url (SecretSource)
  └─ resolve api_key (SecretSource)
  └─ check gateway == litellm
  └─ LiteLlmClient::new(base_url, api_key)
  └─ client.user_info().await
  └─ format_user_info(value) → stdout
```

## Error handling

| Situation | Behaviour |
|---|---|
| `gateway` is not `litellm` | `AixError::NotLiteLlm` — clear message before any HTTP call |
| Non-200 HTTP response | `AixError::GatewayError { status, body_snippet }` |
| Network / TLS error | `AixError::HttpError(reqwest::Error)` |
| Secret resolution failure | Existing `AixError::SecretMissing*` variants |
| Field absent in response | Silently skipped in human output; preserved in `--json` |

## Testing

- **Unit:** `LiteLlmClient` methods tested against a local mock server (`wiremock`
  or `mockito`). No live LiteLLM endpoint required in CI.
- **Integration:** `assert_cmd`-based tests in `tests/` covering:
  - Wrong gateway → correct error message
  - `--json` flag passes raw response through unchanged
  - `--limit` is forwarded as a query parameter

## Out of scope

- Pagination of `/spend/logs` beyond `--limit`
- `/spend/keys`, `/spend/users`, or `/global/spend/report` endpoints
- Typed response structs (deferred until LiteLLM response shapes are confirmed stable)
- Non-LiteLLM gateways
