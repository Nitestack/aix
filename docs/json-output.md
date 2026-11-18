# JSON output and exit codes

The global `--json` flag is currently supported by `current`, `profiles`, `policies`, `policy`, `spend`, `models`, `status`, `doctor`, `gate`, `usage`, `ask`, `prompt`, `runs`, and `auth status`. It can appear before or after the command. Existing JSON forms remain supported. Other commands with their own output options, such as `aix env --format json`, keep those existing formats; using the global `--json` flag with an unsupported command is a validation error.

JSON mode writes exactly one JSON document to stdout on success. Errors and diagnostics go to stderr, and a failed JSON command leaves stdout empty; `doctor` sends its failed-check report to stderr so callers can inspect the findings there. The envelope is owned by `aix`:

```json
{
  "schema_version": 1,
  "command": "profiles",
  "data": [
    { "name": "work", "label": "Work" }
  ]
}
```

`schema_version` is an integer that starts at `1`; `command` is the canonical command name. `current.data` contains `name`, `label`, and `source` (`env`, `default`, or `none`); without a selection, `name` and `label` are `null`. `profiles.data` contains profile names and labels only. `spend.data` preserves the original spend/budget summary, mapped from the gateway's native `/v1/key/info` endpoint, or `{ "status": "unsupported" }` when the gateway does not implement that endpoint. `status.data.spend` exposes the native budget fields with cache metadata. `models.data` contains sorted `{ "id" }` entries and the requested `filter` (or `null` when no filter was used). `status.data` also includes profile and gateway-probe information. `doctor.data.checks` contains ordered check objects with `name`, `status`, `message`, and `duration_ms`; failed doctor reports go to stderr so automation can inspect the findings there. Cache source is `live`, `cache`, or `none`. `usage.data` contains a local date range, observed request/token metrics, daily and attribution breakdowns, and billing status as described in [Local usage](gateway.md#local-usage). `prompt --list` returns sorted `{ "name", "model", "has_system" }` entries without preset bodies; prompt execution returns the same resolved `model`, assistant `content`, and `usage` fields as `ask`. `runs.data` contains non-secret records only; `runs show` returns one complete record. These commands do not expose credentials, prompts, or raw upstream inference responses.

`spend.data` keeps its original `{ "spend": 41.53, "max_budget": 100.0 }`
shape; `max_budget` is omitted when unlimited, and a gateway without the native
endpoint reports `{ "status": "unsupported" }` with exit code `0`.
`status.data.spend` retains the
native key spend, effective remaining, budget layers, expiry, and cache details.
Gateway-reported budget exhaustion uses exit code `6`; failed JSON commands
leave stdout empty and report the error on stderr.

Process exit codes are stable:

| Code | Meaning |
|---:|---|
| `0` | Success |
| `1` | Uncategorized or internal failure |
| `2` | CLI or configuration validation failure |
| `3` | Secret resolution failure |
| `4` | Authentication or authorization failure |
| `5` | Network or gateway failure |
| `6` | Gateway-reported budget or policy failure |
