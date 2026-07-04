# Design: Dynamic label sources for profiles

**Date:** 2026-07-04

## Motivation

Profile labels are display names shown in the interactive picker. Writing them inline in the
config file leaks what services or license tiers the user has — someone who sees the config
can infer account details from label text like "Work Premium" or "Anthropic Internal".

API keys already support `SecretSource` (env / file / command / direct). Labels should accept
the same source variants so Nix users can store label text in a secrets file and keep it out
of the world-readable config.

## Chosen approach

Introduce a new public `DynamicValue` type that accepts the same config syntax as `SecretSource`
but resolves to a plain `String` instead of a zeroed `SecretString`. A private `SourceKind` enum
shared between both types eliminates logic duplication.

Labels are not secrets in the cryptographic sense (they are displayed on screen), so `DynamicValue`
does not apply zeroize or hide values in Debug. The distinction between `SecretSource` and
`DynamicValue` in struct fields is a meaningful signal to future maintainers: `SecretSource` means
"keep this confidential", `DynamicValue` means "this is dynamic but not sensitive".

## Architecture

### `src/secrets.rs`

**New private `SourceKind` enum:**

```rust
enum SourceKind { Direct(String), Env(String), File(PathBuf), Command(String) }
```

`SourceKind::resolve_raw(&self) -> Result<String, AixError>` is the single implementation of
env-var lookup, file read, command execution, and direct passthrough. Both public types delegate
to it.

**Shared deserialization helpers** (renamed from `SecretSourceDe` / `SecretSourceFields`):

- `SourceKindDe` — `#[serde(untagged)]` enum matching plain string vs structured table
- `SourceKindFields` — `#[serde(deny_unknown_fields)]` struct with `env`, `file`, `command`
- `fn try_from_de<E: serde::de::Error>(de: SourceKindDe) -> Result<SourceKind, E>` — the
  "exactly one field" matching used by both `Deserialize` impls

**`SecretSource(SourceKind)`** — unchanged public API. `resolve()` wraps the raw string in
`SecretString`. Debug hides `Direct` values.

**`DynamicValue(SourceKind)`** — new public struct. `resolve() -> Result<String, AixError>`
calls `self.0.resolve_raw()` directly. Debug shows source type and identifier for all variants,
including the literal value for `Direct` (it is already visible in the config file).

Config syntax is identical for both types:

```toml
label = "Work"                                  # Direct
label = { env = "AIX_WORK_LABEL" }             # Env
label = { file = "/run/secrets/aix/label" }    # File
label = { command = "pass show aix/label" }    # Command
```

### `src/config.rs`

- `Profile.label`: `Option<String>` → `Option<DynamicValue>`
- `sorted_profiles(&cfg) -> Result<Vec<(&str, String)>, AixError>` — resolves labels eagerly;
  returns an error if any source (missing env var, unreadable file, failed command) fails
- `validate(&cfg) -> Result<(), AixError>` — adds a label resolution pass so bad sources are
  caught at startup before any command runs
- `format_available_profiles(&cfg) -> String` — stays infallible; resolves each label with
  `.unwrap_or_else(|_| name.to_string())` so a bad source degrades gracefully in error messages
  rather than cascading

### `src/commands/profiles.rs`

- `run()` adds `?` after `sorted_profiles`
- `ProfileEntry.label` and the `print_json` / `print_text` helpers update from `&str` to `String`

### `src/commands/env.rs`

- `select_profile_interactively` calls `sorted_profiles(cfg)?`
- Label comparison in `find` changes from `*label == selected.as_str()` to `label == selected`
- `format_available_profiles` usage in `ProfileNotFound` construction is unchanged

### `nix/home-manager.nix`

- `profiles.*.label` option type: `lib.types.nullOr lib.types.str` →
  `lib.types.nullOr secretSourceType`
- String coercion in `secretSourceType` means `label = "Work"` continues to work unchanged
- `mkProfile` encodes the label via `encodeSecretSource (validateSecretSource profile.label)`
  when non-null, so `{ file = "/run/secrets/my-label"; }` renders as the correct TOML subtable

## Error handling

Label resolution errors surface the same `AixError` variants already used for `api_key`:
`SecretMissingEnvVar`, `SecretFileRead`, `SecretCommandSpawn`, `SecretCommandFailed`. No new
error variants are needed. `validate` is the primary fail-fast site.

## Testing

- Unit tests in `secrets.rs`: `DynamicValue` deserialization (all four variants), resolution
  happy path and error paths, Debug output
- Unit tests in `config.rs`: `sorted_profiles` with a `DynamicValue` label (env and file
  variants), `validate` surfaces label resolution errors, `format_available_profiles` falls back
  gracefully when label resolution fails

## Backward compatibility

Plain string labels (`label = "Work"`) continue to work — `DynamicValue` deserializes a bare
string as `SourceKind::Direct`, identical to the current behaviour.
