//! Downloading a release's dmg into the update cache and verifying it
//! against the release's declared size and digest before it can install.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use sha2::{Digest, Sha256};

use crate::gh_release::ReleaseInfo;

/// Suffix of the in-progress download; the artifact only exists under its
/// final name once size and digest have both checked out.
const PART_SUFFIX: &str = ".part";

/// Hard deadline for one download request, connection through body. See the
/// client builder in [`download`] for why this is the stall bound.
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// The cache directory holding downloaded update artifacts.
pub fn updates_dir() -> Result<PathBuf> {
    let base = dirs::cache_dir().context("missing user cache directory")?;
    Ok(updates_dir_under(&base))
}

/// `<cache>/Steer/updates` — namespaced so a cache shared with other apps
/// never sees these files.
pub fn updates_dir_under(base: &Path) -> PathBuf {
    base.join("Steer").join("updates")
}

/// Remove every file in `dir` except `keep` — leftovers from earlier updates
/// (a failed run's `.part`, dmgs a crash kept) would otherwise accrue
/// forever. Only plain files are removed; directories are left alone.
pub fn prune_stale(dir: &Path, keep: &[&Path]) -> usize {
    let mut removed = 0;
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if !path.is_file() || keep.contains(&path.as_path()) {
            continue;
        }
        if fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Stream the release's dmg to `<updates>/<asset>.part`, then run the
/// integrity gate; only a verified file is promoted to its final name.
pub fn download(release: &ReleaseInfo) -> Result<PathBuf> {
    let dir = updates_dir()?;
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create the updates directory {}", dir.display()))?;
    let file_name = sanitize_file_name(&release.asset_name);
    let artifact_path = dir.join(&file_name);
    let part_path = dir.join(format!("{file_name}{PART_SUFFIX}"));
    prune_stale(&dir, &[&artifact_path, &part_path]);

    // The blocking builder exposes no per-read timeout, so the overall
    // deadline is the stall bound: a server that stops mid-stream fails the
    // request (into a retryable `Failed`) within it instead of hanging the
    // store in `Downloading` forever. The span must still fit a ~80MB dmg on
    // a slow link, hence minutes rather than seconds.
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("Steer/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .context("failed to build the download HTTP client")?;
    let mut response = client
        .get(&release.asset_url)
        .send()
        .with_context(|| format!("failed to request {}", release.asset_url))?;
    if !response.status().is_success() {
        bail!("the artifact download returned {}", response.status());
    }
    // Bail before writing anything when the server's declared length already
    // disagrees with the release manifest.
    if let Some(length) = response.content_length()
        && length != release.asset_size
    {
        bail!(
            "the server declares {length} bytes, the release manifest says {}",
            release.asset_size
        );
    }

    let mut part = File::create(&part_path)
        .with_context(|| format!("failed to create {}", part_path.display()))?;
    let written = response
        .copy_to(&mut part)
        .with_context(|| format!("failed while writing {}", part_path.display()))?;
    drop(part);

    verify_and_promote(
        &part_path,
        &artifact_path,
        release.asset_size,
        &release.asset_sha256,
        written,
    )?;
    Ok(artifact_path)
}

/// The integrity gate, offline-testable: the byte count must match the
/// release manifest and the sha256 must match the asset digest, and only
/// then does the `.part` file rename into place. Every failure removes the
/// temp file — a bad download never lands under the final name.
pub fn verify_and_promote(
    part: &Path,
    artifact: &Path,
    expected_size: u64,
    expected_sha256: &str,
    written: u64,
) -> Result<()> {
    if written != expected_size {
        let _ = fs::remove_file(part);
        bail!("the download is {written} bytes, the release manifest says {expected_size}");
    }
    let actual = match sha256_hex(part) {
        Ok(digest) => digest,
        Err(err) => {
            let _ = fs::remove_file(part);
            return Err(err);
        }
    };
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        let _ = fs::remove_file(part);
        bail!("the artifact digest does not match the release manifest");
    }
    fs::rename(part, artifact)
        .with_context(|| format!("failed to promote the artifact to {}", artifact.display()))
}

/// A file name safe to join under the updates dir: the asset name with every
/// character outside `[A-Za-z0-9._-]` replaced; degenerate inputs fall back
/// to a fixed name.
pub fn sanitize_file_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    match sanitized.as_str() {
        "" | "." | ".." => "steer-update.dmg".into(),
        _ => sanitized,
    }
}

