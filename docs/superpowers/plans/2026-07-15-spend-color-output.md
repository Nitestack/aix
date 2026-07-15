# Spend Color Output Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Color the `aix spend` usage bar and `"NN% used"` label based on a 4-tier
green/yellow/orange/red usage-to-budget ratio, auto-disabled on non-TTY output or `NO_COLOR`.

**Architecture:** Add the `owo-colors` crate (with its `supports-colors` feature, which
handles TTY + `NO_COLOR` detection internally). Add a pure `tier_color(pct_used: f64) -> Rgb`
function to `src/commands/spend.rs`, then wrap the bar and label strings in `print_human`
with `if_supports_color`.

**Tech Stack:** Rust, `owo-colors` v4.

## Global Constraints

- Only `src/commands/spend.rs` and `Cargo.toml` are touched — per `docs/superpowers/specs/2026-07-15-spend-color-output-design.md`, no config changes, no new CLI flags.
- Color tiers: `< 50%` green, `50–74%` yellow, `75–99%` orange (`Rgb(255,165,0)`), `>= 100%` red.
- Only the filled bar (`█`) and the `"{pct_used:.0}% used"` label are colored. The dollar-amount line and the `░` empty bar portion stay uncolored.
- `owo-colors` must be added with `features = ["supports-colors"]` so color auto-disables on non-TTY stdout or when `NO_COLOR` is set — no manual detection code.
- `cargo fmt --all` and `cargo clippy --all-targets -- -D warnings` must pass before any commit (per `.claude/rules/aix-rust.md`).
- Do not touch files outside `src/`, `tests/`, `Cargo.toml` (per root `CLAUDE.md`).

---

### Task 1: Add `owo-colors` dependency and `tier_color` helper

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/commands/spend.rs` (add `tier_color` function + its unit tests, near the bottom of the file alongside `format_age`'s existing test module — check if one exists first; if not, add a new `#[cfg(test)] mod tests` block)

**Interfaces:**
- Produces: `fn tier_color(pct_used: f64) -> owo_colors::Rgb` — pure function, no I/O. Later tasks (Task 2) call this directly.

- [ ] **Step 1: Add the dependency**

Edit `Cargo.toml`, in the `[dependencies]` table, add (keep the existing alignment style of the table):

```toml
owo-colors  = { version = "4", features = ["supports-colors"] }
```

- [ ] **Step 2: Write the failing unit tests for `tier_color`**

Append to `src/commands/spend.rs` (create the `#[cfg(test)] mod tests` block if the file doesn't already have one — it currently doesn't, based on the file as of this plan):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use owo_colors::Rgb;

    #[test]
    fn tier_color_boundaries() {
        assert_eq!(tier_color(0.0), Rgb(0, 200, 0));
        assert_eq!(tier_color(49.0), Rgb(0, 200, 0));
        assert_eq!(tier_color(50.0), Rgb(220, 200, 0));
        assert_eq!(tier_color(74.0), Rgb(220, 200, 0));
        assert_eq!(tier_color(75.0), Rgb(255, 165, 0));
        assert_eq!(tier_color(99.0), Rgb(255, 165, 0));
        assert_eq!(tier_color(100.0), Rgb(220, 0, 0));
        assert_eq!(tier_color(150.0), Rgb(220, 0, 0));
    }
}
```

Note: `owo_colors::Rgb` derives `PartialEq` and `Debug`, so direct `assert_eq!` works.

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test --lib tier_color_boundaries`
Expected: compile error — `tier_color` is not defined yet (or `Rgb` import unused if you added the function stub without a body; either way it must not pass yet).

- [ ] **Step 4: Implement `tier_color`**

Add this function to `src/commands/spend.rs`, above `fn print_human` (after the `use` statements at the top, add `use owo_colors::Rgb;`):

```rust
fn tier_color(pct_used: f64) -> Rgb {
    match pct_used {
        p if p < 50.0 => Rgb(0, 200, 0),
        p if p < 75.0 => Rgb(220, 200, 0),
        p if p < 100.0 => Rgb(255, 165, 0),
        _ => Rgb(220, 0, 0),
    }
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --lib tier_color_boundaries`
Expected: PASS (1 test run, 0 failed)

- [ ] **Step 6: Format, lint, commit**

Run:
```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```
Fix any warnings, then:
```bash
git add Cargo.toml Cargo.lock src/commands/spend.rs
git commit -m "feat(spend): add tier_color helper for usage-based coloring"
```

---

### Task 2: Wire `tier_color` into `print_human`'s bar and label

**Files:**
- Modify: `src/commands/spend.rs:129-143` (the `budget` branch of `print_human`, where `bar` and the final `println!("{bar}  {pct_used:.0}% used")` are built)
- Test: `tests/spend_test.rs` (add one integration test)

