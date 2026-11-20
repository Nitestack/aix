# Releasing

Maintainer notes on how releases are cut and distributed.

## Flow

1. When you are ready to cut a release, go to Actions → **Prepare release**
   ([`prepare-release.yml`](../.github/workflows/prepare-release.yml)) →
   **Run workflow** against `main` and choose:
   - **bump:** `patch` (default), `minor`, or `major`; or
   - **version:** an explicit SemVer version (for example `1.2.3` or `v1.2.3`),
     which overrides `bump`.

   The workflow runs `release-plz update` to draft the
   [`CHANGELOG.md`](../CHANGELOG.md) entry from
   [Conventional Commits](https://www.conventionalcommits.org/) (`feat`, `fix`,
   and `perf` are grouped into `Added`, `Fixed`, and `Performance`; `ci`,
   `docs`, `build`, `chore`, `test`, `style`, and `refactor` are skipped; see
   [`release-plz.toml`](../release-plz.toml)), runs `release-plz set-version` to
   apply your chosen version to `Cargo.toml`, `Cargo.lock`, and `CHANGELOG.md`,
   and opens (or updates) a **`chore: release vX.Y.Z`** PR from `release/next`.
   Re-running **Prepare release** with a different bump or after more commits
   land on `main` updates that same PR in place.
2. Review and edit the PR (for example to polish changelog entries), then merge
   it.
3. [`release.yml`](../.github/workflows/release.yml) then runs on `main` and:
   - publishes `aix-rs` to crates.io;
   - tags `vX.Y.Z` and creates the GitHub release;
   - builds binaries with [`build.yml`](../.github/workflows/build.yml) and attaches them
     along with `SHA256SUMS` and the installer scripts;
   - runs [`packages.yml`](../.github/workflows/packages.yml) to update Homebrew, Scoop,
     the AUR and winget.

[`nightly.yml`](../.github/workflows/nightly.yml) rebuilds the `nightly` prerelease on
every push to `main`. It is never marked as the latest release, so it doesn't change
the stable installer URLs or package-manager channels. The nightly release includes
its own `install.sh` and `install.ps1`, configured to download from the rolling
`nightly` tag. It also publishes `NIGHTLY_VERSION` (the full source commit ID)
in `SHA256SUMS`; the installers and `aix upgrade` use that ID to detect a new
nightly even though the Cargo version may not change between builds. Nightly
installer scripts save the nightly update target beside the installed binary.

## Retrying

- **A build job failed or was never picked up** (macOS runners sometimes queue
  until they time out): use **Re-run failed jobs** on the Release run. Nothing is
  published twice, because the jobs that already succeeded are reused.
- **A package-manager channel failed:** go to Actions → **Package managers** →
  **Run workflow** and enter the tag (for example `v1.1.0`). Every channel is
  safe to re-run.

## Backfilling an existing release

When the pipeline gains new assets or package channels, you don't need a new
version to ship them. Go to Actions → **Backfill release assets**
([`backfill.yml`](../.github/workflows/backfill.yml)) → **Run workflow** and enter an
existing tag (for example `v1.0.0`). The workflow:

1. rebuilds that tag's source with the current build and packaging pipeline;
2. replaces the release's assets: binaries, `.deb`/`.rpm`, `SHA256SUMS`, and the
   installers;
3. runs the package-manager channels for that tag.

It never publishes to crates.io. Crate metadata changes, such as cargo-binstall
support, therefore still need a real release.

## Secrets and variables

Configure these on `get-aix/aix` (Settings → Secrets and variables → Actions).
A channel whose secret is missing is skipped with a notice. It does not fail the
run.

| Name | Kind | Used for | Value |
|------|------|----------|-------|
| `HOMEBREW_TAP_DEPLOY_KEY` | secret | Homebrew tap | Private SSH key; its public key is a write deploy key on `get-aix/homebrew-tap` |
| `SCOOP_BUCKET_DEPLOY_KEY` | secret | Scoop bucket | Private SSH key; its public key is a write deploy key on `get-aix/scoop-bucket` |
| `AUR_SSH_PRIVATE_KEY` | secret | AUR `aix-rs-bin` | Private SSH key whose public key is registered on the AUR account |
| `AUR_USERNAME`, `AUR_EMAIL` | variables | AUR commit author | Optional |
| `WINGET_TOKEN` | secret | winget | Classic PAT with the `public_repo` scope, owned by the account that forks `microsoft/winget-pkgs` |
| `WINGET_FORK_USER` | variable | winget | GitHub username that owns the `winget-pkgs` fork |
| `RELEASE_PLZ_TOKEN` | secret | Optional | PAT or GitHub App token so CI runs on the release PR |

crates.io publishing uses Trusted Publishing (OIDC), so it needs no secret.

## Package channels

| Channel | Where | Package |
|---------|-------|---------|
| crates.io | <https://crates.io/crates/aix-rs> | `aix-rs` (binstall metadata in `Cargo.toml`) |
| Homebrew | <https://github.com/get-aix/homebrew-tap> | `brew tap get-aix/tap` then `brew install aix` |
| Scoop | <https://github.com/get-aix/scoop-bucket> | `get-aix/aix` |
| AUR | <https://aur.archlinux.org/packages/aix-rs-bin> | `aix-rs-bin` |
| winget | <https://github.com/microsoft/winget-pkgs> | `get-aix.aix` |

The manifests are generated by [`scripts/packaging/render.sh`](../scripts/packaging/render.sh)
from the release's `SHA256SUMS`. To preview them locally:

```sh
gh release download v1.1.0 --pattern SHA256SUMS
scripts/packaging/render.sh v1.1.0 SHA256SUMS /tmp/manifests
```

New winget packages must be submitted once by hand, because the automation only
updates existing packages. After that, every release opens a PR to
`microsoft/winget-pkgs` automatically. Microsoft's validation and moderators
merge it, usually within a few days.
