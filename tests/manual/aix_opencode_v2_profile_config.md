# aix OpenCode profile config check

This manual integration check exercises aix's profile config staging against
the exact OpenCode `v2.0.20` CLI. It complements
[`opencode_v2_config_replacement.md`](opencode_v2_config_replacement.md), which
establishes the native OpenCode behavior without exercising aix.

The script refuses to run against another OpenCode release. It invokes real
OpenCode commands through both `aix opencode` and `aix run -- opencode`; no
live inference or external model endpoint is used. A loopback fake model returns
a deterministic response. A small executable shim records the command
arguments and staged files, then execs OpenCode unchanged. All
OpenCode home, config, data, cache, and state paths and all disposable fixtures
are isolated under `/tmp/opencode` and removed on exit. No existing OpenCode
profile, auth, or shared server is accessed.

## Run

From the repository root, with OpenCode `v2.0.20` available as `opencode`:

```sh
nix develop --no-write-lock-file --command cargo build
mkdir -p /tmp/opencode
python3 tests/manual/aix_opencode_v2_profile_config.py
```

Set `AIX_BIN` or `OPENCODE_BIN` to override the default binary paths.

## Assertions

- An aix-selected app config replaces the standard global app config while
  project config and `AGENTS.md` instructions and standard `cli.json` remain
  available. A local fake model endpoint verifies the profile connection.
- An inherited `OPENCODE_CONFIG_DIR`, explicit `OPENCODE_CONFIG`, and inline
  app content cannot override the selected app config.
- A CLI-only profile retains the standard app config and replaces the
  inherited `cli.json`; OpenCode's plugin command reads the selected staged
  file, and changes never reach the source file.
- Switching profile config roots leaves OpenCode's data directory unchanged;
  a session with user history and saved application auth seeded before the
  switch remain available afterward.
- aix supplies `--standalone` to server-backed OpenCode commands in the
  subcommand-specific position and removes each temporary stage after the
  child exits. OpenCode's local `plugin` config commands do not accept the
  server flag and are exercised without it.
- Source files remain byte-for-byte unchanged.

The verified compatibility boundary is OpenCode `v2.0.20`, matching the
version-pinned native proof. Re-run both checks and re-check the pinned upstream
sources before claiming support for another OpenCode release.