fn sha256_hex(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn write_part(path: &Path, contents: &[u8]) {
        let mut file = File::create(path).expect("part file creates");
        file.write_all(contents).expect("part file writes");
    }

    fn digest_of(contents: &[u8]) -> String {
        let hasher = Sha256::digest(contents);
        hasher
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    }

    #[test]
    fn updates_dir_namespaces_under_steer() {
        assert_eq!(
            updates_dir_under(Path::new("/cache")),
            PathBuf::from("/cache/Steer/updates")
        );
    }

    /// Pruning clears every file outside the keep list — earlier artifacts
    /// and abandoned `.part` files — and never touches a kept path or a
    /// directory.
    #[test]
    fn prune_stale_removes_everything_outside_the_keep_list() {
        let dir = std::env::temp_dir().join(format!("steer-updater-prune-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir creates");

        let kept_artifact = dir.join("Steer_0.4.0_macos_arm64.dmg");
        let kept_part = dir.join("Steer_0.4.0_macos_arm64.dmg.part");
        let stale_artifact = dir.join("Steer_0.3.0_macos_arm64.dmg");
        let stale_part = dir.join("Steer_0.3.0_macos_arm64.dmg.part");
        for path in [&kept_artifact, &kept_part, &stale_artifact, &stale_part] {
            fs::write(path, b"x").expect("fixture writes");
        }
        let nested = dir.join("nested");
        fs::create_dir_all(&nested).expect("fixture dir creates");

        let removed = prune_stale(&dir, &[&kept_artifact, &kept_part]);

        assert_eq!(removed, 2);
        assert!(kept_artifact.is_file());
        assert!(kept_part.is_file());
        assert!(!stale_artifact.exists());
        assert!(!stale_part.exists());
        assert!(nested.is_dir());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_keeps_safe_names_and_replaces_the_rest() {
        assert_eq!(
            sanitize_file_name("Steer_0.4.0_macos_arm64.dmg"),
            "Steer_0.4.0_macos_arm64.dmg"
        );
        assert_eq!(
            sanitize_file_name("Steer 0.4.0 (arm64).dmg"),
            "Steer_0.4.0__arm64_.dmg"
        );
        // Path separators never survive, so the name stays inside the dir.
        assert_eq!(sanitize_file_name("../escape.dmg"), ".._escape.dmg");
        assert_eq!(sanitize_file_name(""), "steer-update.dmg");
        assert_eq!(sanitize_file_name(".."), "steer-update.dmg");
    }

    #[test]
    fn verify_promotes_a_matching_download() {
        let dir = std::env::temp_dir().join(format!("steer-updater-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir creates");
        let part = dir.join("artifact.dmg.part");
        let artifact = dir.join("artifact.dmg");
        let contents = b"a verified payload";
        write_part(&part, contents);

        verify_and_promote(
            &part,
            &artifact,
            contents.len() as u64,
            &digest_of(contents),
            contents.len() as u64,
        )
        .expect("a matching download promotes");

        assert!(artifact.is_file());
        assert!(!part.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_accepts_a_case_drifted_digest() {
        let dir = std::env::temp_dir().join(format!("steer-updater-case-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir creates");
        let part = dir.join("artifact.dmg.part");
        let artifact = dir.join("artifact.dmg");
        let contents = b"another payload";
        write_part(&part, contents);

        verify_and_promote(
            &part,
            &artifact,
            contents.len() as u64,
            &digest_of(contents).to_uppercase(),
            contents.len() as u64,
        )
        .expect("hex case drift does not fail the check");

        assert!(artifact.is_file());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_size_mismatch_removes_the_temp_file_and_fails() {
        let dir = std::env::temp_dir().join(format!("steer-updater-size-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir creates");
        let part = dir.join("artifact.dmg.part");
        let artifact = dir.join("artifact.dmg");
        let contents = b"truncated";
        write_part(&part, contents);

        let err = verify_and_promote(&part, &artifact, 999, "aa", contents.len() as u64)
            .expect_err("a short download fails");

        assert!(err.to_string().contains("bytes"));
        assert!(!part.exists());
        assert!(!artifact.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_digest_mismatch_removes_the_temp_file_and_fails() {
        let dir = std::env::temp_dir().join(format!("steer-updater-sum-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir creates");
        let part = dir.join("artifact.dmg.part");
        let artifact = dir.join("artifact.dmg");
        let contents = b"tampered payload";
        write_part(&part, contents);

        let wrong = "0".repeat(64);
        let err = verify_and_promote(
            &part,
            &artifact,
            contents.len() as u64,
            &wrong,
            contents.len() as u64,
        )
        .expect_err("a wrong digest fails");

        assert!(err.to_string().contains("digest"));
        assert!(!part.exists());
        assert!(!artifact.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
