# Codex configuration replacement feasibility (issue #31)

**Result: blocked; this proof does not authorize a production change.** Codex
has a user-configuration replacement selector, but that selector also changes
the home for persistent state. Native `--profile` is only an overlay and does
not meet the replacement contract. A staging-and-shared-state workaround is
plausible, but this investigation does not prove its safety for existing
sessions, all credential stores, concurrent launches, or supported platforms.
Keep issue #31 open for a product decision.

## Tested scope

- Codex CLI: `codex-cli 0.157.0` (local executable).
- Host: NixOS under WSL2, `x86_64-linux`.
- Date: 2026-10-06.
- The probe uses temporary Codex homes and synthetic markers. It does not read,
  copy, or modify the operator's real `~/.codex` or any real credentials.
- Reproduce the local behavior with
  [`tests/manual/codex_config_home_probe.sh`](../../tests/manual/codex_config_home_probe.sh).

This is one tested CLI version on one platform, not a compatibility claim or
support matrix.

## Findings

### Replacement works; native profiles do not replace

The tested invocation is:

```sh
CODEX_HOME=/path/to/existing/codex-home codex ...
```

`CODEX_HOME` must name an existing directory. Codex reads that home's
`config.toml` instead of the normal `$HOME/.codex/config.toml`. The probe puts
conflicting `features.multi_agent` values in the actual default home and the
selected home, then observes each value. It makes only the default home's TOML
invalid: Codex still loads the selected home, while a run with `CODEX_HOME`
unset fails on `$HOME/.codex/config.toml`. This demonstrates replacement
rather than a merge.

By contrast, `codex --profile NAME` / `codex exec --profile NAME` layers
`$CODEX_HOME/NAME.config.toml` over `$CODEX_HOME/config.toml`. The probe makes
the base file invalid and confirms that `codex exec --profile NAME` still
fails on the base file. This is not an alternate source directory and does not
satisfy issue #31. `--config` is a one-off `key=value` override, not a path to a
config file. Codex 0.157.0 exposes no separate config-only file/directory flag
in its CLI help.

Codex's ordinary project, built-in, system, and managed layers remain Codex
behavior, not aix behavior. Its documented precedence keeps trusted project
configuration, system defaults, built-ins, and administrator-enforced
requirements separate from the user home. A `CODEX_HOME` change selects the
user home; it does not authorize aix to suppress native project rules or
mandatory policy.

### State preservation is the blocker

OpenAI documents `CODEX_HOME` as the root for config, auth, logs, sessions,
skills, and other state. `history.jsonl` and session transcripts live there by
default. Codex 0.157.0 also has a separate SQLite state location:
`sqlite_home` in config takes precedence over `CODEX_SQLITE_HOME`, which takes
precedence over the selected `CODEX_HOME` default. Thus changing only
`CODEX_HOME` can make the old home's file-backed auth, history, sessions, and
SQLite state disappear from the selected view.

Credential storage modes have different preservation behavior:

| Mode | Native location | Implication when selecting a different home |
|---|---|---|
| `file` | `$CODEX_HOME/auth.json` | A different home does not see the saved file unless state is deliberately shared. |
| `keyring` | Operating-system credential store | Not stored under `CODEX_HOME`; availability and identity depend on the OS backend and selected mode. No real keyring was inspected or tested. |
| `auto` | Keyring when available, otherwise file | The fallback may be home-local `auth.json`; sharing only the keyring case is insufficient. |
| `ephemeral` | Current process memory | There is no persisted login to preserve. |

The probe confirms the file path with a dummy API-key credential in a disposable
home: an empty home reports “Not logged in,” while another home can see the
synthetic record through an `auth.json` symlink. This proves only the local file
path. It is **not** a ChatGPT login, a valid saved-application-auth test, or a
proof that session history can be resumed through the same arrangement.

Codex's native selectors therefore provide no config-only replacement while
keeping the rest of the original home implicitly shared. A staging home could
symlink or otherwise share state and pin `CODEX_SQLITE_HOME`/`sqlite_home`, but
that would be a separate mechanism requiring proof for the complete state tree,
auth modes, lock/concurrency behavior, and cleanup. This issue does not authorize
inventing isolated per-profile state stores, and that proof was not completed.

### Relative resource paths

The probe sets `model_catalog_json = "catalog.json"` in the selected home and
places a different catalog in the working directory. Codex reports the
malformed selected-home catalog by its absolute path, demonstrating that this
resource is resolved from the selected `CODEX_HOME`, not the process working
directory. No source config is rewritten. Relative path rules are setting- and
layer-specific; for example, Codex documents project `.codex/config.toml`
references relative to their containing `.codex` directory. This direct-home
test does not prove path behavior after copying or symlinking a config into a
staging home. Such staging could change a relative path's base and needs its own
test for every resource type aix intends to support.

### aix connection precedence

The existing Codex-specific aix integration is ChatGPT-authenticated Codex
app-server using the local Responses gateway. It creates a process-local
provider and loopback base URL, then appends aix's `-c` provider overrides
after caller arguments. The existing test
`codex_local_gateway_receives_only_a_per_launch_bearer_and_dynamic_provider_config`
in `tests/cli_chatgpt_launch.rs` verifies that a conflicting caller-supplied
provider URL appears before aix's loopback URL and that Codex config/auth/session
files are unchanged. This is an argv/precedence proof using a fake executable,
not a live inference test with the selected alternate home. It does not
demonstrate Codex's effective provider selection against a conflicting value
in a native `config.toml`; that part of the acceptance criterion remains open.

