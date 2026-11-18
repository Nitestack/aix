# Gateway commands

These commands inspect the gateway behind a profile, or what aix observed locally: `aix models`, `aix spend`, `aix status`, `aix doctor`, `aix usage`, and the `aix cache` that backs spend and status.

## Model discovery

`aix models` fetches model IDs live from the selected profile's OpenAI-compatible `/v1/models` endpoint. It does not cache the response or assume provider-specific metadata.

Every authenticated request also carries `anthropic-version: 2023-06-01`, so a
profile whose `base_url` points at a native Anthropic endpoint (for example
`https://api.anthropic.com`) can be listed too: Anthropic's `/v1/models`
requires that header, while OpenAI-compatible gateways ignore it. No
`provider`/`gateway` metadata value selects this behavior; the header is always
sent and inert where it is unknown.

```bash
# List model IDs for the default or selected profile
aix models work

# Filter IDs by a case-insensitive substring
aix models work --filter gpt

# Emit the stable JSON envelope for scripts
aix --json models work
```

Human output contains one sorted model ID per line. If no IDs match a filter, the command succeeds with empty output. Use `--filter <TEXT>` rather than a second positional argument so the profile position remains unambiguous.

## Spend and budget

`aix spend` fetches the selected key's current spend and budget information from
the gateway's native `GET /v1/key/info` endpoint and presents the values the
gateway reports. It does not create or select another key.

The output is unchanged: one spend/budget summary and one colored progress bar,
with two-decimal USD amounts and the existing cache-age suffix. For layered
budgets, the native `limited_by` scope selects the actual spent/limit pair;
aix never invents a maximum by adding key spend to a shared balance. If nothing
caps the key, it prints the original `spent (no budget set)` summary without a
bar. For example:

```text
$41.53 of $100.00  ·  $58.47 available (58%)
[█████████████████░░░░░░░░░░░░░░░░░░░░░░░░░]  42% used
```

```bash
# Show spend for the default or selected profile
aix spend work

# Original JSON summary: spend and an optional max_budget
aix spend work --json

# The global flag is also accepted before the command
aix --json spend work

# Always fetch fresh data (result is still cached for future calls)
aix spend work --no-cache
```

Responses are cached locally for 1 hour by default (configurable via `[cache]`).
Use `--no-cache` to bypass the cache for one call; the fresh response is still
written to cache for subsequent calls. The endpoint is a gateway capability and
does not require a `gateway = "litellm"` metadata value.

A gateway that does not implement the endpoint (`404`, `405`, or `501`) is a
missing capability, not a failure: `aix spend` prints
`Spend unsupported by this gateway` and exits `0`, matching what `aix status`
reports for the same endpoint. The unsupported result is not cached, and real
failures (authentication, server errors, timeouts, malformed payloads) still
exit non-zero. Native Anthropic endpoints, for example, have no `/v1/key/info`.

## Gateway status

`aix status` gives a one-screen overview of the selected profile, a live
authenticated gateway probe, and any available current-key budget information
from `GET /v1/key/info`:

```bash
# Use the default profile or select one explicitly
aix status
aix status work

# Refresh budget data instead of using the cache
aix status work --refresh

# Emit the shared JSON envelope
aix --json status work
```

The OpenAI-compatible `GET /v1/models` probe is always live, including when
budget data comes from cache. `--refresh` bypasses only the budget cache; a
successful probe reports its latency and authenticated state. The header
handling described under model discovery applies here as well, so native
Anthropic endpoints report `reachable · authenticated` with
`Spend unsupported`. The output shows configured gateway/provider metadata and
only the API key's final four characters (short keys are fully redacted). The
resolved gateway URL and full key are never printed. If native budget
information is unsupported or unavailable, status reports that separately
without hiding a successful connectivity/authentication result.

## Diagnostics

`aix doctor [PROFILE]` runs read-only checks for config discovery and validation, profile and secret resolution, gateway authentication and model discovery, native budget information, and cache-directory writability. It never opens the profile picker. If no profile is available from the positional argument, `--profile`/`AIX_PROFILE`, or `default_profile`, the profile check fails instead of prompting.

```bash
# Human-readable checks
aix doctor
aix doctor work

# Stable JSON report (global --json may appear before or after the command)
aix doctor work --json
aix --json doctor work
```

Each row has one of four statuses: `PASS` means the check succeeded; `FAIL` includes a remediation hint; `SKIP` means a prerequisite failed; and `N/A` means the check does not apply (for example `budget_info` on a gateway that answers `404` for `/v1/key/info`). The checks run in this order:

