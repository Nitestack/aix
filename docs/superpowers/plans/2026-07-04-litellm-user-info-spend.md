# LiteLLM user info and spend commands implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `aix info [profile] [--json]` and `aix spend [profile] [--json] [--limit N]` commands that query LiteLLM's `/user/info` and `/spend/logs` endpoints using the active profile's credentials.

**Architecture:** A new `src/client.rs` module holds `LiteLlmClient` (reqwest-backed async HTTP client). Two new command modules (`info.rs`, `spend.rs`) resolve the profile, check `gateway == litellm`, call the client, and format output. `main()` becomes async via `#[tokio::main]`.

**Tech Stack:** Rust, reqwest 0.12 (rustls-tls), tokio 1.x, wiremock 0.6 (dev dep for HTTP mocking)

---

## File map

| Action | Path | Responsibility |
|---|---|---|
| Modify | `Cargo.toml` | Add reqwest, tokio; add wiremock dev dep |
| Modify | `src/main.rs` | `#[tokio::main]`, async `run()`, dispatch `Info`/`Spend` |
| Modify | `src/cli.rs` | Add `Info` and `Spend` variants to `Command` |
| Modify | `src/error.rs` | Add `NotLiteLlm`, `GatewayError`, `HttpError` variants |
| Create | `src/client.rs` | `LiteLlmClient` struct + `user_info()`, `spend_logs()` |
| Create | `src/commands/info.rs` | `aix info` handler |
| Create | `src/commands/spend.rs` | `aix spend` handler |
| Modify | `src/commands/mod.rs` | Pub mod info; pub mod spend |
| Create | `tests/info_test.rs` | Integration tests for `aix info` |
| Create | `tests/spend_test.rs` | Integration tests for `aix spend` |

---

## Task 1: Add dependencies and make main async

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/main.rs`

- [ ] **Step 1: Add dependencies to `Cargo.toml`**

In `[dependencies]` add:
```toml
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
tokio   = { version = "1", features = ["macros", "rt-multi-thread"] }
```

In `[dev-dependencies]` add:
```toml
wiremock = "0.6"
```

- [ ] **Step 2: Replace `src/main.rs` to make it async**

The full new content of `src/main.rs`:

```rust
use clap::Parser;
use cli::{Cli, Command};
use config::ApiFormat;

mod cli;
mod commands;
mod config;
mod error;
mod secrets;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    run(cli).await
}

async fn run(cli: Cli) -> color_eyre::Result<()> {
    let global_profile = cli.profile;
    let config_path = cli.config;
    match cli.command {
        Command::Profiles { json } => commands::profiles::run(config_path, json),
        Command::Env { profile, format } => {
            let effective_profile = profile.or(global_profile);
            commands::env::run(effective_profile, config_path, format)
        }
        Command::Shell {
            profile,
            dry_run,
            extra_args,
        } => {
            if !extra_args.is_empty() {
                return Err(color_eyre::eyre::eyre!(
                    "`aix shell` does not accept commands after --; use `aix exec` to run a command directly"
                ));
            }
            let effective_profile = profile.or(global_profile);
            commands::shell::run(effective_profile, config_path, dry_run)
        }
        Command::Exec {
            profile,
            dry_run,
            args,
        } => {
            let effective_profile = profile.or(global_profile);
            commands::exec::run(effective_profile, config_path, dry_run, args)
        }
        Command::Config { action } => commands::config::run(action, config_path),
        Command::Info { profile, json } => {
            let effective_profile = profile.or(global_profile);
            commands::info::run(effective_profile, config_path, json).await
        }
        Command::Spend { profile, json, limit } => {
            let effective_profile = profile.or(global_profile);
            commands::spend::run(effective_profile, config_path, json, limit).await
        }
        Command::Tool(raw) => {
            let tool = raw[0].clone();
            let rest = &raw[1..];

            let sep = rest.iter().position(|a| a == "--");
            let (pre, tool_args) = match sep {
                Some(i) => (&rest[..i], rest[i + 1..].to_vec()),
                None => (rest, vec![]),
            };

            let dry_run = pre.iter().any(|a| a == "--dry-run");
            let profile = pre.iter().find(|a| !a.starts_with('-')).cloned();

            let format = if tool == "claude" {
                ApiFormat::Anthropic
            } else {
                ApiFormat::OpenAi
            };

            commands::launch::run_named_tool(
                &tool,
                format,
                profile.or(global_profile),
                config_path,
                dry_run,
                tool_args,
            )
        }
    }
}
```

Note: `commands::info` and `commands::spend` don't exist yet (and `mod client;` is not yet declared) — `cargo check` will error on those until Tasks 3 and 4. That's expected.

- [ ] **Step 3: Verify dependency resolution**

```bash
cargo check 2>&1 | grep -E "^error" | head -5
```

Expected: errors about `commands::info` and `commands::spend` not found, nothing about syntax or Cargo.toml.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs
git commit -m "chore: add reqwest + tokio; make main async"
```

