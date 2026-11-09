//! The GitHub Releases check: query the repo's `latest` published release
//! and fold it into an update candidate against the running version.

use anyhow::{Context as _, Result, bail};
use semver::Version;
use serde::Deserialize;

/// The repo's `latest` release. The endpoint never surfaces drafts or
/// prereleases, so release-candidate builds only ever update by hand.
pub const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/dspo/manox-app/releases/latest";

/// The install surface this updater consumes: the release workflow's macOS
/// arm64 dmg. Suffix matching tolerates prefix/version drift in the name
/// where full-name equality would not.
const ASSET_SUFFIX: &str = "_macos_arm64.dmg";

/// A folded update candidate: everything the download and install steps need,
/// resolved against one specific release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseInfo {
    /// The release tag with its `v` prefix stripped; `+build` metadata rides
    /// along but never affects the ordering (semver ignores it).
    pub version: Version,
    pub asset_name: String,
    pub asset_url: String,
    /// Hex sha256 of the dmg, from the asset's `digest` field.
    pub asset_sha256: String,
    pub asset_size: u64,
}

/// The `/releases/latest` payload, narrowed to what the fold consumes.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct LatestRelease {
    pub tag_name: String,
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub name: String,
    pub size: u64,
    pub browser_download_url: String,
    /// `sha256:<hex>`; the integrity anchor for the download. A release
    /// without it is treated as unusable — an undetectable corrupt download
    /// is worse than a missed update.
    pub digest: Option<String>,
}

