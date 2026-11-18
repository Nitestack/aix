# Shell integration

This page covers selecting a profile for the current shell (`aix init` / `aix use` / `aix current`), exporting credentials explicitly with `aix env`, and running commands or a subshell with a profile via `aix exec` and `aix shell`.

## Profile switching

Install the lightweight shell wrapper once. It intercepts `aix use` to set only
`AIX_PROFILE` in the current shell; every other invocation is passed to the aix
binary unchanged. Switching profiles checks profile names only and does not
resolve API keys, base URLs, or custom profile variables.

### POSIX sh / bash / zsh

```sh
# Pick the shell matching your current session.
eval "$(command aix init zsh)"

aix use work
aix current                 # prints: work
aix current --format short  # same one-line output, suitable for prompts
aix use --clear             # unsets AIX_PROFILE
```

Use `command aix init sh` or `command aix init bash` for those shells.

### Fish

```fish
command aix init fish | source
aix use work
aix current
```

### Nushell

Nushell's `source` command needs a file path, so save the generated wrapper once:

```nu
mkdir ~/.config/aix
^aix init nu | save --force ~/.config/aix/init.nu
source ~/.config/aix/init.nu

aix use work
aix current
```

### PowerShell

```powershell
aix init powershell | Invoke-Expression
aix use work
aix current
```

`aix current --json` uses the standard JSON envelope and returns `name`, `label`,
and `source` (`env`, `default`, or `none`) in `data`. If no valid profile is
selected, human output is `none` and the JSON `name` and `label` are `null`.

Without shell initialization, `aix use work` prints a POSIX `sh` assignment;
source or eval that output in the current shell if desired. The shell wrapper
selects the correct output format explicitly. Direct calls can choose one with
`--shell sh|bash|zsh|fish|nu|powershell`; `--format json` prints an object with
`AIX_PROFILE` set to the selected name or `null` when cleared. For example:

```sh
aix use work --shell fish
aix use --clear --shell fish
```

## Explicit credential export with `aix env`

`aix env` is available for workflows that intentionally need credential
variables in the current shell. Unlike `aix use`, it resolves and prints those
credentials.

### POSIX sh / bash / zsh

```sh
# Emit and eval (current shell)
eval "$(aix env work)"

# Explicitly request sh format
eval "$(aix env work --format sh)"

# Use default profile
eval "$(aix env)"
```

### Nushell

```nu
# JSON is the simplest approach — no temp file needed
aix env work --format json | from json | load-env

# Alternatively, use the Nushell format and source a file
# (source requires a literal path, so save first)
aix env work --format nu | save --force /tmp/aix-env.nu
source /tmp/aix-env.nu
```

### Fish

```fish
# Fish format uses set -x KEY 'value' syntax
aix env work --format fish | source
```

> **Note:** `eval (aix env work --format fish)` does **not** work. Fish command substitution splits output on newlines into separate list elements, and `eval` then joins them with spaces — collapsing all the `set -x` lines into a single malformed `set` call. Use `| source` instead.

### PowerShell

```powershell
# PowerShell format uses $env:KEY = 'value' syntax
aix env work --format powershell | Invoke-Expression
```

### Windows Command Prompt (cmd.exe)

```cmd
REM Save to a temp file and call it in the current session
aix env work --format cmd > "%TEMP%\aix-env.cmd" && call "%TEMP%\aix-env.cmd"

REM Use default profile
aix env --format cmd > "%TEMP%\aix-env.cmd" && call "%TEMP%\aix-env.cmd"
```

> **Note:** Values containing `%` are safe — the output doubles them to `%%` so `SET` interprets them correctly. Values containing `"` use a `""` encoding that works on modern Windows 10/11 cmd.exe but is not guaranteed on all NT versions. If your API key or base URL contains a literal double-quote (rare in practice), use `--format powershell` instead.

### Inspect without loading (JSON)

```bash
# Useful for scripting or debugging — outputs a JSON object
aix env work --format json | jq .
```

## Running commands with a profile

For launching AI tools such as `claude`, `codex`, or `opencode`, see [Running AI tools](tools.md). `aix exec` and `aix shell` below are the generic, stateless wrappers; use [`aix run`](runs.md) when you need run history.

### `aix exec` — one-shot command

Run any command with the profile environment set. Arguments after `--` are passed through unchanged.

```bash
# Run curl with work credentials
aix exec work -- curl -s https://ai-hub.example.com/health

# Use default profile
aix exec -- my-ai-tool --prompt "Hello"

# Dry-run: print what would run without running it (no secrets in output)
aix exec work --dry-run -- my-ai-tool
```

### `aix shell` — interactive shell session

Opens an interactive shell with the profile environment pre-loaded. Shell detection order: `$SHELL` env var → `NU_VERSION` (Nushell, if on `$PATH`) → `sh` fallback.

```bash
# Work profile
aix shell work

# Default profile
aix shell

# See what shell would be launched
aix shell work --dry-run
```
