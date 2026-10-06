# Pi agent-directory replacement proof

## Result

**Feasible with a small launch adapter; not implemented in production by this ticket.**
Pi can replace its complete user-level agent directory with the directory selected by
an aix profile while reusing the original session and authentication stores. The
adapter must stage an overlay for read-only/generated configuration and make the
selected aix connection win over Pi's saved and model-configured credentials.

The reproducible proof is [`pi_agent_config_replacement.py`](pi_agent_config_replacement.py).
It uses synthetic credentials and local mock OpenAI-compatible gateways; it does not
need a real model account or send requests to an external service.

## Tested versions and invocation

- Pi Coding Agent: `@earendil-works/pi-coding-agent` **1.0.4** (npm package).
- Node.js: **24.20.0** (Pi 1.0.4 requires Node.js >=22.19.0).
- aix: this worktree's `target/debug/aix`.
- Host: Linux x86_64. Other platforms are not claimed as tested.

From the repository root:

```sh
npm install --prefix /tmp/opencode/pi-agent-1.0.4 \
  --ignore-scripts --no-audit --no-fund --package-lock=false \
  @earendil-works/pi-coding-agent@1.0.4
nix develop --no-write-lock-file --command cargo build --locked
python3 tests/manual/pi_agent_config_replacement.py \
  --pi /tmp/opencode/pi-agent-1.0.4/node_modules/.bin/pi \
  --aix target/debug/aix
```

Expected summary:

```text
pi_version=1.0.4
config_replacement=settings+models+keybindings+resources=passed
project_rules_and_trust=passed
session_history_and_saved_auth=passed
aix_run_named_tool_exec_and_concurrent_profiles=passed
read_only_sources_and_relative_resources=passed
```

The script creates all Pi homes, project files, credentials, session history, and
mock gateways under a temporary directory. It removes that directory when finished.

## Findings and mechanism

### Full user-config replacement

Pi's supported selector is `PI_CODING_AGENT_DIR`; the default is `~/.pi/agent`.
The SDK has the equivalent `agentDir` option. Pi does not expose a CLI
`--agent-dir` option. With the environment selector changed, the proof confirms
that settings, model catalog entries, keybindings, user context, and extensions
come from the selected agent directory. A model and extension unique to the normal
directory do not appear in the profile launch.

Pi project configuration remains separate. The proof approves a project `.pi`
directory and observes its system-prompt addition and extension; a control launch
with `--no-approve` omits those trust-gated resources but still loads project
`AGENTS.md`. No project-configuration disabling is used to simulate replacement.

### Preserve existing sessions and saved credentials

Changing only `PI_CODING_AGENT_DIR` changes Pi's default session directory too.
The profile must set `PI_CODING_AGENT_SESSION_DIR` (or pass Pi's native
`--session-dir`) to the existing per-project session directory, not merely the
parent `sessions/` directory. Pi's default path is
`<agent-dir>/sessions/--<resolved-cwd-with-separators-replaced-by-hyphens>--/`.
The test creates history under the normal agent home, resumes it under a selected
profile, and checks that the earlier prompt is in the next model request.
Concurrent runs write new session files to that same original per-project
directory.

Pi reads saved provider credentials from `<agent-dir>/auth.json`. The selected
profile's native directory and staged overlay therefore share the existing auth
file rather than copying it; the test confirms the saved synthetic key remains
available. Pi's documented credential order is runtime `--api-key`, stored
`auth.json`, a `models.json` key, then provider environment variables. Therefore
injecting aix's key only as `OPENAI_API_KEY` is insufficient when saved Pi auth is
present. The adapter supplies the selected aix key through Pi's one-shot
`--api-key` option. It rejects a user-supplied `--api-key`, which would contradict
the selected aix profile. Native `--provider openai` and model selection remain
available for model IDs declared by that profile; other providers, unconfigured
models, and `--models` cycling patterns are rejected by this proof adapter so they
cannot route around the selected endpoint.

### Keep the aix connection authoritative

Pi stores compatible provider endpoints in `models.json`. The adapter stages a
temporary copy of the selected profile's model configuration, sets its OpenAI
provider `baseUrl` from aix's generated `OPENAI_BASE_URL`, and passes aix's
`OPENAI_API_KEY` with `--api-key`. The proof runs all three applicable aix launch
forms: `aix run -- pi ...`, the configured named-tool form `aix pi -- ...`, and
generic `aix exec -- <adapter> ...`. Two concurrent `aix run -- pi ...` launches
reach separate local gateways with their own profile keys and model configuration.
The read-only source files remain byte-for-byte unchanged. The proof uses ordinary
Pi model selection and session flags; those overrides do not replace aix's
connection. The adapter rejects a conflicting API-key argument and Pi's nonexistent
`--agent-dir` argument.

### Relative and project resources

Pi resolves resource paths in user settings relative to the selected agent
directory. The proof loads a profile extension and skill referenced via `../shared`
from outside that directory. Its staging layout mirrors the relative base and
symlinks the unchanged shared resource directory, so references keep resolving to
the intended source. This pattern also works for read-only generated settings and
keybindings; only the staged `models.json` is rewritten.

The test covers file-based extensions and skills, not npm/git package installation.
Pi stores package-manager state below the agent directory (`npm`, `git`, and
temporary extension directories); a production adapter must keep those paths
available or share the selected profile's existing stores. Package installation
and updates need a writable store even when declarative configuration sources are
read-only.

## Compatibility limits and follow-up

- The proof pins Pi 1.0.4. Recheck selectors, credential priority, config paths,
  and package loading when supporting another Pi version.
- The proof exercises API-key auth. Pi's saved OAuth credentials also live in
  `auth.json`, but an OAuth refresh flow was not run; production must preserve the
  same file and Pi's locking/permissions behavior.
- Project trust-gated resources and context discovery are verified on Linux. No
  Windows or macOS run was performed.
- Temporary staging is required when aix must override a native model endpoint
  without editing its source. Relative external resource paths must be mirrored
  or made absolute in the stage; silently copying only `settings.json` breaks
  native relative-path semantics.
- The proof confirms feasibility through aix's existing configured-tool and generic
  process launch modes. It makes no production schema or launcher changes.

## Authoritative Pi sources

- [Configuration](https://pi.dev/docs/latest/configuration): agent-directory selector,
  user/project config files, and context discovery.
- [Environment variables](https://pi.dev/docs/latest/environment-variables):
  `PI_CODING_AGENT_DIR` and `PI_CODING_AGENT_SESSION_DIR`.
- [Sessions and context](https://pi.dev/docs/latest/sessions): default session
  location and `--session-dir` behavior.
- [Choose a model](https://pi.dev/docs/latest/models): credential precedence and
  compatible endpoint configuration.
- [Providers](https://pi.dev/docs/latest/providers): provider environment variables
  and `auth.json` behavior.
- [Settings](https://pi.dev/docs/latest/settings): resource path bases.
- [Pi packages](https://pi.dev/docs/latest/packages): native package sources,
  relative paths, and package stores.
- [Security / project trust](https://pi.dev/docs/latest/security): project trust,
  trust-gated resources, and context files.
- [Command line](https://pi.dev/docs/latest/cli): `--api-key`, session and resource
  options, and the absence of a CLI agent-directory selector.

The npm package version was resolved and installed explicitly as 1.0.4 for the
test; these documentation URLs are the maintained Pi references consulted on
2026-10-06.