/// GET the repo's latest published release. macOS-only: the update channel
/// ends in a bundle overwrite that only macOS has, so the seam refuses
/// elsewhere rather than spending API quota on an uninstallable answer.
#[cfg(target_os = "macos")]
pub fn fetch_latest() -> Result<LatestRelease> {
    const API_ACCEPT: &str = "application/vnd.github+json";
    const API_VERSION: &str = "2022-11-28";
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("Steer/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("failed to build the update-check HTTP client")?;
    let response = client
        .get(LATEST_RELEASE_URL)
        .header(reqwest::header::ACCEPT, API_ACCEPT)
        .header("X-GitHub-Api-Version", API_VERSION)
        .send()
        .context("update check request failed")?;
    let status = response.status();
    if !status.is_success() {
        bail!("GitHub Releases returned {status}");
    }
    response
        .json()
        .context("the GitHub Releases response did not decode")
}

/// The seam's non-macOS shape: no install surface, no check.
#[cfg(not(target_os = "macos"))]
pub fn fetch_latest() -> Result<LatestRelease> {
    bail!("in-app updates are only supported on macOS")
}

/// Fold a fetched release into an update candidate against `current`.
/// `Ok(None)` = not updatable — up to date, or a tag that refuses to parse
/// (an unreadable tag is a judgment the check makes conservatively, not an
/// error). `Err` = a newer release exists but is unusable (no dmg asset,
/// missing digest) — an anomaly worth surfacing, not silence.
pub fn fold_release(raw: &LatestRelease, current: &Version) -> Result<Option<ReleaseInfo>> {
    let Some(version) = normalize_tag(&raw.tag_name) else {
        return Ok(None);
    };
    if version <= *current {
        return Ok(None);
    }
    let asset = raw
        .assets
        .iter()
        .find(|asset| asset.name.ends_with(ASSET_SUFFIX))
        .context("the new release carries no macOS arm64 dmg asset")?;
    let digest = asset
        .digest
        .as_deref()
        .context("the dmg asset carries no digest")?;
    let sha256 = digest
        .strip_prefix("sha256:")
        .context("the dmg asset digest is not a sha256")?
        .to_ascii_lowercase();
    Ok(Some(ReleaseInfo {
        version,
        asset_name: asset.name.clone(),
        asset_url: asset.browser_download_url.clone(),
        asset_sha256: sha256,
        asset_size: asset.size,
    }))
}

/// A release tag as a semver: leading `v` stripped, the rest parsed whole —
/// prerelease and build parts stay (semver ordering already ranks `1.2.0-rc.1`
/// below `1.2.0`), and anything unparseable is `None`.
fn normalize_tag(tag: &str) -> Option<Version> {
    let tag = tag.trim();
    Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURRENT: &str = "0.3.0";

    fn fixture(json: &str) -> LatestRelease {
        serde_json::from_str(json).expect("fixture decodes")
    }

    fn release_with_assets(tag: &str, assets: &[(&str, u64, Option<&str>)]) -> LatestRelease {
        LatestRelease {
            tag_name: tag.into(),
            assets: assets
                .iter()
                .map(|(name, size, digest)| ReleaseAsset {
                    name: (*name).into(),
                    size: *size,
                    browser_download_url: format!(
                        "https://github.com/dspo/manox-app/releases/download/{tag}/{name}"
                    ),
                    digest: digest.map(|d| d.into()),
                })
                .collect(),
        }
    }

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn normalize_tag_strips_v_and_parses_the_rest() {
        assert_eq!(
            normalize_tag("v0.4.0"),
            Some("0.4.0".parse().expect("valid semver"))
        );
        assert_eq!(
            normalize_tag("0.4.1"),
            Some("0.4.1".parse().expect("valid semver"))
        );
        assert_eq!(normalize_tag("  v1.2.3  "), Some("1.2.3".parse().unwrap()));
        // Build metadata rides along; prerelease parts stay for ordering.
        assert_eq!(
            normalize_tag("v0.4.0+build.7"),
            Some("0.4.0+build.7".parse().unwrap())
        );
        assert_eq!(
            normalize_tag("0.4.0-rc.1"),
            Some("0.4.0-rc.1".parse().unwrap())
        );
        assert_eq!(normalize_tag("nightly"), None);
        assert_eq!(normalize_tag("v1.2"), None);
        assert_eq!(normalize_tag(""), None);
    }

    #[test]
    fn an_unparseable_tag_is_not_updatable_not_an_error() {
        let raw = release_with_assets("nightly", &[("Steer_0.4.0_macos_arm64.dmg", 1, Some("x"))]);
        assert_eq!(fold_release(&raw, &CURRENT.parse().unwrap()).unwrap(), None);
    }

    #[test]
    fn an_older_or_equal_release_is_not_updatable() {
        let current: Version = CURRENT.parse().unwrap();
        let older = release_with_assets(
            "v0.2.9",
            &[("Steer_0.2.9_macos_arm64.dmg", 1, Some("sha256:x"))],
        );
        let equal = release_with_assets(
            "v0.3.0",
            &[("Steer_0.3.0_macos_arm64.dmg", 1, Some("sha256:x"))],
        );
        assert_eq!(fold_release(&older, &current).unwrap(), None);
        assert_eq!(fold_release(&equal, &current).unwrap(), None);
    }

    #[test]
    fn a_newer_release_folds_into_a_candidate() {
        let raw = release_with_assets(
            "v0.4.0",
            &[
                ("SHA256SUMS.txt", 200, None),
                (
                    "Steer_0.4.0_macos_arm64.zip",
                    50_000_000,
                    Some("sha256:zip"),
                ),
                (
                    "Steer_0.4.0_macos_arm64.dmg",
                    90_000_000,
                    Some(&format!("sha256:{SHA}")),
                ),
            ],
        );
        let folded = fold_release(&raw, &CURRENT.parse().unwrap())
            .unwrap()
            .expect("a newer release with a dmg is updatable");
        assert_eq!(folded.version, "0.4.0".parse().unwrap());
        assert_eq!(folded.asset_name, "Steer_0.4.0_macos_arm64.dmg");
        assert_eq!(
            folded.asset_url,
            "https://github.com/dspo/manox-app/releases/download/v0.4.0/Steer_0.4.0_macos_arm64.dmg"
        );
        assert_eq!(folded.asset_sha256, SHA);
        assert_eq!(folded.asset_size, 90_000_000);
    }

    /// The zip and the checksums file must never win the asset selection,
    /// even when they precede the dmg in the list.
    #[test]
    fn the_asset_selection_ignores_zip_and_checksum_files() {
        let raw = release_with_assets(
            "v0.4.0",
            &[
                ("SHA256SUMS.txt", 200, None),
                (
                    "Steer_0.4.0_macos_arm64.zip",
                    50_000_000,
                    Some("sha256:zip"),
                ),
                (
                    "Steer_0.4.0_macos_arm64.dmg",
                    90_000_000,
                    Some(&format!("sha256:{SHA}")),
                ),
            ],
        );
        let folded = fold_release(&raw, &CURRENT.parse().unwrap())
            .unwrap()
            .expect("updatable");
        assert!(folded.asset_name.ends_with("_macos_arm64.dmg"));
    }

    /// A newer release without a dmg is an anomaly worth surfacing, not
    /// silence — the check errors instead of clearing the store.
    #[test]
    fn a_newer_release_without_a_dmg_is_an_error() {
        let raw = release_with_assets(
            "v0.4.0",
            &[("Steer_0.4.0_macos_arm64.zip", 1, Some("sha256:zip"))],
        );
        assert!(fold_release(&raw, &CURRENT.parse().unwrap()).is_err());
    }

    /// Missing digest = unusable release: an unverifiable download is worse
    /// than a missed update.
    #[test]
    fn a_dmg_without_a_digest_is_an_error() {
        let raw = release_with_assets("v0.4.0", &[("Steer_0.4.0_macos_arm64.dmg", 1, None)]);
        assert!(fold_release(&raw, &CURRENT.parse().unwrap()).is_err());
    }

    #[test]
    fn a_non_sha256_digest_is_an_error() {
        let raw = release_with_assets(
            "v0.4.0",
            &[("Steer_0.4.0_macos_arm64.dmg", 1, Some("md5:abc"))],
        );
        assert!(fold_release(&raw, &CURRENT.parse().unwrap()).is_err());
    }

    /// A real-shaped `/releases/latest` payload (digest attached to the dmg
    /// asset) decodes and folds end to end.
    #[test]
    fn a_full_payload_folds() {
        let raw = fixture(
            r#"{
              "tag_name": "v0.4.0",
              "html_url": "https://github.com/dspo/manox-app/releases/tag/v0.4.0",
              "assets": [
                {
                  "name": "Steer_0.4.0_macos_arm64.dmg",
                  "size": 94371840,
                  "browser_download_url": "https://github.com/dspo/manox-app/releases/download/v0.4.0/Steer_0.4.0_macos_arm64.dmg",
                  "digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                },
                {
                  "name": "Steer_0.4.0_macos_arm64.zip",
                  "size": 92274688,
                  "browser_download_url": "https://github.com/dspo/manox-app/releases/download/v0.4.0/Steer_0.4.0_macos_arm64.zip",
                  "digest": "sha256:fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"
                }
              ]
            }"#,
        );
        let folded = fold_release(&raw, &CURRENT.parse().unwrap())
            .unwrap()
            .expect("updatable");
        assert_eq!(folded.asset_size, 94371840);
        assert_eq!(
            folded.asset_sha256,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
    }
}
