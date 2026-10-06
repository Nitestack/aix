# OpenCode V2 profile config replacement proof (#33)

This is a feasibility proof only. It makes no production launch, schema, or
host-configuration changes.

## Boundary and reproduction

The proof is pinned to the installed `opencode v2.0.20` CLI and its exact
upstream tag, commit `84c9be93a56304a108f1a22df0c5d62c26d5b6ca`. It does not
infer V2 behavior from V1 documentation. The script refuses to run with a
different CLI version.

On a Linux/XDG host with Python 3.10+ and OpenCode v2.0.20 available as
`opencode` (or set `OPENCODE_BIN` to its path):

```sh
mkdir -p /tmp/opencode
python3 tests/manual/opencode_v2_config_replacement.py
```

All OpenCode home, config, data, cache, and state paths are redirected into a
temporary directory under `/tmp/opencode`. The proof uses disposable native
config files, synthetic auth/session data, and a loopback-only fake model
endpoint. It does not read or contact an existing OpenCode profile or shared
server. The CLI test data is removed when the script exits.

## V2 mechanism established

`OPENCODE_CONFIG_DIR` selects the OpenCode global **config directory**. The
normal XDG data directory remains separate; the server builds its database
path from that data directory. This permits a private launch to select a
staged config tree while continuing to use the existing sessions, history,
credentials, and other app data.

The selector covers both native global config families:

- `opencode.json` / `opencode.jsonc` for application configuration.
- `cli.json` for terminal configuration.

There is one directory selector, not two independent V2 path flags. Independent
profile selection therefore requires a staged config directory: copy the
selected app config and its relative resources into it; copy the selected
`cli.json`; and copy the **unchanged standard counterpart** when that other
configuration family should retain its normal settings. The proof checks both
directions: profile A replaces app config while retaining the standard
`cli.json`; profile B replaces terminal config while retaining the standard
app config. A CLI command that reads and updates terminal config confirms it
used the selected staged `cli.json`, and a global-only CLI marker is absent.
That mutation check runs on an extra disposable clone, never on the staged
source.

Other app-config sources are additive, not alternate selectors:

- `OPENCODE_CONFIG_DIR` selects the global config root.
- `OPENCODE_CONFIG` adds an explicit config file.
- `OPENCODE_CONFIG_CONTENT` adds process-local config content; this is the
  current aix ChatGPT bridge mechanism.
- Project configs are still discovered from the working directory, and
  project instructions remain active.
- `OPENCODE_CLI_CONFIG_CONTENT` merges over `cli.json` rather than replacing
  it.

Consequently, an aix profile launch must own `OPENCODE_CONFIG_DIR` and reject
an explicitly conflicting `OPENCODE_CONFIG` source (or deliberately account
for it); merely setting the profile directory does not suppress that source.
The proof verifies the explicit file is loaded in addition to the profile
directory. The bridge content is intentionally additive: its provider and the
selected profile provider both remain present. `--server` and `--standalone`
are mutually exclusive and V2 rejects the combination; ordinary overrides
such as `--model` continue to work.

The private API-key launch shape established by the proof is:

```sh
OPENCODE_CONFIG_DIR="$STAGED_NATIVE_CONFIG" \
  opencode run --standalone --model "$PROFILE_PROVIDER/$MODEL" "..."
```

For the existing ChatGPT integration, aix additionally supplies its current
`OPENCODE_CONFIG_CONTENT` provider payload and the per-launch
`AIX_OPENCODE_BRIDGE_TOKEN`. These are additive runtime config/credentials;
the bridge config is not a replacement for staged user configuration. An
ordinary launch with no custom profile selector remains the unchanged V2 CLI
invocation and uses the regular managed-server path.

## Checks and results

The script makes these assertions against the installed V2 CLI:

