# Ticket 08: LiteLLM Endpoint Compatibility Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the loose `api_format: String` / `provider: Option<String>` / `gateway: Option<String>` fields and the `Compat` struct with typed enums, and wire `api_format` directly into env var emission in `collect_vars`.

**Architecture:** Add a closed `ApiFormat` enum and open `Provider`/`Gateway` enums (known variant + `Custom(String)` fallback) to `config.rs`. Remove the `Compat` struct and `Config.compat` field entirely. Update `collect_vars` in `env.rs` to accept `&config::ApiFormat` and use `matches!` to decide which SDK env vars to emit.

**Tech Stack:** Rust, `serde` (`#[serde(untagged)]` for open enums, `#[serde(rename)]` for exact string values), `toml`/`serde_yaml`/`serde_json`/`json5` for parsing, `cargo test` for verification.

---

## File Map

| File | Change |
|------|--------|
| `tools/aix/src/config.rs` | Add `ApiFormat`, `KnownProvider`, `Provider`, `KnownGateway`, `Gateway`; update `Endpoint` field types; remove `Compat` struct and `Config.compat`; update tests |
| `tools/aix/src/commands/env.rs` | Update `collect_vars` signature and body; update call site in `run()`; replace compat-based tests with `ApiFormat`-based tests |
| `docs/aix/example-config.toml` | New: full example config showing all three `api_format` values |

---

## Task 1: Add `ApiFormat` enum

**Files:**
- Modify: `tools/aix/src/config.rs`

- [ ] **Step 1: Write failing tests for `ApiFormat` parsing**

  Add to the bottom of `mod tests` in `tools/aix/src/config.rs`:

  ```rust
  // --- ApiFormat ---

  #[test]
  fn api_format_anthropic_parses() {
      let cfg: Config = toml::from_str(
          r#"
  [endpoint]
  base_url = { env = "X" }
  api_format = "anthropic"
  [profiles.work]
  api_key = "sk-test"
  "#,
      )
      .unwrap();
      assert_eq!(cfg.endpoint.api_format, ApiFormat::Anthropic);
  }

  #[test]
  fn api_format_openai_parses() {
      let cfg: Config = toml::from_str(
          r#"
  [endpoint]
  base_url = { env = "X" }
  api_format = "openai"
  [profiles.work]
  api_key = "sk-test"
  "#,
      )
      .unwrap();
      assert_eq!(cfg.endpoint.api_format, ApiFormat::OpenAi);
  }

  #[test]
  fn api_format_both_parses() {
      let cfg: Config = toml::from_str(
          r#"
  [endpoint]
  base_url = { env = "X" }
  api_format = "both"
  [profiles.work]
  api_key = "sk-test"
  "#,
      )
      .unwrap();
      assert_eq!(cfg.endpoint.api_format, ApiFormat::Both);
  }

  #[test]
  fn api_format_unknown_rejected() {
      let bad = r#"
  [endpoint]
  base_url = { env = "X" }
  api_format = "grpc"
  [profiles.work]
  api_key = "sk-test"
  "#;
      assert!(toml::from_str::<Config>(bad).is_err());
  }
  ```

- [ ] **Step 2: Run tests — expect compile error**

  ```bash
  cargo test --manifest-path tools/aix/Cargo.toml 2>&1 | head -20
  ```

  Expected: compile error containing `cannot find type \`ApiFormat\`` or similar.

- [ ] **Step 3: Add `ApiFormat` enum and update `Endpoint`**

  In `tools/aix/src/config.rs`, add the enum **before** the `Endpoint` struct:

  ```rust
  #[derive(Debug, Deserialize, PartialEq)]
  pub enum ApiFormat {
      #[serde(rename = "anthropic")]
      Anthropic,
      #[serde(rename = "openai")]
      OpenAi,
      #[serde(rename = "both")]
      Both,
  }
  ```

  Then update `Endpoint`:

  ```rust
  #[derive(Debug, Deserialize)]
  #[serde(deny_unknown_fields)]
  pub struct Endpoint {
      pub base_url: SecretSource,
      pub api_format: ApiFormat,        // was: String
      pub provider: Option<String>,
      pub gateway: Option<String>,
  }
  ```

- [ ] **Step 4: Fix `assert_standard` to use enum comparison**

  In `mod tests`, find `assert_standard` and change the `api_format` assertion:

  ```rust
  // Remove:
  assert_eq!(cfg.endpoint.api_format, "anthropic");
  // Add:
  assert_eq!(cfg.endpoint.api_format, ApiFormat::Anthropic);
  ```

