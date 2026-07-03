# Ticket 08 — LiteLLM endpoint compatibility model

**Date:** 2026-07-03
**Status:** approved

## Problem

`Endpoint` stores `api_format` as a plain `String` and `provider`/`gateway` as
`Option<String>`. None of these fields drive any runtime behavior — env var
emission is controlled by a separate `[compat]` table with two bool flags.
This creates two sources of truth for the same concept, and misses the
opportunity to validate `api_format` at parse time.

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| What replaces `Compat`? | `api_format` enum drives `collect_vars` directly | Single source of truth; `Compat` was redundant |
| How extensible are `provider`/`gateway`? | Open enums: `Known(KnownX) \| Custom(String)` | Validated known values, unknown strings accepted as metadata |
| How is `api_format` wired to `collect_vars`? | Direct: `collect_vars` takes `&ApiFormat` | No indirection needed at this scale |

## Architecture

### New types in `config.rs`

```rust
/// Closed enum — drives which SDK-compat vars are emitted.
/// Unknown values are rejected at parse time.
#[derive(Debug, Deserialize, PartialEq)]
pub enum ApiFormat {
    #[serde(rename = "anthropic")]  Anthropic,
    #[serde(rename = "openai")]     OpenAi,
    #[serde(rename = "both")]       Both,
}

/// Open enum — metadata only, does not drive behavior.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Provider {
    Known(KnownProvider),
    Custom(String),
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum KnownProvider {
    LiteLlm,  // "litellm"
}

/// Open enum — metadata only, does not drive behavior.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Gateway {
    Known(KnownGateway),
    Custom(String),
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum KnownGateway {
    Litellm,  // "litellm"
}
```

### `Endpoint` struct

```rust
// The gateway is a LiteLLM proxy: it speaks the Anthropic Messages API
// (and optionally the OpenAI Chat Completions API) but is not api.anthropic.com.
// `api_format` declares which SDK env vars the CLI should emit so downstream
// tools (claude, pi, etc.) reach the gateway without extra configuration.
pub struct Endpoint {
    pub base_url:   SecretSource,
    pub api_format: ApiFormat,        // was: String
    pub provider:   Option<Provider>, // was: Option<String>
    pub gateway:    Option<Gateway>,  // was: Option<String>
}
```

### `Config` struct

`compat: Compat` field removed. `Compat` struct removed entirely.

### `collect_vars` in `env.rs`

Signature changes from `&config::Compat` to `&config::ApiFormat`.

```rust
pub(crate) fn collect_vars(
    profile_name: &str,
    api_key:      &str,
    base_url:     &str,
    api_format:   &config::ApiFormat,
) -> Vec<(&'static str, String)>
```

Logic:

```
AIX_PROFILE, AIX_API_KEY, AIX_BASE_URL  — always emitted

api_format = "anthropic"  →  + ANTHROPIC_API_KEY, ANTHROPIC_BASE_URL
api_format = "openai"     →  + OPENAI_API_KEY, OPENAI_BASE_URL
api_format = "both"       →  + both pairs above
```

Call site in `env::run()` changes from `&cfg.compat` to `&cfg.endpoint.api_format`.

## Config shape

```toml
[endpoint]
provider   = "litellm"                # known Provider variant
gateway    = "litellm"                 # known Gateway variant
api_format = "anthropic"              # drives ANTHROPIC_* emission
base_url   = { env = "AIX_BASE_URL" }

[profiles.work]
label   = "Work"
api_key = { env = "AIX_API_KEY" }
```

## Var counts

| `api_format` | Total vars | Vars emitted |
|---|---|---|
| `"anthropic"` | 5 | AIX_{PROFILE,API_KEY,BASE_URL} + ANTHROPIC_{API_KEY,BASE_URL} |
| `"openai"` | 5 | AIX_{PROFILE,API_KEY,BASE_URL} + OPENAI_{API_KEY,BASE_URL} |
| `"both"` | 7 | AIX_{PROFILE,API_KEY,BASE_URL} + ANTHROPIC_* + OPENAI_* |

## Tests

### `config.rs`

- `api_format = "anthropic"` / `"openai"` / `"both"` parse to correct variants
- Unknown `api_format` value is rejected at parse time
- `provider = "litellm"` → `Provider::Known(KnownProvider::LiteLlm)`
- `provider = "some-future-provider"` → `Provider::Custom(...)`
- `gateway = "litellm"` → `Gateway::Known(KnownGateway::Litellm)`
- `gateway = "custom-gw"` → `Gateway::Custom(...)`
- Config with `[compat]` section is rejected (`deny_unknown_fields`)
- Existing TOML/YAML/JSON/JSON5 fixture tests updated to use enum assertions

### `env.rs`

- `ApiFormat::Anthropic` → 5 vars, ANTHROPIC_* present, OPENAI_* absent
- `ApiFormat::OpenAi` → 5 vars, OPENAI_* present, ANTHROPIC_* absent
- `ApiFormat::Both` → 7 vars, both present

## Files changed

| File | Change |
|------|--------|
| `src/config.rs` | Add `ApiFormat`, `Provider`, `Gateway`, `KnownProvider`, `KnownGateway`; update `Endpoint`; remove `Compat` |
| `src/commands/env.rs` | Update `collect_vars` signature and call site |
| `docs/aix/example-config.toml` | New file: full example with all three `api_format` values |

## Acceptance criteria

- `api_format = "anthropic"` → ANTHROPIC_* emitted, OPENAI_* not emitted
- `api_format = "openai"` → OPENAI_* emitted, ANTHROPIC_* not emitted
- `api_format = "both"` → both emitted
- AIX_* vars always emitted regardless of `api_format`
- Unknown `api_format` value fails at parse time with a clear error
- Unknown provider/gateway strings are accepted (Custom variant)
- `[compat]` table in config causes a parse error (field no longer exists)
- No deployment-specific secret path hardcoded in Rust