---

## Task 2: Add error variants

**Files:**
- Modify: `src/error.rs`

- [ ] **Step 1: Write failing tests for the new error messages**

Add to the `#[cfg(test)]` block in `src/error.rs`:

```rust
#[test]
fn not_litellm_message_mentions_litellm_and_gateway() {
    let e = AixError::NotLiteLlm;
    let msg = e.to_string();
    assert!(msg.contains("litellm"), "got: {msg}");
    assert!(msg.contains("gateway"), "got: {msg}");
}

#[test]
fn gateway_error_message_includes_status_and_body() {
    let e = AixError::GatewayError {
        status: 403,
        body: "Forbidden".to_string(),
    };
    let msg = e.to_string();
    assert!(msg.contains("403"), "got: {msg}");
    assert!(msg.contains("Forbidden"), "got: {msg}");
}
```

- [ ] **Step 2: Run tests to verify they fail to compile**

```bash
cargo test -- not_litellm_message_mentions_litellm_and_gateway gateway_error_message 2>&1 | tail -10
```

Expected: compile error — `AixError::NotLiteLlm` does not exist yet.

- [ ] **Step 3: Add the three new variants to `src/error.rs`**

Add after the `ExecutableNotFound` variant (before the closing `}` of the enum):

```rust
    #[error("this command requires gateway = \"litellm\" in your endpoint config")]
    NotLiteLlm,

    #[error("gateway returned HTTP {status}: {body}")]
    GatewayError { status: u16, body: String },

    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -- not_litellm_message_mentions_litellm_and_gateway gateway_error_message
```

Expected: both PASS.

- [ ] **Step 5: Commit**

```bash
git add src/error.rs
git commit -m "feat(error): add NotLiteLlm, GatewayError, HttpError variants"
```

---

## Task 3: Implement `src/client.rs`

**Files:**
- Create: `src/client.rs`

- [ ] **Step 1: Create `src/client.rs` with stubs and tests**

```rust
use crate::error::AixError;

pub struct LiteLlmClient {
    base_url: String,
    api_key: String,
    inner: reqwest::Client,
}

impl LiteLlmClient {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            inner: reqwest::Client::new(),
        }
    }

    pub async fn user_info(&self) -> Result<serde_json::Value, AixError> {
        todo!()
    }

    pub async fn spend_logs(&self, limit: u32) -> Result<serde_json::Value, AixError> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn user_info_calls_correct_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/info"))
            .and(header("Authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "user_id": "u123",
                "spend": 1.23
            })))
            .mount(&server)
            .await;

        let client = LiteLlmClient::new(server.uri(), "test-key");
        let result = client.user_info().await.unwrap();
        assert_eq!(result["user_id"], "u123");
        assert_eq!(result["spend"], 1.23);
    }

    #[tokio::test]
    async fn spend_logs_calls_correct_endpoint_with_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/spend/logs"))
            .and(header("Authorization", "Bearer test-key"))
            .and(query_param("limit", "25"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"model": "gpt-4", "spend": 0.01}
            ])))
            .mount(&server)
            .await;

        let client = LiteLlmClient::new(server.uri(), "test-key");
        let result = client.spend_logs(25).await.unwrap();
        assert_eq!(result[0]["model"], "gpt-4");
    }

    #[tokio::test]
    async fn non_200_response_returns_gateway_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/info"))
            .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
            .mount(&server)
            .await;

        let client = LiteLlmClient::new(server.uri(), "test-key");
        let err = client.user_info().await.unwrap_err();
        assert!(matches!(err, AixError::GatewayError { status: 403, .. }));
    }

    #[tokio::test]
    async fn base_url_trailing_slash_is_normalized() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;

        let uri_with_slash = format!("{}/", server.uri());
        let client = LiteLlmClient::new(uri_with_slash, "k");
        client.user_info().await.unwrap();
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test client::tests 2>&1 | tail -15
```