- [ ] **Step 5: Run tests — expect all pass**

  ```bash
  cargo test --manifest-path tools/aix/Cargo.toml
  ```

  Expected: all tests pass.

- [ ] **Step 6: Commit**

  ```bash
  git add tools/aix/src/config.rs
  git commit -m "feat(config): add ApiFormat enum, replace api_format: String"
  ```

---

## Task 2: Add `Provider` and `Gateway` open enums

**Files:**
- Modify: `tools/aix/src/config.rs`

- [ ] **Step 1: Write failing tests for `Provider` and `Gateway` parsing**

  Add to the bottom of `mod tests` in `tools/aix/src/config.rs`:

  ```rust
  // --- Provider ---

  #[test]
  fn provider_known_parses() {
      let cfg: Config = toml::from_str(
          r#"
  [endpoint]
  base_url = { env = "X" }
  api_format = "anthropic"
  provider = "litellm"
  [profiles.work]
  api_key = "sk-test"
  "#,
      )
      .unwrap();
      assert_eq!(
          cfg.endpoint.provider,
          Some(Provider::Known(KnownProvider::LiteLlm))
      );
  }

  #[test]
  fn provider_custom_parses() {
      let cfg: Config = toml::from_str(
          r#"
  [endpoint]
  base_url = { env = "X" }
  api_format = "anthropic"
  provider = "my-future-provider"
  [profiles.work]
  api_key = "sk-test"
  "#,
      )
      .unwrap();
      assert!(
          matches!(cfg.endpoint.provider, Some(Provider::Custom(ref s)) if s == "my-future-provider")
      );
  }

  #[test]
  fn provider_absent_is_none() {
      let cfg: Config = toml::from_str(
          r#"
  [endpoint]
  base_url = { env = "X" }
  api_format = "anthropic"
  [profiles.work]
  api_key = "sk-test"
  "#,
      )
      .unwrap();
      assert_eq!(cfg.endpoint.provider, None);
  }

  // --- Gateway ---

  #[test]
  fn gateway_known_parses() {
      let cfg: Config = toml::from_str(
          r#"
  [endpoint]
  base_url = { env = "X" }
  api_format = "anthropic"
  gateway = "litellm"
  [profiles.work]
  api_key = "sk-test"
  "#,
      )
      .unwrap();
      assert_eq!(
          cfg.endpoint.gateway,
          Some(Gateway::Known(KnownGateway::Litellm))
      );
  }

  #[test]
  fn gateway_custom_parses() {
      let cfg: Config = toml::from_str(
          r#"
  [endpoint]
  base_url = { env = "X" }
  api_format = "anthropic"
  gateway = "my-custom-gateway"
  [profiles.work]
  api_key = "sk-test"
  "#,
      )
      .unwrap();
      assert!(
          matches!(cfg.endpoint.gateway, Some(Gateway::Custom(ref s)) if s == "my-custom-gateway")
      );
  }

  #[test]
  fn gateway_absent_is_none() {
      let cfg: Config = toml::from_str(
          r#"
  [endpoint]
  base_url = { env = "X" }
  api_format = "anthropic"
  [profiles.work]
  api_key = "sk-test"
  "#,
      )
      .unwrap();
      assert_eq!(cfg.endpoint.gateway, None);
  }
  ```

- [ ] **Step 2: Run tests — expect compile error**

  ```bash
  cargo test --manifest-path tools/aix/Cargo.toml 2>&1 | head -20
  ```

  Expected: compile error containing `cannot find type \`Provider\`` or similar.

- [ ] **Step 3: Add the four new types in `config.rs`**

  Add these **before** the `Endpoint` struct, after `ApiFormat`:

  ```rust
  #[derive(Debug, Deserialize, PartialEq)]
  #[serde(rename_all = "kebab-case")]
  pub enum KnownProvider {
      LiteLlm,
  }

  #[derive(Debug, Deserialize, PartialEq)]
  #[serde(untagged)]
  pub enum Provider {
      Known(KnownProvider),
      Custom(String),
  }

  #[derive(Debug, Deserialize, PartialEq)]
  #[serde(rename_all = "kebab-case")]
  pub enum KnownGateway {
      Litellm,
  }

  #[derive(Debug, Deserialize, PartialEq)]
  #[serde(untagged)]
  pub enum Gateway {
      Known(KnownGateway),
      Custom(String),
  }
  ```

  `#[serde(untagged)]` makes serde try `Known` first (requires a matching string) and fall through to `Custom(String)` for anything else.