Generic API-key `aix codex` launching is not the same Codex-specific adapter:
it injects the generic `OPENAI_*` variables, but the Codex public environment
variable list does not document `OPENAI_BASE_URL` as a Codex selector. This
investigation cannot claim that aix controls the connection for that generic
mode. A future implementation must either define the supported mode precisely
or add and test the appropriate Codex-specific override; that is outside this
feasibility-only issue.

### Selector/argument handling and deployment limits

- `CODEX_HOME` is an inherited environment selector and can be set for a child
  process. An aix-managed value would need to take precedence over the
  inherited value.
- `--profile` selects an overlay under the effective `CODEX_HOME`; it is not a
  source-directory argument. Whether aix should allow such overlays on top of
  its selected native home or reject them as contradictory remains a product
  decision.
- Ordinary `--config key=value` overrides can coexist with a selected home,
  but provider overrides must remain later/higher precedence than values that
  could redirect the connection.
- `CODEX_HOME` must already exist and is a state root. Pointing it directly at
  a read-only generated directory is not proven usable; all Codex writes would
  need a writable home or separately redirected storage. A writable staging
  home may be possible, but its relative resources and shared state have not
  been validated.
- No macOS, Windows, plain-Linux, keyring, live session-resume, real history,
  or concurrent shared-state test was run. The probe's separate-home config
  checks are not evidence of concurrency safety for shared sessions or SQLite.

## Acceptance summary

| Issue criterion | Result |
|---|---|
| Tested version, sources, invocation, reproducible checks | Recorded for `codex-cli 0.157.0` on this Linux host. |
| Replace global user config without leakage; preserve native Codex layers | User-config replacement is tested; project/system/managed precedence is sourced from Codex docs, not emulated. |
| Distinguish replacement from native profile overlay | Proven: `--profile` layers on the selected base config. |
| Preserve sessions, history, and saved auth across credential modes | **Blocked:** no supported config-only selector; only synthetic file-auth path was probed. |
| aix connection wins over chosen native config | Partial: current ChatGPT app-server argv ordering is tested; effective native-config selection, generic API-key Codex mode, and live alternate-home request remain unproven. |
| Relative resource paths and read-only source behavior | Direct-home `model_catalog_json` path is tested; staged-home references and read-only generated configs are unproven. |
| Concurrent launches without crossover or shared-state loss | Separate read-only config loads are tested concurrently; shared Codex-state concurrency is unproven. |
| Inherited/configured selectors and explicit source arguments | CLI selectors are inventoried; aix rejection/precedence behavior remains a product/implementation decision. |
| Supported versions/platforms | Only the tested version and host are recorded; no support matrix is established. |
| If unavailable, report blocker and return for decision | **Done:** gate remains blocked; decision requested below. |

## Reproducible evidence and sources

Run the probe with the exact version used for this report:

```sh
sh tests/manual/codex_config_home_probe.sh
```

Expected result: three `PASS` lines for home replacement/profile layering,
selected-home relative paths/file-auth path, and concurrent read-only config
selection. A `LIMIT` line states that real ChatGPT auth, sessions, history,
keyring, and shared-state concurrency are not proven. Temporary files are
removed on exit.

Authoritative references checked for this report:

- [Codex configuration basics: precedence and project config](https://developers.openai.com/codex/config-file/config-basic/)
- [Codex advanced config: profiles, state locations, and history](https://developers.openai.com/codex/config-file/config-advanced/)
- [Codex environment variables: `CODEX_HOME` and `CODEX_SQLITE_HOME`](https://developers.openai.com/codex/config-file/environment-variables/)
- [Codex authentication: credential storage modes](https://developers.openai.com/codex/auth/)
- [Codex configuration reference: `sqlite_home`, model catalog, and auth settings](https://developers.openai.com/codex/config-file/config-reference/)
- [Codex 0.157.0 source: home and SQLite path resolution](https://github.com/openai/codex/blob/rust-v0.157.0/codex-rs/core/src/config/mod.rs#L4037-L4049)
- [Codex 0.157.0 source: `CODEX_HOME` must exist when explicitly set](https://github.com/openai/codex/blob/rust-v0.157.0/codex-rs/core/src/config/mod.rs#L4869-L4878)
- Local `codex --help`, `codex exec --help`, and `codex app-server --help` from `codex-cli 0.157.0`.
- aix implementation and existing regression test: `src/commands/launch.rs`,
  `src/commands/launch/chatgpt.rs`, `tests/cli_chatgpt_launch.rs`.

## Product decision needed

Choose one before dependent implementation proceeds:

1. Accept a specified writable staging home with an explicitly shared Codex
   state layout, including the SQLite path, and fund the missing credential,
   history/session, concurrency, relative-resource, and platform tests; or
2. Require a native Codex config-only selector (or another supported Codex
   mechanism) that separates user config from persistent user state.

Also decide whether native `--profile` overlays are allowed inside an aix
selected home, and whether generic API-key Codex launch is in scope. Until those
choices and the preservation proof exist, this feasibility gate is not complete.
