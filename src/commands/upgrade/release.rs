//! GitHub release lookups and verified downloads.

use color_eyre::eyre::{bail, eyre, WrapErr};
use color_eyre::Result;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::time::Duration;

/// Releases live next to the source; the URL comes from Cargo metadata so it
/// is not repeated in command logic.
pub(crate) const REPO_URL: &str = env!("CARGO_PKG_REPOSITORY");

/// Downloads of release assets may take longer than a plain API call.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// `MAJOR.MINOR.PATCH`; any pre-release or build suffix is ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Version(u64, u64, u64);

impl Version {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches('v');
        let core = text.split(['-', '+']).next()?;
        let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
        let version = Self(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(version)
    }

    pub(crate) fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("crate version is semver")
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// Is `latest` worth installing over `current`?
pub(crate) fn needs_upgrade(current: Version, latest: Version) -> bool {
    latest.cmp(&current) == Ordering::Greater
}

pub(crate) struct Releases {
    client: reqwest::Client,
    no_redirect: reqwest::Client,
    timeout: Duration,
}

impl Releases {
    pub(crate) fn new(timeout: Duration) -> Result<Self> {
        let user_agent = concat!("aix/", env!("CARGO_PKG_VERSION"));
        let build = |redirect| {
            reqwest::Client::builder()
                .user_agent(user_agent)
                .connect_timeout(timeout)
                .redirect(redirect)
                .build()
        };
        Ok(Self {
            client: build(reqwest::redirect::Policy::limited(10))?,
            no_redirect: build(reqwest::redirect::Policy::none())?,
            timeout,
        })
    }

    /// The newest stable release. GitHub answers `/releases/latest` with a
    /// redirect to `/releases/tag/vX.Y.Z`; reading the redirect avoids the
    /// rate-limited REST API.
    pub(crate) async fn latest_version(&self) -> Result<Version> {
        let url = format!("{REPO_URL}/releases/latest");
        let response = self
            .no_redirect
            .get(&url)
            .timeout(self.timeout)
            .send()
            .await
            .wrap_err_with(|| format!("could not reach {url}"))?;
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| {
                eyre!(
                    "{url} did not redirect to a release (HTTP {})",
                    response.status()
                )
            })?;
        let tag = location.rsplit('/').next().unwrap_or_default();
        Version::parse(tag).ok_or_else(|| eyre!("latest release tag {tag:?} is not a version"))
    }

    pub(crate) async fn download(&self, url: &str) -> Result<Vec<u8>> {
        let response = self
            .client
            .get(url)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await
            .wrap_err_with(|| format!("could not download {url}"))?;
        if !response.status().is_success() {
            bail!("downloading {url} failed (HTTP {})", response.status());
        }
        Ok(response
            .bytes()
            .await
            .wrap_err_with(|| format!("could not download {url}"))?
            .to_vec())
    }

    /// Download `asset` from release `version` and check it against that
    /// release's `SHA256SUMS`.
    pub(crate) async fn download_verified(&self, version: Version, asset: &str) -> Result<Vec<u8>> {
        let base = format!("{REPO_URL}/releases/download/v{version}");
        let sums = self.download(&format!("{base}/SHA256SUMS")).await?;
        let sums = String::from_utf8_lossy(&sums);
        let expected = checksum_for(&sums, asset)
            .ok_or_else(|| eyre!("SHA256SUMS of v{version} has no entry for {asset}"))?;
        let bytes = self.download(&format!("{base}/{asset}")).await?;
        let actual = sha256_hex(&bytes);
        if !actual.eq_ignore_ascii_case(&expected) {
            bail!("checksum mismatch for {asset} (expected {expected}, got {actual})");
        }
        Ok(bytes)
    }
}

/// Find `asset` in a `sha256sum`-style listing (`<hash>  <name>` or
/// `<hash> *<name>`).
fn checksum_for(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let hash = fields.next()?;
        let name = fields.next()?;
        (name.trim_start_matches('*') == asset).then(|| hash.to_string())
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tags_and_versions() {
        assert_eq!(Version::parse("v1.2.3"), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse("1.2.3"), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse("1.2.3-rc.1"), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse("nightly"), None);
        assert_eq!(Version::parse("1.2"), None);
        assert_eq!(Version::parse("1.2.3.4"), None);
    }

    #[test]
    fn only_newer_releases_need_an_upgrade() {
        let v = |s| Version::parse(s).unwrap();
        assert!(needs_upgrade(v("1.0.0"), v("1.0.1")));
        assert!(needs_upgrade(v("1.9.0"), v("1.10.0")));
        assert!(!needs_upgrade(v("1.0.0"), v("1.0.0")));
        assert!(!needs_upgrade(v("1.1.0"), v("1.0.9")));
    }

    #[test]
    fn finds_checksums_in_both_sha256sum_formats() {
        let sums = "aaa  aix-x86_64-unknown-linux-musl\nbbb *aix_1.0.0_amd64.deb\n";
        assert_eq!(
            checksum_for(sums, "aix-x86_64-unknown-linux-musl").as_deref(),
            Some("aaa")
        );
        assert_eq!(
            checksum_for(sums, "aix_1.0.0_amd64.deb").as_deref(),
            Some("bbb")
        );
        assert_eq!(checksum_for(sums, "aix"), None);
    }

    #[test]
    fn sha256_matches_known_value() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