- [ ] **Step 4: Update `Endpoint` to use the new types**

  ```rust
  #[derive(Debug, Deserialize)]
  #[serde(deny_unknown_fields)]
  pub struct Endpoint {
      pub base_url: SecretSource,
      pub api_format: ApiFormat,
      pub provider: Option<Provider>,   // was: Option<String>
      pub gateway: Option<Gateway>,     // was: Option<String>
  }
  ```

- [ ] **Step 5: Fix `assert_standard` for the provider field**

  In `mod tests`, find the provider assertion in `assert_standard` and update it:

  ```rust
  // Remove:
  assert_eq!(cfg.endpoint.provider.as_deref(), Some("litellm"));
  // Add:
  assert_eq!(
      cfg.endpoint.provider,
      Some(Provider::Known(KnownProvider::LiteLlm))
  );
  ```

- [ ] **Step 6: Run tests — expect all pass**

  ```bash
  cargo test --manifest-path tools/aix/Cargo.toml
  ```

  Expected: all tests pass.

- [ ] **Step 7: Commit**

  ```bash
  git add tools/aix/src/config.rs
  git commit -m "feat(config): add Provider and Gateway open enums"
  ```

---

## Task 3: Remove `Compat`, wire `collect_vars` to `ApiFormat`

**Files:**
- Modify: `tools/aix/src/config.rs`
- Modify: `tools/aix/src/commands/env.rs`

- [ ] **Step 1: Write failing tests for `collect_vars` with `ApiFormat`**

  In `tools/aix/src/commands/env.rs`, inside `mod tests`, replace the existing compat helper functions and their three tests with these:

  ```rust
  use crate::config::ApiFormat;

  // --- collect_vars with ApiFormat ---

  #[test]
  fn collect_vars_anthropic_produces_5_vars() {
      let vars = collect_vars("swtb", "sk-key", "https://example.com", &ApiFormat::Anthropic);
      assert_eq!(vars.len(), 5);
      assert_eq!(vars[0], ("AIX_PROFILE", "swtb".to_string()));
      assert_eq!(vars[1], ("AIX_API_KEY", "sk-key".to_string()));
      assert_eq!(vars[2], ("AIX_BASE_URL", "https://example.com".to_string()));
      assert_eq!(vars[3], ("ANTHROPIC_API_KEY", "sk-key".to_string()));
      assert_eq!(vars[4], ("ANTHROPIC_BASE_URL", "https://example.com".to_string()));
  }

  #[test]
  fn collect_vars_anthropic_has_no_openai_vars() {
      let vars = collect_vars("p", "k", "u", &ApiFormat::Anthropic);
      let names: Vec<&str> = vars.iter().map(|(k, _)| *k).collect();
      assert!(!names.contains(&"OPENAI_API_KEY"));
      assert!(!names.contains(&"OPENAI_BASE_URL"));
  }

  #[test]
  fn collect_vars_openai_produces_5_vars() {
      let vars = collect_vars("swtb", "sk-key", "https://example.com", &ApiFormat::OpenAi);
      assert_eq!(vars.len(), 5);
      assert_eq!(vars[3], ("OPENAI_API_KEY", "sk-key".to_string()));
      assert_eq!(vars[4], ("OPENAI_BASE_URL", "https://example.com".to_string()));
  }

  #[test]
  fn collect_vars_openai_has_no_anthropic_vars() {
      let vars = collect_vars("p", "k", "u", &ApiFormat::OpenAi);
      let names: Vec<&str> = vars.iter().map(|(k, _)| *k).collect();
      assert!(!names.contains(&"ANTHROPIC_API_KEY"));
      assert!(!names.contains(&"ANTHROPIC_BASE_URL"));
  }

  #[test]
  fn collect_vars_both_produces_7_vars() {
      let vars = collect_vars("swtb", "sk-key", "https://example.com", &ApiFormat::Both);
      assert_eq!(vars.len(), 7);
      assert_eq!(vars[3], ("ANTHROPIC_API_KEY", "sk-key".to_string()));
      assert_eq!(vars[4], ("ANTHROPIC_BASE_URL", "https://example.com".to_string()));
      assert_eq!(vars[5], ("OPENAI_API_KEY", "sk-key".to_string()));
      assert_eq!(vars[6], ("OPENAI_BASE_URL", "https://example.com".to_string()));
  }

  #[test]
  fn collect_vars_always_emits_aix_base_vars() {
      for fmt in [ApiFormat::Anthropic, ApiFormat::OpenAi, ApiFormat::Both] {
          let vars = collect_vars("p", "k", "u", &fmt);
          let names: Vec<&str> = vars.iter().map(|(k, _)| *k).collect();
          assert!(names.contains(&"AIX_PROFILE"), "missing for {fmt:?}");
          assert!(names.contains(&"AIX_API_KEY"), "missing for {fmt:?}");
          assert!(names.contains(&"AIX_BASE_URL"), "missing for {fmt:?}");
      }
  }
  ```

  Remove these from `mod tests`:
  - The `fn default_compat() -> Compat { ... }` helper
  - The `fn full_compat() -> Compat { ... }` helper
  - The `fn minimal_compat() -> Compat { ... }` helper
  - The `collect_vars_default_compat_produces_5_vars` test
  - The `collect_vars_full_compat_produces_7_vars` test
  - The `collect_vars_minimal_compat_produces_3_vars` test
  - The `use crate::config::Compat;` import (replace with `use crate::config::ApiFormat;`)

