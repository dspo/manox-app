//! The macOS install surface: locate the running bundle, mount the verified
//! dmg, and `rsync` it over the install position. The running process keeps
//! its old image (the old inode survives the copy); the new files are what
//! the next launch executes. Every other platform has no equivalent — this
//! module compiles to a refusing stub there.

use std::path::Path;

use anyhow::{Result, bail};

#[cfg(target_os = "macos")]
use anyhow::Context as _;
#[cfg(target_os = "macos")]
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;

/// Refuse early — before a download is spent — when there is no install
/// position to overwrite: a dev (`cargo run`) build has no `.app` bundle.
#[cfg(target_os = "macos")]
pub fn preflight() -> Result<()> {
    installed_app_bundle().map(|_| ())
}

/// The stub's shape: no install surface, no update.
#[cfg(not(target_os = "macos"))]
pub fn preflight() -> Result<()> {
    bail!("in-app updates are only supported on macOS")
}

/// Copy the verified dmg over the running install position: mount it at a
/// private mount point, locate the bundle inside, `rsync` its contents onto
/// the installed bundle, and detach through a drop guard on every exit path
/// — a failed copy must not leave the dmg mounted.
#[cfg(target_os = "macos")]
pub fn install_artifact(artifact: &Path) -> Result<()> {
    let installed = installed_app_bundle()?;
    let mount_point = mount_point(&installed);
    // A stale mount point from a crashed run would fail hdiutil; start clean.
    let _ = std::fs::remove_dir_all(&mount_point);
    std::fs::create_dir_all(&mount_point)
        .with_context(|| format!("failed to create the mount point {}", mount_point.display()))?;

    let status = Command::new("hdiutil")
        .arg("attach")
        .args(["-nobrowse", "-readonly", "-mountpoint"])
        .arg(&mount_point)
        .arg(artifact)
        .status()
        .context("failed to run hdiutil attach")?;
    if !status.success() {
        bail!("hdiutil failed to mount {}", artifact.display());
    }

    let _unmount = UnmountGuard {
        mount_point: mount_point.clone(),
    };

    let source = find_app_bundle(&mount_point, &installed)?;
    let status = Command::new("rsync")
        // `-a` keeps the bundle's structure and metadata; `--delete` drops
        // everything the old build had and the new one doesn't; the Finder
        // custom-icon file carries a control character rsync cannot express,
        // so it is excluded rather than copied.
        .args(["-a", "--delete", "--exclude", "Icon?"])
        .arg(trailing_slash(&source))
        .arg(trailing_slash(&installed))
        .status()
        .context("failed to run rsync")?;
    if !status.success() {
        bail!(
            "rsync failed to copy the update over {}",
            installed.display()
        );
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn install_artifact(_artifact: &Path) -> Result<()> {
    bail!("in-app updates are only supported on macOS")
}

/// Detaches the mounted dmg and removes the mount point, whatever the
/// install outcome.
#[cfg(target_os = "macos")]
struct UnmountGuard {
    mount_point: PathBuf,
}

#[cfg(target_os = "macos")]
impl Drop for UnmountGuard {
    fn drop(&mut self) {
        let _ = Command::new("hdiutil")
            .args(["detach", "-force"])
            .arg(&self.mount_point)
            .status();
        let _ = std::fs::remove_dir_all(&self.mount_point);
    }
}

/// The `.app` bundle this process runs from, derived from `current_exe`:
/// `…/Steer.app/Contents/MacOS/steer` → `…/Steer.app`. Anything else — a
/// bare cargo-run binary, a helper at the wrong depth — has no install
/// position and refuses.
#[cfg(target_os = "macos")]
fn installed_app_bundle() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("failed to resolve the current executable")?;
    app_bundle_from_executable(&exe).with_context(|| {
        format!(
            "in-app updates need a bundled .app, but the executable is {}",
            exe.display()
        )
    })
}

#[cfg(target_os = "macos")]
fn app_bundle_from_executable(executable: &Path) -> Option<PathBuf> {
    let macos_dir = executable.parent()?;
    if macos_dir.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents = macos_dir.parent()?;
    if contents.file_name()?.to_str()? != "Contents" {
        return None;
    }
    let bundle = contents.parent()?;
    (bundle.extension()?.to_str()? == "app").then_some(bundle.to_path_buf())
}

/// `<tmp>/steer-update-mount-<pid>-<label>` — pid-scoped so concurrent runs
/// never share a mount point, with the label derived from the bundle's own
/// name, sanitized to `[A-Za-z0-9._-]`.
#[cfg(target_os = "macos")]
fn mount_point(installed: &Path) -> PathBuf {
    let label = installed
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            name.chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                        ch
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
        })
        .unwrap_or_else(|| "app".into());
    std::env::temp_dir().join(format!("steer-update-mount-{}-{label}", std::process::id()))
}

/// The app bundle inside the mounted dmg: prefer the install position's own
/// name, fall back to scanning the volume root for the single `.app` there.
#[cfg(target_os = "macos")]
fn find_app_bundle(mount_point: &Path, installed: &Path) -> Result<PathBuf> {
    if let Some(name) = installed.file_name() {
        let candidate = mount_point.join(name);
        if candidate.is_dir() {
            return Ok(candidate);
        }
    }
    let entries = std::fs::read_dir(mount_point).with_context(|| {
        format!(
            "failed to inspect the mounted update {}",
            mount_point.display()
        )
    })?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("app") {
            return Ok(path);
        }
    }
    bail!(
        "no .app bundle in the mounted update {}",
        mount_point.display()
    )
}

/// `src/` — a trailing slash tells rsync to copy the *contents* of the
/// mounted bundle over the contents of the installed one, not to nest it.
#[cfg(target_os = "macos")]
fn trailing_slash(path: &Path) -> std::ffi::OsString {
    let mut value = path.as_os_str().to_os_string();
    value.push("/");
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test binary itself never lives in a bundle, so the preflight
    /// refuses on every platform — that is the contract.
    #[test]
    fn preflight_refuses_outside_a_bundle() {
        assert!(preflight().is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bundle_paths_derive_from_the_installed_layout() {
        assert_eq!(
            app_bundle_from_executable(Path::new("/Applications/Steer.app/Contents/MacOS/steer")),
            Some(PathBuf::from("/Applications/Steer.app"))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn non_bundle_paths_do_not_derive() {
        // A bare binary has no Contents step.
        assert_eq!(
            app_bundle_from_executable(Path::new("/usr/local/bin/steer")),
            None
        );
        // A helper inside Contents but outside MacOS has no MacOS step.
        assert_eq!(
            app_bundle_from_executable(Path::new(
                "/Applications/Steer.app/Contents/Resources/steer-helper"
            )),
            None
        );
        // The bundle root itself must carry the .app extension.
        assert_eq!(
            app_bundle_from_executable(Path::new("/Applications/Steer/Contents/MacOS/steer")),
            None
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mount_point_labels_are_sanitized_and_pid_scoped() {
        let point = mount_point(Path::new("/Applications/Steer.app"));
        let name = point.file_name().and_then(|n| n.to_str()).unwrap();
        assert!(name.contains("Steer.app"), "{name}");
        assert!(name.starts_with("steer-update-mount-"), "{name}");

        let hostile = mount_point(Path::new("/tmp/app with spaces/../x"));
        let name = hostile.file_name().and_then(|n| n.to_str()).unwrap();
        assert!(
            name.chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')),
            "the label is filesystem-safe: {name}"
        );
    }
}
