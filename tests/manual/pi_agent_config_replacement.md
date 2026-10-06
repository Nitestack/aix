# Pi agent-directory replacement proof

## Result

**Partial proof; the full feasibility gate is not satisfied.** Pi can replace its
complete user-level agent directory with the directory selected by an aix profile
while reusing the original session, trust, and authentication stores. A launch
adapter can stage read-only/generated configuration and make the selected aix
connection win for requests using the profile's OpenAI-compatible models. Pi's
native RPC model-switch command can still select another configured provider from
the preserved auth/config stores, so this proof does not claim connection authority
after arbitrary native provider switches.

The reproducible proof is [`pi_agent_config_replacement.py`](pi_agent_config_replacement.py).
Its adapter, checks, and shared test fixtures are
[`pi_agent_config_overlay.py`](pi_agent_config_overlay.py),
[`pi_agent_config_replacement_checks.py`](pi_agent_config_replacement_checks.py),
and [`pi_agent_config_replacement_support.py`](pi_agent_config_replacement_support.py).
The runner uses synthetic credentials and local mock OpenAI-compatible gateways; it
does not need a real model account or send requests to an external service.

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
aix_shell_run_named_tool_exec_and_concurrent_profiles=passed
connection_authority=blocked_by_native_rpc_model_switch
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
directory and observes its `defaultThinkingLevel` setting, system-prompt addition,
and extension; a control launch with `--no-approve` omits those trust-gated
resources but still loads project `AGENTS.md`. An SDK settings check confirms the
project setting is effective only in the trusted state. No project-configuration
disabling is used to simulate replacement. The existing project's saved decision
in the normal agent's `trust.json` is shared into each profile; one launch uses it
without an override, while `--no-approve` overrides it as Pi specifies.

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
available. The profile also shares the existing `trust.json` and session leaf.
Pi's documented credential order is runtime `--api-key`, stored
`auth.json`, a `models.json` key, then provider environment variables. Therefore
injecting aix's key only as `OPENAI_API_KEY` is insufficient when saved Pi auth is
present. The adapter supplies the selected aix key through Pi's one-shot
`--api-key` option. It rejects a user-supplied `--api-key`, which would contradict
the selected aix profile. Native `--provider openai` and model selection remain
available for model IDs declared by that profile. The adapter supplies an exact
`--models` scope for the selected profile's OpenAI catalog, and rejects user
`--models`, other-provider CLI selectors, unconfigured models, and explicit
`--extension` sources. That scope limits startup lookup and model cycling, but it
does not constrain Pi's RPC `set_model` command.

### Connection-authority blocker

Pi stores compatible provider endpoints in `models.json`. The adapter stages a
temporary copy of the selected profile's model configuration, sets both the
OpenAI provider and each selected model's `baseUrl` from aix's generated
`OPENAI_BASE_URL`, scopes model cycling to those OpenAI models, and passes aix's
`OPENAI_API_KEY` with `--api-key`. The proof runs all four aix launch forms:
`aix shell` followed by `pi ...` through a temporary `PATH` shim,
`aix run -- pi ...`, the configured named-tool form `aix pi -- ...`, and generic
`aix exec -- <adapter> ...`. Two concurrent
`aix run -- pi ...` launches reach separate local gateways with their own profile
keys and model configuration.
The read-only source files remain byte-for-byte unchanged. The proof demonstrates
that ordinary CLI model selection within the AIX profile's OpenAI catalog routes
to the selected gateway in all four launch forms. It also demonstrates a blocker:
with a synthetic saved Anthropic credential and an Anthropic model in the existing
Pi `models.json`, Pi 1.0.4 accepts an RPC `set_model` to that model even when
launched with `--models openai/profile-a-model`. The returned model has
`baseUrl: http://saved-provider.invalid/v1`, not the AIX-selected endpoint. This
test sends no request to that model or URL.

The behavior is consistent with Pi's documented/source behavior: `--models`
provides a startup/cycling scope (`docs/cli.md`), while `dist/modes/rpc/rpc-mode.js`
implements `set_model` by searching `session.modelRuntime.getAvailableSnapshot()`
without checking the scope. The test preserves the existing `auth.json`, so an
authenticated non-OpenAI model remains available. Deciding whether aix profiles
must disable these native provider switches, map every Pi provider through the
profile, or explicitly permit them requires a product decision; this proof does
not weaken that contract or claim the gate is complete.

The adapter rejects conflicting API-key, provider, extension, and agent-directory
arguments. Explicit project/profile extensions still load under Pi trust rules;
as arbitrary in-process code they are not a security sandbox.

### Relative and project resources

Pi resolves resource paths in user settings relative to the selected agent
directory. The proof loads a profile extension and skill referenced via `../shared`
from outside that directory. Its staging layout mirrors the relative base and
symlinks the unchanged shared resource directory, so references keep resolving to
the intended source. This pattern also works for read-only generated settings and
keybindings. The staged overlay rewrites `settings.json` only to set the default
provider to OpenAI and `models.json` to replace provider- and model-level endpoints;
the source files themselves remain unchanged.

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
- Project trust-gated settings/resources and context discovery are verified on
  Linux. No Windows or macOS run was performed.
- Temporary staging is required when aix must override a native model endpoint
  without editing its source. Relative external resource paths must be mirrored
  or made absolute in the stage; silently copying only `settings.json` breaks
  native relative-path semantics.
- Replacement, session/history preservation, and saved-auth preservation are
  demonstrated through existing aix launch modes. Overall connection authority
  remains blocked by the native RPC provider switch above. No production schema or
  launcher changes are made.

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