- [ ] **Step 2: Run tests — expect compile error**

  ```bash
  cargo test --manifest-path tools/aix/Cargo.toml 2>&1 | head -30
  ```

  Expected: compile error because `collect_vars` still takes `&Compat` but tests pass `&ApiFormat`.

- [ ] **Step 3: Update `collect_vars` signature and body**

  In `tools/aix/src/commands/env.rs`, replace the `collect_vars` function:

  ```rust
  pub(crate) fn collect_vars(
      profile_name: &str,
      api_key: &str,
      base_url: &str,
      api_format: &config::ApiFormat,
  ) -> Vec<(&'static str, String)> {
      use config::ApiFormat;
      let mut vars = vec![
          ("AIX_PROFILE", profile_name.to_string()),
          ("AIX_API_KEY", api_key.to_string()),
          ("AIX_BASE_URL", base_url.to_string()),
      ];
      let emit_anthropic = matches!(api_format, ApiFormat::Anthropic | ApiFormat::Both);
      let emit_openai = matches!(api_format, ApiFormat::OpenAi | ApiFormat::Both);
      if emit_anthropic {
          vars.push(("ANTHROPIC_API_KEY", api_key.to_string()));
          vars.push(("ANTHROPIC_BASE_URL", base_url.to_string()));
      }
      if emit_openai {
          vars.push(("OPENAI_API_KEY", api_key.to_string()));
          vars.push(("OPENAI_BASE_URL", base_url.to_string()));
      }
      vars
  }
  ```

- [ ] **Step 4: Update the call site in `run()`**

  In `tools/aix/src/commands/env.rs`, find the `collect_vars` call in `run()` and change the last argument:

  ```rust
  // Remove:
  let vars = collect_vars(
      &profile_name,
      api_key.expose_secret(),
      base_url.expose_secret(),
      &cfg.compat,
  );
  // Add:
  let vars = collect_vars(
      &profile_name,
      api_key.expose_secret(),
      base_url.expose_secret(),
      &cfg.endpoint.api_format,
  );
  ```

- [ ] **Step 5: Remove `Compat` from `config.rs`**

  In `tools/aix/src/config.rs`:

  Delete the entire `Compat` struct and its `Default` impl:
  ```rust
  // DELETE this entire block:
  fn default_true() -> bool {
      true
  }

  #[derive(Debug, Deserialize)]
  #[serde(deny_unknown_fields)]
  pub struct Compat {
      #[serde(default = "default_true")]
      pub anthropic_env: bool,
      #[serde(default)]
      pub openai_env: bool,
  }

  impl Default for Compat {
      fn default() -> Self {
          Self {
              anthropic_env: true,
              openai_env: false,
          }
      }
  }
  ```

  Remove the `compat` field from `Config`:
  ```rust
  // Remove this line from Config:
  pub compat: Compat,
  ```

  Delete the three compat tests from `mod tests` in `config.rs`:
  - `compat_defaults_to_anthropic_true_openai_false`
  - `compat_can_be_overridden`
  - `compat_rejects_unknown_fields`

  Also delete or update any `Config { ..., compat: ..., }` struct literals in the test helpers that construct `Config` manually (the `load_env_files_*` tests). Remove the `compat: ...` field from those literals.

  Add a test that `[compat]` in config now causes a parse error:
  ```rust
  #[test]
  fn compat_section_now_rejected() {
      let bad = r#"
  [compat]
  anthropic_env = true

  [endpoint]
  base_url = { env = "X" }
  api_format = "anthropic"

  [profiles.work]
  api_key = "sk-test"
  "#;
      assert!(toml::from_str::<Config>(bad).is_err());
  }
  ```