| Check | Result |
| --- | --- |
| No-selector control reads the normal global app config | Pass |
| `OPENCODE_CONFIG_DIR` replaces global app values and beats an inherited config-directory selector | Pass |
| App and terminal config can be selected independently using native-format staged files | Pass |
| A global-only app value and global-only CLI setting do not leak into their respective replacements | Pass |
| Project `opencode.json` and `AGENTS.md` instructions still apply | Pass |
| Omitted `--agent` uses V2's normal default path after the global-only default agent is removed | Pass |
| Profile-relative `{file:...}` resource resolves from the staged native config file | Pass |
| API-key provider sends the selected key/model to the configured loopback endpoint; explicit `--model` wins | Pass |
| The current `aix-chatgpt` Responses provider shape sends its bridge bearer/model to the configured loopback endpoint | Pass |
| Bridge config adds its provider without dropping the selected profile provider | Pass |
| Private `--standalone` launches preserve a seeded session, user-message history, and saved application auth across a config switch | Pass |
| The dummy saved-auth key and dummy bridge bearer are absent from command output | Pass |
| Two concurrent private launches with different app configs have no config crossover | Pass |
| Standard, project, staged, and profile config sources remain byte-for-byte unchanged | Pass |
| `--server` plus `--standalone` is rejected; normal `--model` override is accepted | Pass |

No shared server is started, stopped, or restarted by the proof. The
no-selector control uses `--standalone` too, so it verifies the ordinary
global config source without risking a connection to or mutation of a host
service. No production launch behavior was changed; the regular managed-server
path remains the CLI's default when neither `--standalone` nor `--server` is
requested.

## Authoritative V2 sources

All links below are pinned to OpenCode tag `v2.0.20`:

- [`global-roots.ts`](https://github.com/anomalyco/opencode/blob/v2.0.20/packages/util/src/global-roots.ts) and [`global.ts`](https://github.com/anomalyco/opencode/blob/v2.0.20/packages/util/src/global.ts): XDG data/config roots are separate; the `OPENCODE_CONFIG_DIR` override changes only the config root.
- [`cli/index.ts`](https://github.com/anomalyco/opencode/blob/v2.0.20/packages/cli/src/index.ts) and [`cli/server-process.ts`](https://github.com/anomalyco/opencode/blob/v2.0.20/packages/cli/src/server-process.ts): server startup wires the config directory, explicit file/content sources, and database path separately.
- [`cli/config/config.ts`](https://github.com/anomalyco/opencode/blob/v2.0.20/packages/cli/src/config/config.ts): terminal config reads `<config-dir>/cli.json`; inline CLI content is merged over the file.
- [`cli/services/server-connection.ts`](https://github.com/anomalyco/opencode/blob/v2.0.20/packages/cli/src/services/server-connection.ts) and [`cli/services/standalone.ts`](https://github.com/anomalyco/opencode/blob/v2.0.20/packages/cli/src/services/standalone.ts): private-server selection, default managed-server path, and `--server`/`--standalone` conflict handling.
- [`core/config/discovery.ts`](https://github.com/anomalyco/opencode/blob/v2.0.20/packages/core/src/config/discovery.ts) and [`core/config.ts`](https://github.com/anomalyco/opencode/blob/v2.0.20/packages/core/src/config.ts): global, explicit, direct/project, and inline content source discovery and loading order.
- [`core/config/variable.ts`](https://github.com/anomalyco/opencode/blob/v2.0.20/packages/core/src/config/variable.ts): relative `{file:...}` references resolve from the selected config file's directory.
- [`cli/database-path.ts`](https://github.com/anomalyco/opencode/blob/v2.0.20/packages/cli/src/database-path.ts): the database path is rooted under the separately resolved data directory.
- [`schema/config.ts`](https://github.com/anomalyco/opencode/blob/v2.0.20/packages/schema/src/config.ts) and [`schema/config/provider.ts`](https://github.com/anomalyco/opencode/blob/v2.0.20/packages/schema/src/config/provider.ts): native application/provider configuration schema.

## Limits

This proves config selection and the API connections using fake local
endpoints; it does not perform live inference, a ChatGPT account login, or a
real SIWC token refresh. It does not implement profile staging or conflicting
environment-selector handling in aix. Those are production work and remain
outside issue #33. The supported evidence boundary is exactly OpenCode
v2.0.20; rerun the proof and re-check the pinned sources before claiming
compatibility with another V2 release.