Expected: FAIL — `todo!()` panics on every test.

- [ ] **Step 3: Implement `user_info` and `spend_logs`**

Replace the two `todo!()` bodies:

```rust
pub async fn user_info(&self) -> Result<serde_json::Value, AixError> {
    let url = format!("{}/user/info", self.base_url);
    let resp = self
        .inner
        .get(&url)
        .header("Authorization", format!("Bearer {}", self.api_key))
        .send()
        .await?;
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AixError::GatewayError { status, body });
    }
    Ok(resp.json().await?)
}

pub async fn spend_logs(&self, limit: u32) -> Result<serde_json::Value, AixError> {
    let url = format!("{}/spend/logs", self.base_url);
    let resp = self
        .inner
        .get(&url)
        .header("Authorization", format!("Bearer {}", self.api_key))
        .query(&[("limit", limit.to_string())])
        .send()
        .await?;
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AixError::GatewayError { status, body });
    }
    Ok(resp.json().await?)
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test client::tests
```

Expected: all 4 tests PASS.

- [ ] **Step 5: Add `mod client;` to `src/main.rs`**

Add `mod client;` after the existing `mod cli;` line in `src/main.rs`:

```rust
mod cli;
mod client;
mod commands;
```

- [ ] **Step 6: Verify the project compiles**

```bash
cargo check 2>&1 | grep -c "^error"
```

Expected: errors only about `commands::info` and `commands::spend` (not yet created) — the same as after Task 1. No new errors about `client`.

- [ ] **Step 7: Commit**

```bash
git add src/client.rs src/main.rs
git commit -m "feat(client): add LiteLlmClient with user_info and spend_logs"
```

---

## Task 4: Add CLI variants and stub command modules

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/commands/mod.rs`
- Create: `src/commands/info.rs` (stub)
- Create: `src/commands/spend.rs` (stub)

- [ ] **Step 1: Add `Info` and `Spend` to `Command` in `src/cli.rs`**

Add after the `Config` variant and before `Tool`:

```rust
    /// Show user info and budget for the selected profile (LiteLLM only)
    Info {
        /// Profile name (positional; overrides the global --profile flag)
        profile: Option<String>,
        /// Output raw JSON instead of formatted text
        #[arg(long)]
        json: bool,
    },
    /// Show recent spend logs for the selected profile (LiteLLM only)
    Spend {
        /// Profile name (positional; overrides the global --profile flag)
        profile: Option<String>,
        /// Output raw JSON instead of formatted text
        #[arg(long)]
        json: bool,
        /// Maximum number of log entries to return
        #[arg(long, default_value = "50")]
        limit: u32,
    },
```

- [ ] **Step 2: Update `src/commands/mod.rs`**

```rust
pub mod config;
pub mod env;
pub mod exec;
pub mod info;
pub mod launch;
pub mod profiles;
pub mod shell;
pub mod spend;
```

- [ ] **Step 3: Create stub `src/commands/info.rs`**

```rust
use color_eyre::Result;
use std::path::PathBuf;