- [ ] **Step 6: Run tests — expect all pass**

  ```bash
  cargo test --manifest-path tools/aix/Cargo.toml
  ```

  Expected: all tests pass, no warnings about unused `Compat` or `default_true`.

- [ ] **Step 7: Run fmt and clippy**

  ```bash
  cargo fmt --manifest-path tools/aix/Cargo.toml
  cargo clippy --manifest-path tools/aix/Cargo.toml --all-targets -- -D warnings
  ```

  Expected: no warnings, no errors.

- [ ] **Step 8: Commit**

  ```bash
  git add tools/aix/src/config.rs tools/aix/src/commands/env.rs
  git commit -m "feat(endpoint): replace Compat with ApiFormat, wire collect_vars"
  ```

---

## Task 4: Doc comment and example config

**Files:**
- Modify: `tools/aix/src/config.rs`
- Create: `docs/aix/example-config.toml`

- [ ] **Step 1: Add doc comment above `Endpoint` in `config.rs`**

  Replace the bare `#[derive(...)]` line above `Endpoint` with:

  ```rust
  // The gateway is a LiteLLM proxy that speaks the Anthropic Messages
  // API (and optionally OpenAI Chat Completions) but is not api.anthropic.com.
  // `api_format` declares which SDK env vars to emit so downstream tools
  // (claude, pi, etc.) reach the gateway without extra per-tool configuration.
  #[derive(Debug, Deserialize)]
  #[serde(deny_unknown_fields)]
  pub struct Endpoint {
  ```

- [ ] **Step 2: Create `docs/aix/example-config.toml`**

  ```toml
  # aix example configuration
  # Copy to ~/.config/aix/aix.toml and fill in your values.

  # Optional: profile selected when no --profile flag is given and stdin is
  # not a TTY (e.g. in scripts). Remove to always prompt interactively.
  default_profile = "work"

  # Optional: .env files loaded before the CLI runs. The real process
  # environment always wins — these files only fill in missing vars.
  # env_files = ["~/.config/aix/secrets.env"]

  # ---------------------------------------------------------------------------
  # Endpoint — describes the AI gateway this config targets
  # ---------------------------------------------------------------------------

  [endpoint]
  # provider and gateway are metadata; they do not affect runtime behavior
  # but document what the gateway is for future operators.
  provider = "litellm"         # KnownProvider: "litellm"; or any string
  gateway  = "litellm"         # KnownGateway:  "litellm";       or any string

  # api_format controls which SDK-compatible env vars are emitted.
  #
  #   "anthropic"  →  ANTHROPIC_API_KEY + ANTHROPIC_BASE_URL   (5 vars total)
  #   "openai"     →  OPENAI_API_KEY    + OPENAI_BASE_URL      (5 vars total)
  #   "both"       →  both pairs above                          (7 vars total)
  #
  # AIX_PROFILE, AIX_API_KEY, AIX_BASE_URL are always emitted.
  # Use "anthropic" for tools that read ANTHROPIC_* (claude, pi).
  # Use "openai"    for tools that read OPENAI_*    (openai-python, LangChain).
  # Use "both"      when you run both kinds of tools from one shell session.
  api_format = "anthropic"

  # Where to find the gateway URL. Supported secret sources:
  #   { env  = "VAR_NAME" }           — read from environment variable
  #   { file = "/run/secrets/url" }   — read from file (trailing newline stripped)
  #   { command = "op read ..." }     — stdout of a command
  #   "https://gateway.example.com"   — literal value (avoid for secrets)
  base_url = { env = "AIX_BASE_URL" }

  # ---------------------------------------------------------------------------
  # Profiles — named sets of credentials
  # ---------------------------------------------------------------------------

  [profiles.work]
  label   = "Work"                        # shown in interactive picker
  api_key = { env = "AIX_API_KEY" }

  [profiles.local]
  label   = "Local (no auth)"
  api_key = "sk-no-key"                   # literal — fine for a local dev gateway

  # [profiles.personal]
  # label   = "Personal"
  # api_key = { file = "/run/secrets/aix/personal" }
  ```

- [ ] **Step 3: Run tests one final time**

  ```bash
  cargo test --manifest-path tools/aix/Cargo.toml
  ```

  Expected: all tests pass.

- [ ] **Step 4: Commit**

  ```bash
  git add tools/aix/src/config.rs docs/aix/example-config.toml
  git commit -m "docs(config): add Endpoint doc comment and example-config.toml"
  ```
