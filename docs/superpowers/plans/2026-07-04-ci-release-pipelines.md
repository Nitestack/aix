# CI and Release Pipelines Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add two GitHub Actions workflows — a CI check pipeline and a rolling `latest` release pipeline — to the `aix` Rust CLI project.

**Architecture:** Two YAML files under `.github/workflows/`. CI runs `cargo fmt`, `cargo clippy`, and `cargo test` in a single sequential job on `ubuntu-latest`. Release builds five targets across three OS runners in parallel, then a dependent job upserts a rolling `latest` GitHub Release by deleting the old one and recreating it.

**Tech Stack:** GitHub Actions, `dtolnay/rust-toolchain`, `Swatinem/rust-cache@v2`, `taiki-e/install-action` (fast cross install), `cross` (Docker-based cross-compilation for Linux aarch64), `gh` CLI (release management via `GITHUB_TOKEN`).

---

## File Map

| Action | Path | Purpose |
|---|---|---|
| Create | `.github/workflows/ci.yml` | CI: fmt + clippy + test on PRs and pushes to main |
| Create | `.github/workflows/release.yml` | Release: build 5 targets, upsert rolling `latest` release |

---

### Task 1: Create the CI workflow

**Files:**
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Create the workflows directory**

```bash
mkdir -p .github/workflows
```

- [ ] **Step 2: Write `.github/workflows/ci.yml`**

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  check:
    name: Check
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt,clippy

      - uses: Swatinem/rust-cache@v2

      - name: Format check
        run: cargo fmt --check

      - name: Clippy
        run: cargo clippy --all-targets -- -D warnings

      - name: Test
        run: cargo test
```

- [ ] **Step 3: Validate the YAML is well-formed**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))" && echo "YAML OK"
```

Expected: `YAML OK`

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: add CI workflow (fmt, clippy, test)"
```

---

### Task 2: Create the release workflow

**Files:**
- Create: `.github/workflows/release.yml`

- [ ] **Step 1: Write `.github/workflows/release.yml`**

```yaml
name: Release

on:
  push:
    branches: [main]

jobs:
  build-linux:
    name: Build Linux
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable

      - uses: Swatinem/rust-cache@v2

      - name: Install cross
        uses: taiki-e/install-action@v2
        with:
          tool: cross

      - name: Build x86_64-unknown-linux-gnu
        run: cross build --release --target x86_64-unknown-linux-gnu

      - name: Build aarch64-unknown-linux-gnu
        run: cross build --release --target aarch64-unknown-linux-gnu

      - name: Stage Linux artifacts
        run: |
          mkdir -p stage
          cp target/x86_64-unknown-linux-gnu/release/aix stage/aix-x86_64-unknown-linux-gnu
          cp target/aarch64-unknown-linux-gnu/release/aix stage/aix-aarch64-unknown-linux-gnu

      - uses: actions/upload-artifact@v4
        with:
          name: linux-binaries
          path: stage/

  build-macos:
    name: Build macOS
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: aarch64-apple-darwin,x86_64-apple-darwin

      - uses: Swatinem/rust-cache@v2

      - name: Build aarch64-apple-darwin
        run: cargo build --release --target aarch64-apple-darwin

      - name: Build x86_64-apple-darwin
        run: cargo build --release --target x86_64-apple-darwin

      - name: Stage macOS artifacts
        run: |
          mkdir -p stage
          cp target/aarch64-apple-darwin/release/aix stage/aix-aarch64-apple-darwin
          cp target/x86_64-apple-darwin/release/aix stage/aix-x86_64-apple-darwin

      - uses: actions/upload-artifact@v4
        with:
          name: macos-binaries
          path: stage/

  build-windows:
    name: Build Windows
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable

      - uses: Swatinem/rust-cache@v2

      - name: Build x86_64-pc-windows-msvc
        run: cargo build --release --target x86_64-pc-windows-msvc

      - name: Stage Windows artifact
        shell: pwsh
        run: |
          New-Item -ItemType Directory -Force -Path stage
          Copy-Item target\x86_64-pc-windows-msvc\release\aix.exe stage\aix-x86_64-pc-windows-msvc.exe

      - uses: actions/upload-artifact@v4
        with:
          name: windows-binary
          path: stage/

  release:
    name: Release
    needs: [build-linux, build-macos, build-windows]
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v4

      - uses: actions/download-artifact@v4
        with:
          path: artifacts
          merge-multiple: true

      - name: Upsert latest release
        run: |
          gh release delete latest --yes 2>/dev/null || true
          git tag -f latest ${{ github.sha }}
          git push -f origin latest
          gh release create latest \
            --title "Latest" \
            --notes "Latest build from \`main\` at ${{ github.sha }}." \
            artifacts/*
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

After `download-artifact` with `merge-multiple: true`, the `artifacts/` directory will contain all five files flat:
```
artifacts/
  aix-x86_64-unknown-linux-gnu
  aix-aarch64-unknown-linux-gnu
  aix-aarch64-apple-darwin
  aix-x86_64-apple-darwin
  aix-x86_64-pc-windows-msvc.exe
```

- [ ] **Step 2: Validate the YAML is well-formed**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml'))" && echo "YAML OK"
```

Expected: `YAML OK`

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: add rolling latest release workflow"
```

---

### Task 3: Verify on GitHub

**Files:** none (observation only)

- [ ] **Step 1: Push to main and watch the Actions tab**

After pushing, go to the repository's **Actions** tab. Verify:
- The `CI` workflow triggers and the `check` job passes all three steps (Format check → Clippy → Test)
- The `Release` workflow triggers and all three build jobs (`Build Linux`, `Build macOS`, `Build Windows`) run in parallel, followed by the `Release` job

- [ ] **Step 2: Verify release assets**

Go to the repository's **Releases** page. Confirm the `latest` release contains exactly these five files:
- `aix-x86_64-unknown-linux-gnu`
- `aix-aarch64-unknown-linux-gnu`
- `aix-aarch64-apple-darwin`
- `aix-x86_64-apple-darwin`
- `aix-x86_64-pc-windows-msvc.exe`