pub async fn run(
    _profile: Option<String>,
    _config_path: Option<PathBuf>,
    _json: bool,
) -> Result<()> {
    todo!("info not yet implemented")
}
```

- [ ] **Step 4: Create stub `src/commands/spend.rs`**

```rust
use color_eyre::Result;
use std::path::PathBuf;

pub async fn run(
    _profile: Option<String>,
    _config_path: Option<PathBuf>,
    _json: bool,
    _limit: u32,
) -> Result<()> {
    todo!("spend not yet implemented")
}
```

- [ ] **Step 5: Verify the project builds**

```bash
cargo build 2>&1 | tail -5
```

Expected: builds successfully.

- [ ] **Step 6: Verify help output shows new subcommands**

```bash
cargo run -- --help 2>&1 | grep -E "info|spend"
```

Expected: both `info` and `spend` appear in the list.

- [ ] **Step 7: Commit**

```bash
git add src/cli.rs src/commands/mod.rs src/commands/info.rs src/commands/spend.rs
git commit -m "feat(cli): add Info and Spend command variants (stubs)"
```

---

## Task 5: Implement `aix info`

**Files:**
- Modify: `src/commands/info.rs`
- Create: `tests/info_test.rs`

`resolve_profile` is `pub(crate)` in `src/commands/env.rs` — import it directly.
`Gateway` and `KnownGateway` are defined in `src/config.rs`.

- [ ] **Step 1: Create `tests/info_test.rs` with integration tests**

```rust
use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use predicates::prelude::*;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn write_config(dir: &TempDir, base_url: &str) -> std::path::PathBuf {
    let file = dir.child("aix.toml");
    file.write_str(&format!(
        r#"
[endpoint]
base_url = "{base_url}"
gateway = "litellm"

[profiles.test]
api_key = "sk-test-key"
"#
    ))
    .unwrap();
    file.path().to_path_buf()
}

#[tokio::test]
async fn info_shows_user_id_and_spend() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/info"))
        .and(header("Authorization", "Bearer sk-test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "user_id": "user-abc",
            "spend": 2.50,
            "max_budget": 10.0,
            "keys": [{"key": "sk-test-key"}]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    Command::cargo_bin("aix")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "info", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("user-abc"))
        .stdout(predicate::str::contains("2.50"));
}

#[tokio::test]
async fn info_json_flag_returns_raw_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"user_id": "u1"})))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    let output = Command::cargo_bin("aix")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "info", "test", "--json"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["user_id"], "u1");
}

