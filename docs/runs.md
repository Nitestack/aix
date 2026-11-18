# Managed runs

Use `aix run` when a command needs a stable run ID and local, metadata-only
provenance. The child keeps normal stdin/stdout/stderr behavior; `aix exec`
remains the stateless wrapper and does not create run records.

```sh
aix --profile work run \
  --name "nightly review" \
  --workflow verification \
  --task-id task-42 \
  --tag nightly --tag ci \
  -- review --summary

# List the newest 20 runs, or request JSON for scripts.
aix runs
aix runs --json --limit 10

# Inspect one complete non-secret record.
aix runs show RUN_ID --json
```

When the command token matches `[tools.<name>]`, `run` uses that tool's launch
configuration. An unconfigured command receives both credential formats, as
with `aix exec`. The child receives `AIX_RUN_ID`, plus the optional
`AIX_RUN_NAME`, `AIX_WORKFLOW`, `AIX_TASK_ID`, and JSON-array `AIX_RUN_TAGS`.

Records are stored under the platform state directory; set `AIX_STATE_DIR` to
override it. Records include run metadata and lifecycle status, but never
command arguments, prompts, stdin/stdout/stderr, API keys, base URLs, or secret
environment values. Listing is local and does not contact the gateway. `aix`
does not manage worktrees, tasks, retries, or orchestration.

## Run policies

Declarative run policies continue to constrain the selected profile, maximum
duration, allowed models, and run tags. For example:

```toml
[run_policies.review]
profile = "work"
max_duration = "30m"
allowed_models = ["fast"]
tags = ["verification"]
```

Apply one with `aix run --policy review -- review-tool`. Spend limits are not a
run-policy field; old `max_budget` entries are rejected as unknown configuration
rather than silently ignored. Existing run-history files remain local and
available offline, including older records that contain lease metadata. An
upgrade does not contact the gateway or revoke keys exported by an older
version.

Allowed-model restrictions require a configured local-gateway tool capable of
enforcing them; otherwise the run fails before launch. `--duration` can shorten
the policy limit, `--allow-model` can narrow its model set, and `--dry-run`
validates the policy without launching the child. These flags require `--policy`.

Inspect configured policies, or preflight one against the gateway before launching:

```sh
aix policies               # list configured run policies
aix policy show review     # show the complete configuration for one policy
aix gate --policy review   # check whether the policy can execute right now
```

All three accept the global `--json` flag; see [JSON output and exit codes](json-output.md).
