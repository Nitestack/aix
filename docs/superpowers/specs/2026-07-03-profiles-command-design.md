# Profiles Command & Interactive Profile Selection — Design

**Ticket:** 06  
**Date:** 2026-07-03

## Goal

Replace the `gum choose` dependency for profile selection with built-in Rust.
Add `aix profiles` and `aix profiles --json` commands.
Make `aix env` fall through to an interactive selector when no profile is given and the terminal is interactive.

## Behavior

### `aix profiles`

Reads config, prints one line per profile — the label if set, the name otherwise:

```
Work account
Fast (gpt-4o-mini)
local
```

Sorted by profile name (alphabetical).

### `aix profiles --json`

Emits a JSON array of objects. No secrets.

```json
[
  {"name": "fast", "label": "Fast (gpt-4o-mini)"},
  {"name": "local", "label": "local"},
  {"name": "work", "label": "Work account"}
]
```

`label` is the configured label value, or the profile name when no label is set.

### `aix env [profile]` — profile resolution chain

1. Positional `profile` argument → use it directly.
2. `default_profile` in config → use it.
3. Both `stdin` and `stdout` are terminals → show `inquire::Select` picker. Display label (or name) for each option; resolve back to the profile name on selection.
4. Non-interactive (not a terminal) → fail with `AihubError::NoInteractiveTerminal`.

The picker displays labels, never secret values. If the user cancels (Esc / Ctrl-C), the command exits with a clear error: `AihubError::SelectionCancelled`.

### Duplicate label validation

`config::validate()` checks that no two profiles share the same non-None label. On collision it returns `AihubError::DuplicateLabel { label, first, second }` where `first`/`second` are the profile names. This fires for all commands, not just the selector.

## Files

| File | Change |
|---|---|
| `src/cli.rs` | Add `json: bool` flag to `Profiles` variant; pass `config_path` into profiles dispatch |
| `src/main.rs` | Thread `config_path` into `profiles::run` |
| `src/error.rs` | Add `DuplicateLabel { label, first, second }` and `NoInteractiveTerminal` variants |
| `src/config.rs` | Add `sorted_profiles()` helper; extend `validate()` with duplicate-label check |
| `src/commands/profiles.rs` | Full implementation: text + JSON rendering |
| `src/commands/env.rs` | Replace two-step `.or_else()` chain with explicit three-step resolver |
| `tests/cli_profiles.rs` | Integration tests (see Testing section) |

## Key types / functions

### `config::sorted_profiles<'a>(cfg: &'a Config) -> Vec<(&'a str, &'a str)>`

Returns `(name, display_label)` pairs sorted by name.  
`display_label` is `profile.label.as_deref().unwrap_or(name)`.

Used by both `profiles::run` and the interactive selector in `env::run`.

### `config::validate()` — duplicate-label extension

```
collect all (label, name) pairs where label is Some
group by label
if any group has >1 name → return Err(DuplicateLabel { label, first, second })
```

### `error.rs` new variants

```rust
#[error("duplicate profile label \"{label}\" — profiles \"{first}\" and \"{second}\"")]
DuplicateLabel { label: String, first: String, second: String },

#[error("no profile specified; pass a profile name or run in an interactive terminal")]
NoInteractiveTerminal,

#[error("profile selection cancelled")]
SelectionCancelled,
```

### `env::run` — profile resolver

```
fn resolve_profile(positional, cfg) -> Result<String, AihubError>:
  if positional.is_some() → return positional
  if cfg.default_profile.is_some() → return default_profile
  if stdin().is_terminal() && stdout().is_terminal():
    build options from sorted_profiles()
    run inquire::Select
    return selected name
  else:
    return Err(NoInteractiveTerminal)
```

TTY detection uses `std::io::IsTerminal` (stable since Rust 1.70, no extra crate).

## Testing

### Unit tests (in-module)

- `config::sorted_profiles` returns entries sorted by name, label falls back to name.
- `config::validate` errors on duplicate labels with correct field values.
- `config::validate` passes when labels are all distinct or all None.

### Integration tests (`tests/cli_profiles.rs`)

| Scenario | Mechanism |
|---|---|
| `aix profiles` text output | assert_cmd + temp config file |
| `aix profiles --json` valid JSON, no secrets | parse output with serde_json |
| `aix env <profile>` explicit profile | existing test patterns |
| `aix env` with `default_profile` set | env var + temp config |
| `aix env` non-interactive, no profile | pipe stdin (not a terminal) → expect `NoInteractiveTerminal` error |
| duplicate label in config | any command → expect `DuplicateLabel` error |

Non-interactive path is testable without mocking: `assert_cmd` runs the binary as a child process with piped stdio, so `is_terminal()` naturally returns false.

## Out of scope

- Interactive selection for commands other than `env`.
- Fuzzy search within the picker.
- Profile ordering other than alphabetical by name.