#[test]
fn info_wrong_gateway_errors_with_litellm_hint() {
    let dir = TempDir::new().unwrap();
    let file = dir.child("aix.toml");
    file.write_str(
        r#"
[endpoint]
base_url = "https://example.com"
gateway = "my-other-gateway"

[profiles.test]
api_key = "sk-test"
"#,
    )
    .unwrap();

    Command::cargo_bin("aix")
        .unwrap()
        .args(["--config", file.path().to_str().unwrap(), "info", "test"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("litellm"));
}

#[test]
fn info_no_gateway_set_errors_with_litellm_hint() {
    let dir = TempDir::new().unwrap();
    let file = dir.child("aix.toml");
    file.write_str(
        r#"
[endpoint]
base_url = "https://example.com"

[profiles.test]
api_key = "sk-test"
"#,
    )
    .unwrap();

    Command::cargo_bin("aix")
        .unwrap()
        .args(["--config", file.path().to_str().unwrap(), "info", "test"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("litellm"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test --test info_test 2>&1 | tail -15
```

Expected: FAIL — `todo!()` panic.

- [ ] **Step 3: Implement `src/commands/info.rs`**

```rust
use crate::client::LiteLlmClient;
use crate::commands::env::resolve_profile;
use crate::config::{self, Gateway, KnownGateway};
use crate::error::AixError;
use color_eyre::Result;
use std::path::PathBuf;

pub async fn run(
    positional_profile: Option<String>,
    config_path: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    let path = config::find_config_path(config_path.as_deref())?.ok_or(AixError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;
    config::load_env_files(&cfg)?;

    if !is_litellm(&cfg) {
        return Err(AixError::NotLiteLlm.into());
    }

    let profile_name = resolve_profile(positional_profile, &cfg)?;
    let profile = cfg
        .profiles
        .get(&profile_name)
        .ok_or_else(|| AixError::ProfileNotFound {
            name: profile_name.clone(),
            available_hint: config::format_available_profiles(&cfg),
        })?;

    let base_url = cfg.endpoint.base_url.resolve()?;
    let api_key = profile.api_key.resolve()?;

    let client = LiteLlmClient::new(base_url.expose_secret(), api_key.expose_secret());
    let data = client.user_info().await?;

    if json {
        let mut out = serde_json::to_string_pretty(&data)?;
        out.push('\n');
        print!("{out}");
    } else {
        print_human(&data);
    }
    Ok(())
}

fn is_litellm(cfg: &config::Config) -> bool {
    matches!(
        cfg.endpoint.gateway,
        Some(Gateway::Known(KnownGateway::Litellm))
    )
}

fn print_human(data: &serde_json::Value) {
    if let Some(id) = data.get("user_id").and_then(|v| v.as_str()) {
        println!("User ID:  {id}");
    }
    if let Some(teams) = data.get("teams").and_then(|v| v.as_array()) {
        let names: Vec<&str> = teams
            .iter()
            .filter_map(|t| t.get("team_alias").and_then(|v| v.as_str()))
            .collect();
        if !names.is_empty() {
            println!("Teams:    {}", names.join(", "));
        }
    }
    if let Some(spend) = data.get("spend").and_then(|v| v.as_f64()) {
        if let Some(budget) = data.get("max_budget").and_then(|v| v.as_f64()) {
            let remaining = budget - spend;
            println!("Spend:    ${spend:.4}  /  ${budget:.2} budget  (${remaining:.4} remaining)");
        } else {
            println!("Spend:    ${spend:.4}");
        }
    }
    if let Some(keys) = data.get("keys").and_then(|v| v.as_array()) {
        println!("Keys:     {}", keys.len());
    }
}
```

- [ ] **Step 4: Run info tests**

```bash
cargo test --test info_test
```

Expected: all 4 tests PASS.

- [ ] **Step 5: Run full suite to check for regressions**

```bash
cargo test
```

Expected: all tests PASS.

- [ ] **Step 6: Commit**

```bash
git add src/commands/info.rs tests/info_test.rs
git commit -m "feat(cmd): implement aix info"
```

---

## Task 6: Implement `aix spend`

**Files:**
- Modify: `src/commands/spend.rs`
- Create: `tests/spend_test.rs`

- [ ] **Step 1: Create `tests/spend_test.rs` with integration tests**

```rust
use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use predicates::prelude::*;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn write_config(dir: &TempDir, base_url: &str) -> std::path::PathBuf {
    let file = dir.child("aix.toml");
    file.write_str(&format!(
        r#"
[endpoint]
base_url = "{base_url}"
gateway = "litellm"

[profiles.test]
api_key = "sk-test-key"
"#
    ))
    .unwrap();
    file.path().to_path_buf()
}

#[tokio::test]
async fn spend_shows_model_and_cost() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/spend/logs"))
        .and(header("Authorization", "Bearer sk-test-key"))
        .and(query_param("limit", "50"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "request_id": "req-abc123-xyz",
                "model": "gpt-4o",
                "spend": 0.0042,
                "startTime": "2026-07-04T10:00:00Z"
            }
        ])))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    Command::cargo_bin("aix")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "spend", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("gpt-4o"))
        .stdout(predicate::str::contains("0.0042"));
}

#[tokio::test]
async fn spend_limit_flag_forwarded_as_query_param() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/spend/logs"))
        .and(query_param("limit", "10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "spend",
            "test",
            "--limit",
            "10",
        ])
        .assert()
        .success();
}