**Interfaces:**
- Consumes: `tier_color(pct_used: f64) -> owo_colors::Rgb` from Task 1.
- Produces: no new public interface — this is the terminal consumer of `tier_color`.

- [ ] **Step 1: Write the failing integration test**

Append to `tests/spend_test.rs`:

```rust
#[tokio::test]
async fn spend_output_has_no_ansi_codes_when_piped() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "spend": 90.0,
            "max_budget": 100.0,
            "keys": []
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let config = write_config(&dir, &server.uri(), "sk-colortest1");

    let output = Command::cargo_bin("aix")
        .unwrap()
        .env("AIX_CACHE_DIR", cache_dir.path())
        .args(["--config", config.to_str().unwrap(), "spend", "test"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    // assert_cmd captures stdout to a pipe, not a TTY, so owo-colors'
    // supports-colors detection must suppress ANSI escapes here.
    assert!(!stdout.contains('\x1b'), "expected no ANSI codes, got: {stdout:?}");
    assert!(stdout.contains("90% used"));
}
```

- [ ] **Step 2: Run the test to verify it currently passes (baseline) then verify the color change doesn't break it**

Run: `cargo test --test spend_test spend_output_has_no_ansi_codes_when_piped`
Expected: PASS even before Step 3's change (no coloring wired up yet, so obviously no ANSI codes). This confirms the test is meaningful once coloring is added — re-run after Step 3 to confirm it still passes with color code wired in but suppressed on non-TTY.

- [ ] **Step 3: Wire the color into `print_human`**

In `src/commands/spend.rs`, add to the top-level `use` statements:

```rust
use owo_colors::{OwoColorize, Stream::Stdout};
```

Replace the existing bar-building and final `println!` lines inside the `if let Some(b) = budget` block (currently):

```rust
        const BAR: usize = 40;
        let filled = (pct_used / 100.0 * BAR as f64).round() as usize;
        let bar = format!("[{}{}]", "█".repeat(filled), "░".repeat(BAR - filled));
```

...through...

```rust
        println!("{bar}  {pct_used:.0}% used");
```

with:

```rust
        const BAR: usize = 40;
        let filled = (pct_used / 100.0 * BAR as f64).round() as usize;
        let bar_filled = "█".repeat(filled);
        let bar_empty = "░".repeat(BAR - filled);
        let color = tier_color(pct_used);
        let bar_filled = bar_filled
            .if_supports_color(Stdout, |t| t.color(color))
            .to_string();
        let label = format!("{pct_used:.0}% used");
        let label = label
            .if_supports_color(Stdout, |t| t.color(color))
            .to_string();
```

And change the final `println!` (still inside the same `if let Some(b) = budget` block, after the `if remaining < 0.0 { ... } else { ... }` block) from:

```rust
        println!("{bar}  {pct_used:.0}% used");
```

to:

```rust
        println!("[{bar_filled}{bar_empty}]  {label}");
```

- [ ] **Step 4: Run the full test suite to verify everything passes**

Run: `cargo test`
Expected: all tests pass, including the pre-existing `spend_shows_matching_key_by_suffix` and `spend_falls_back_to_user_totals_when_no_key_match` tests (which assert on `"█"` and `"% used"` substrings — both still present, just wrapped in a no-op when non-TTY) and the new `spend_output_has_no_ansi_codes_when_piped` test.

- [ ] **Step 5: Manual verification on a real TTY**

Run in an actual terminal (not through a pipe or `assert_cmd`):

```bash
cargo build
./target/debug/aix spend --json '{"spend": 30, "max_budget": 100}' 2>/dev/null || true
```

(If there's no quick way to fake a live response, instead point `--config` at a temp config file pointing to a local mock, or just trust the integration test plus a manual run against a real configured profile if available.) Confirm visually:
- Bar + `"NN% used"` render in green when usage < 50%.
- Run `NO_COLOR=1 aix spend <profile>` and confirm no color escapes appear even on a real TTY.

- [ ] **Step 6: Format, lint, commit**

Run:
```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```
Fix any warnings, then:
```bash
git add src/commands/spend.rs tests/spend_test.rs
git commit -m "feat(spend): color usage bar and label by budget tier"
```

---

## Self-Review Notes

- **Spec coverage:** 4-tier thresholds (Task 1), bar+label-only coloring (Task 2 Step 3), NO_COLOR/TTY auto-detection via `supports-colors` feature (Task 1 Step 1, Task 2 Step 3), JSON output untouched (no task modifies the `if json { ... }` branch) — all covered.
- **Type consistency:** `tier_color` signature (`fn tier_color(pct_used: f64) -> owo_colors::Rgb`) is identical between Task 1 (definition) and Task 2 (call site).
- **No placeholders:** every step shows exact code/commands.