| Check | What it verifies |
|---|---|
| `config_discovery` | A config path can be found from `--config`, `AIX_CONFIG`, or the platform config directory. |
| `config_validation` | The config can be parsed and structurally validated, and its configured `env_files` can be loaded. |
| `profile_selection` | The requested/global/default profile exists; no interactive picker is used. |
| `api_key` | The selected profile's API-key source resolves to a non-empty value. The value is never displayed. |
| `base_url` | The profile or endpoint base-URL source resolves to a non-empty value. The value is never displayed. |
| `gateway_auth` | An authenticated OpenAI-compatible request to the gateway succeeds. |
| `model_discovery` | `/v1/models` returns a parseable model list with non-empty string IDs. |
| `budget_info` | The gateway's native `GET /v1/key/info` endpoint returns valid current-key budget information. |
| `cache_directory` | The configured/default cache directory can be created and a temporary write probe succeeds; existing data is not deleted. |

Checks that do not require the network have `duration_ms: null`; network checks report elapsed milliseconds. `doctor --json` uses the shared JSON envelope with an ordered `data.checks` array. Successful reports go to stdout; if any check fails, the report goes to stderr so failed commands leave stdout empty. Scripts can inspect failed checks from stderr. The process exits `0` only when every applicable check passes; otherwise it uses the exit category for the first failing layer (config `2`, secret `3`, authentication `4`, gateway/network `5`, cache/internal `1`, or budget/policy `6`). Skipped and not-applicable checks do not fail the command by themselves.

Neither mode prints API keys, resolved secret-backed URLs, secret-file contents, secret-command stdout, or raw upstream error bodies.

## Local usage

`aix usage` reports locally observed request and token activity for the selected
profile. It reads aix's local usage-event store only: it does not contact a
gateway, resolve API keys or gateway URLs, or fetch upstream history. For current
gateway-reported budget information, use `aix spend`.

aix only sees traffic that passes through it, so events are recorded for:

- API-key tools with [`local_gateway = true`](tools.md#local-gateway-for-api-key-tools)
- ChatGPT-plan tools that go through a local bridge: the `opencode` binding and
  Codex app-server with `transport = "local_gateway"` (see
  [ChatGPT plan integrations](tools.md#chatgpt-plan-integrations))

A tool that talks to the gateway directly (the default for API-key tools) is
not counted; use `aix spend` for those.

```bash
# Default: the last 30 local calendar dates, including today
aix usage work

# Last seven calendar dates, including today
aix usage work --since 7d

# Inclusive explicit date range
aix usage work --start 2026-09-01 --end 2026-09-30

# Filter observed events by model, protocol, tool, or managed run ID
aix usage work --model gpt-6-luna
aix usage work --protocol openai_responses
aix usage work --tool opencode
aix usage work --run RUN_ID

# Stable JSON envelope
aix usage work --json
```

`--since Nd` requires a positive whole number of days and cannot be combined
with `--start`/`--end`. Explicit dates must both be supplied as inclusive
`YYYY-MM-DD` dates, with the end on or after the start. Model, protocol, tool,
and run filters match exact IDs/names. The default range is the last 30 local
calendar dates, including today.

JSON output uses the shared envelope. `usage.data` contains the selected date
range, local request/token metrics, daily and model/tool/protocol/run
breakdowns, and billing status. A cost is shown only when observed events carry
an authoritative cost; unknown cost is reported as unavailable, and ChatGPT
plan usage is not treated as USD spend. The `model_filter` field is included
when `--model` is applied. Local reports contain no prompts, secret values, or
upstream response fields.

## Cache

`aix spend` and `aix status` cache native gateway budget information in the platform cache directory to avoid redundant requests. `aix status` still performs its gateway probe on every invocation:

| Platform | Default cache directory |
|----------|------------------------|
| Linux    | `~/.cache/aix/`        |
| macOS    | `~/Library/Caches/aix/` |
| Windows  | `%LOCALAPPDATA%\aix\cache\` |

Override the directory for all invocations:

```bash
export AIX_CACHE_DIR=/tmp/aix-cache
```

Cache entries are replaced atomically so an interrupted write does not corrupt the previous entry. On Unix, aix restricts cache directories to owner access and cache files to owner read/write access; other platforms use their available filesystem defaults.

### Cache configuration

Caching is enabled by default with a 1-hour TTL. Add an optional `[cache]` block to your config to tune behaviour:

```toml
[cache]
ttl_secs = 3600  # cached entries older than this are treated as stale (0 = never expire)
disabled = false # set to true to always fetch live data (equivalent to always passing --no-cache)
```

### Clearing the cache

```bash
aix cache clear
```

Deletes all cached response files and prints a count of removed files.
