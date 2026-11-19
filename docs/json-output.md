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

`schema_version` is an integer that starts at `1`; `command` is the canonical command name. `current.data` contains `name`, `label`, and `source` (`env`, `default`, or `none`); without a selection, `name` and `label` are `null`. `profiles.data` contains profile names and labels only. `spend.data` preserves the original spend/budget summary and adds the full budget object; `status.data.spend` holds the same budget object (see [Budget fields](#budget-fields)). `models.data` contains sorted `{ "id" }` entries and the requested `filter` (or `null` when no filter was used). `status.data` also includes profile and gateway-probe information. `doctor.data.checks` contains ordered check objects with `name`, `status`, `message`, and `duration_ms`; failed doctor reports go to stderr so automation can inspect the findings there. Cache source is `live`, `cache`, or `none`. `usage.data` contains a local date range, observed request/token metrics, daily and attribution breakdowns, and billing status as described in [Local usage](gateway.md#local-usage). `prompt --list` returns sorted `{ "name", "model", "has_system" }` entries without preset bodies; prompt execution returns the same resolved `model`, assistant `content`, and `usage` fields as `ask`. `runs.data` contains non-secret records only; `runs show` returns one complete record. These commands do not expose credentials, prompts, or raw upstream inference responses.

## Budget fields

`spend.data` keeps its original `{ "spend": 41.53, "max_budget": 100.0 }`
summary; `max_budget` is omitted when unlimited. The full budget object is
added beside those two fields, so a caller can read the budget without the
gateway probe that `status` performs. `status.data.spend` holds the same budget
object without the summary, profile, fingerprint, or gateway fields.

```json
{
  "schema_version": 1,
  "command": "spend",
  "data": {
    "spend": 159.04,
    "max_budget": 175.0,
    "status": "available",
    "key_spent_usd": 159.04,
    "remaining_usd": 15.96,
    "limited_by": "key",
    "layers": [
      {
        "scope": "key",
        "budget_usd": 175.0,
        "spent_usd": 159.04,
        "remaining_usd": 15.96,
        "period_start": "2026-11-01T00:00:00Z",
        "resets_at": "2026-12-01T00:00:00Z",
        "budget_duration": "1mo",
        "reset_source": "inferred_monthly"
      }
    ],
    "expires_at": null,
    "project_id": "proj-123",
    "cache": { "source": "cache", "age_seconds": 42, "fetched_at": "2026-11-19T14:29:40Z" },
    "profile": { "name": "work", "label": "Work" },
    "key_fingerprint": "9c1e0d6f2b7a4e58a3d0c6b1f4e2a7d95b8c3f0e1a6d2b4c7e9f0a3b5d8c1e2f",
    "gateway": { "gateway": "aihub", "provider": null, "detected_from": "response" }
  }
}
```

- `status` is `available`, `unsupported` (the gateway has no budget endpoint;
  exit code `0`), or `unavailable` (`status` only, when the budget lookup failed
  but the probe succeeded). When it is not `available`, the budget fields are
  `null`, `cache.source` is `none`, and `spend` and `max_budget` are absent.
- `layers[].resets_at` is when the layer's window resets (RFC 3339, UTC), and
  `budget_duration` is the window length (`7d`, `30d`, `1mo`, …). `reset_source`
  is `gateway` when the gateway reported the reset or the duration, or
  `inferred_monthly` when the gateway only reported a period start on the first
  of a month at midnight UTC and aix assumes a calendar-month window. All three
  are `null` when neither applies.
- `project_id` is the gateway's project identifier, or `null`.
- `cache.source` is `live`, `cache`, or `none`. `age_seconds` is `null` for live
  reads, and `fetched_at` is when the gateway answered.
- `key_fingerprint` (also in `status.data`) is a stable, non-secret identifier
  for the endpoint and key: the hex SHA-256 of a domain-separated encoding of
  the base URL and API key. Use it to tell keys apart or to key a caller-side
  cache; it cannot be reversed into the key.
- `gateway.gateway` and `gateway.provider` (also in `status.data.gateway`) come
  from the profile's `gateway`/`provider` config first, then from the gateway
  kind the budget endpoint reported, then from well-known hosts
  (`api.openai.com`, `api.anthropic.com`). `detected_from` is `config`,
  `response`, `base_url`, or `null`.

Gateway-reported budget exhaustion uses exit code `6`; failed JSON commands
leave stdout empty and report the error on stderr.

## Exit codes

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