#[tokio::test]
async fn spend_json_flag_returns_raw_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/spend/logs"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([{"model": "claude-sonnet-4-6"}])),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    let output = Command::cargo_bin("aix")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "spend",
            "test",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed[0]["model"], "claude-sonnet-4-6");
}

#[tokio::test]
async fn spend_empty_logs_prints_no_spend_logs() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/spend/logs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri());

    Command::cargo_bin("aix")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "spend", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no spend logs"));
}

#[test]
fn spend_no_gateway_errors_with_litellm_hint() {
    let dir = TempDir::new().unwrap();
    let file = dir.child("aix.toml");
    file.write_str(
        r#"
[endpoint]
base_url = "https://example.com"

[profiles.test]
api_key = "sk-test"
"#,
    )
    .unwrap();

    Command::cargo_bin("aix")
        .unwrap()
        .args(["--config", file.path().to_str().unwrap(), "spend", "test"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("litellm"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test --test spend_test 2>&1 | tail -15
```

Expected: FAIL — `todo!()` panic.

- [ ] **Step 3: Implement `src/commands/spend.rs`**

```rust
use crate::client::LiteLlmClient;
use crate::commands::env::resolve_profile;
use crate::config::{self, Gateway, KnownGateway};
use crate::error::AixError;
use color_eyre::Result;
use std::path::PathBuf;

pub async fn run(
    positional_profile: Option<String>,
    config_path: Option<PathBuf>,
    json: bool,
    limit: u32,
) -> Result<()> {
    let path = config::find_config_path(config_path.as_deref())?.ok_or(AixError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;
    config::load_env_files(&cfg)?;

    if !is_litellm(&cfg) {
        return Err(AixError::NotLiteLlm.into());
    }

    let profile_name = resolve_profile(positional_profile, &cfg)?;
    let profile = cfg
        .profiles
        .get(&profile_name)
        .ok_or_else(|| AixError::ProfileNotFound {
            name: profile_name.clone(),
            available_hint: config::format_available_profiles(&cfg),
        })?;

    let base_url = cfg.endpoint.base_url.resolve()?;
    let api_key = profile.api_key.resolve()?;

    let client = LiteLlmClient::new(base_url.expose_secret(), api_key.expose_secret());
    let data = client.spend_logs(limit).await?;

    if json {
        let mut out = serde_json::to_string_pretty(&data)?;
        out.push('\n');
        print!("{out}");
    } else {
        print_human(&data);
    }
    Ok(())
}

fn is_litellm(cfg: &config::Config) -> bool {
    matches!(
        cfg.endpoint.gateway,
        Some(Gateway::Known(KnownGateway::Litellm))
    )
}

fn print_human(data: &serde_json::Value) {
    let entries = match data.as_array() {
        Some(arr) => arr,
        None => {
            println!("(no spend data)");
            return;
        }
    };
    if entries.is_empty() {
        println!("(no spend logs)");
        return;
    }
    println!("{:<30}  {:<25}  {:>12}", "Time", "Model", "Cost ($)");
    println!("{}", "-".repeat(72));
    for entry in entries {
        let time = entry
            .get("startTime")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let model = entry
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let cost = entry
            .get("spend")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let req_id = entry
            .get("request_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let short_id = if req_id.len() > 8 { &req_id[..8] } else { req_id };
        println!("{time:<30}  {model:<25}  {cost:>12.6}  [{short_id}]");
    }
}
```

- [ ] **Step 4: Run spend tests**

```bash
cargo test --test spend_test
```

Expected: all 5 tests PASS.

- [ ] **Step 5: Run full suite with fmt and clippy**

```bash
cargo test && cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

Expected: all tests PASS, zero clippy warnings.

- [ ] **Step 6: Commit**

```bash
git add src/commands/spend.rs tests/spend_test.rs
git commit -m "feat(cmd): implement aix spend"
```
