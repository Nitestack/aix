# Pi agent-directory replacement proof

## Result

The production aix launcher replaces Pi's complete user-level agent configuration
with the directory selected by the active profile, while sharing the existing
session, trust, package, and authentication stores. The version-pinned proof runs
the real Pi 1.0.4 executable through both `aix pi` and `aix run -- pi`; the configured
tool command is a wrapper that simply execs Pi.

Pi's native provider and RPC model-switch commands remain available. Following the
product decision for issue #35, native switching is allowed; the proof therefore
makes no claim that aix controls requests after a user switches to another provider.

The reproducible proof is [`pi_agent_config_replacement.py`](pi_agent_config_replacement.py),
with checks and shared fixtures in
[`pi_agent_config_replacement_checks.py`](pi_agent_config_replacement_checks.py) and
[`pi_agent_config_replacement_support.py`](pi_agent_config_replacement_support.py).
It uses synthetic credentials and local mock OpenAI-compatible gateways; no real
model account or external model request is needed.

The TOML option is `[profiles.<name>.tool_configs.pi].agent_dir`. It accepts an
absolute path, a `~` home-relative path, or a path relative to the aix config file.

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
aix_named_and_run_launches_with_concurrent_profiles=passed
native_project_session_dir=passed
native_provider_switch=allowed_without_aix_authority_claim
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

The launcher links the staged agent's `sessions/` directory to the normal Pi
agent's store. Pi's default per-project session path therefore resolves to the
existing history without injecting `PI_CODING_AGENT_SESSION_DIR` or overriding Pi's
native `sessionDir` settings. The proof creates history under the normal agent
home, resumes it through the selected profile, and checks that the earlier prompt
is present in the next model request. Concurrent profile launches add sessions to
that same store. A separate launch confirms a relative project-level `sessionDir`
continues to select its own directory.

Pi reads saved provider credentials from `<agent-dir>/auth.json`. The staged
directory links the normal `auth.json`, `mcp-auth.json`, and `trust.json` rather
than copying them; the proof confirms the saved synthetic keys remain available.
Pi's documented credential order gives runtime `--api-key` priority over saved
auth, so the launcher supplies the selected aix key through Pi's one-shot
`--api-key` option for normal launches. It rejects explicit `--api-key` and
`--agent-dir` arguments when profile selection is active. Native provider/model
selection remains available; the configured provider's endpoint is rewritten to
the aix endpoint, while other native providers retain their own configuration.

### Configured endpoint and native switching policy

Pi stores compatible provider endpoints in `models.json`. The production launcher
stages a temporary copy of the selected profile's native directory and rewrites
the provider and model `baseUrl` values for the configured aix API format. It passes
the selected profile key with `--api-key`. The proof runs the real Pi process
through both `aix pi -- ...` and `aix run -- pi ...`; simultaneous runs reach
separate local gateways with their own profile keys and model configuration. The
read-only source files remain byte-for-byte unchanged.

Pi 1.0.4 requires `--api-key` to accompany a CLI model selector, even when
`settings.json` already defines a default model. If a profile-configured launch has
no `--model`, `--provider`, or `--models` argument, aix adds `--models '*'`. Pi
still selects the configured default model first, and the wildcard keeps the full
available model set in scope. The pinned proof checks this behavior through both
`aix pi` and `aix run -- pi`.

Native provider switching is deliberately allowed. With a synthetic saved
Anthropic credential and model in the existing Pi configuration, an RPC
`set_model` switches to that provider and returns its original
`http://saved-provider.invalid/v1` endpoint. The test sends no request to that
provider. This is the agreed behavior for issue #35: ordinary requests using the
configured aix provider use the profile endpoint and key, but aix does not claim
connection authority after a user explicitly switches to another native provider.
The launcher rejects explicit `--api-key` and `--agent-dir` arguments while custom
profile configuration is active. Other native Pi options and project extensions
remain available under Pi's trust rules; extensions are not a security sandbox.

### Relative and project resources

Pi resolves resource paths in user settings relative to the selected agent
directory. The launcher converts relative file resources to their original source
paths before staging, so extensions and skills referenced via `../shared` continue
to resolve outside that directory. This works with read-only settings and
keybindings; the source files themselves remain unchanged. The stage rewrites
`settings.json` only for relative resource references and `models.json` only for
configured-provider endpoints.

The proof covers file-based extensions and skills, not npm/git package installation.
The launcher shares the original agent's `npm` and `git` stores; package
installation or updates may need those stores to be writable even when declarative
configuration sources are read-only.

## Compatibility limits and follow-up

- The proof pins Pi 1.0.4. Recheck selectors, credential priority, config paths,
  and package loading when supporting another Pi version.
- Profile-specific Pi directory selection requires an API-key profile. ChatGPT
  authentication is rejected before launch; no unrelated authentication mode is
  added by this feature.
- The proof exercises API-key auth. Saved OAuth credentials also live in `auth.json`,
  but an OAuth refresh flow was not run.
- Project trust-gated settings/resources and context discovery are verified on
  Linux. No Windows or macOS run was performed.
- Temporary staging lets aix override a native model endpoint without editing its
  source. The production launcher resolves relative resources to their source path
  and shares user data by link rather than copying it.
- Replacement, session/history preservation, saved-auth preservation, and both
  configured launch modes are demonstrated through the production schema and
  launcher. Native provider switches remain an explicit supported limitation, not
  a claim of aix connection authority.

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
- [Settings](https://pi.dev/docs/latest/settings): resource path bases and
  `sessionDir` precedence.
- [Pi packages](https://pi.dev/docs/latest/packages): native package sources,
  relative paths, and package stores.
- [Security / project trust](https://pi.dev/docs/latest/security): project trust,
  trust-gated resources, and context files.
- [Command line](https://pi.dev/docs/latest/cli): `--api-key`, session and resource
  options, and the absence of a CLI agent-directory selector.

The npm package version was resolved and installed explicitly as 1.0.4 for the
test; these documentation URLs are the maintained Pi references consulted on
2026-10-06.
