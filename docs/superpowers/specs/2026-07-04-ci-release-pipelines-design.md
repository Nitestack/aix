# CI and Release Pipelines Design

**Date:** 2026-07-04
**Status:** Approved

## Overview

Two GitHub Actions workflows for the `aix` Rust CLI:

1. **CI** — fast correctness gate on PRs and pushes to `main`
2. **Release** — rolling `latest` binary release on every push to `main`

No versioning. The release pipeline overwrites the previous `latest` artifacts on each merge.

---

## CI Pipeline

**File:** `.github/workflows/ci.yml`

**Triggers:**
- `push` to `main`
- `pull_request` targeting `main`

**Single job:** `check` on `ubuntu-latest`

**Steps:**
1. `actions/checkout`
2. `dtolnay/rust-toolchain@stable` with `rustfmt` and `clippy` components
3. `Swatinem/rust-cache` — caches `~/.cargo` and `target/` keyed on `Cargo.lock`
4. `cargo fmt --check`
5. `cargo clippy --all-targets -- -D warnings`
6. `cargo test`

**Rationale:** A single sequential job is more minute-efficient than splitting into parallel jobs (fmt/clippy/test), which would triple checkout and setup overhead. For a small Rust project the sequential run is fast enough that the wall-clock difference is negligible on GitHub Free.

---

## Release Pipeline

**File:** `.github/workflows/release.yml`

**Trigger:** `push` to `main`

### Build Jobs (run in parallel)

| Job | Runner | Targets |
|---|---|---|
| `build-linux` | `ubuntu-latest` | `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` |
| `build-macos` | `macos-latest` | `aarch64-apple-darwin`, `x86_64-apple-darwin` |
| `build-windows` | `windows-latest` | `x86_64-pc-windows-msvc` |

**`build-linux`:** Uses [`cross`](https://github.com/cross-rs/cross) (Docker-based cross-compilation) for the aarch64 target. Both binaries are built in the same job to avoid a second Linux runner.

**`build-macos`:** Runs on `macos-latest` (Apple Silicon). Adds `x86_64-apple-darwin` via `rustup target add` and builds both targets natively using Apple's toolchain, which supports cross-compilation between macOS architectures without extra tooling.

**`build-windows`:** Straight `cargo build --release --target x86_64-pc-windows-msvc` on `windows-latest`.

**Steps (each job):**
1. `actions/checkout`
2. `dtolnay/rust-toolchain@stable`
3. `Swatinem/rust-cache`
4. Install `cross` (Linux only) via `cargo install cross`
5. Build all targets for this job
6. `actions/upload-artifact` — upload binaries

**Binary naming:** `aix-<target>` (e.g. `aix-x86_64-unknown-linux-gnu`) and `aix-x86_64-pc-windows-msvc.exe` for Windows.

### Release Job

**Depends on:** all three build jobs

**Steps:**
1. `actions/checkout`
2. `actions/download-artifact` — collect all binaries
3. `softprops/action-gh-release` with:
   - `tag_name: latest`
   - `name: Latest`
   - `prerelease: false`
   - All 5 binaries as release assets

This upserts the `latest` GitHub Release on every push, replacing previous binaries in-place.

---

## Cost Profile (GitHub Free)

| Runner | Minute multiplier | Jobs |
|---|---|---|
| `ubuntu-latest` | 1× | 1 (CI) + 1 (release Linux) |
| `macos-latest` | 10× | 1 (release macOS, covers 2 targets) |
| `windows-latest` | 2× | 1 (release Windows) |

Using a single macOS runner for both Darwin targets halves macOS minute consumption compared to a naive per-target matrix.
