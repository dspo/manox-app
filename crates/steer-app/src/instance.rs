//! The single-instance guard (PLAN-ABSORB-07 sub-item 3).
//!
//! A second launch must not start a second workbench: it hands focus to the
//! running one and exits. The slot is `~/.steer/instance.lock` held with a
//! non-blocking `flock` — the kernel drops it with the process, so a crash
//! never strands a stale lock (no PID probing, no `/proc` parsing). macOS
//! additionally raises the running app through AppKit's running-application
//! table, so the newcomer's activation lands on the existing windows instead
//! of a dock icon that does nothing.

use std::path::PathBuf;

/// Claims the process's single-instance slot: the first instance keeps
/// running; a second one focuses the first (macOS) and exits.
pub fn guard() {
    #[cfg(unix)]
    unix_guard();
}

#[cfg(unix)]
fn unix_guard() {
    let Some(path) = lock_path() else {
        // No resolvable state root: run without the guard rather than refuse
        // to start (the same best-effort posture as `steer_paths::install`).
        return;
    };
    let Some(held) = try_lock(&path) else {
        focus_running();
        std::process::exit(0);
    };
    // The lock is the process's lifetime; keeping the descriptor open is the
    // point (dropping it would release the slot).
    std::mem::forget(held);
}

#[cfg(unix)]
fn lock_path() -> Option<PathBuf> {
    let home = steer_paths::steer_home()?;
    std::fs::create_dir_all(&home).ok()?;
    Some(home.join("instance.lock"))
}

/// Takes the exclusive lock without waiting; `None` means another instance
/// holds it.
#[cfg(unix)]
fn try_lock(path: &std::path::Path) -> Option<std::fs::File> {
    use std::os::unix::io::AsRawFd;

    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
        .ok()?;
    // SAFETY: `as_raw_fd` is valid for this call and `flock` takes no
    // pointers.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    (rc == 0).then_some(file)
}

/// Brings the running instance forward. A bundle-less run (a dev binary)
/// has no stable identity to match, so the guard still prevents the duplicate
/// but cannot raise the first window.
#[cfg(target_os = "macos")]
fn focus_running() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    use objc2_foundation::NSBundle;

    let Some(_mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(bundle_id) = NSBundle::mainBundle().bundleIdentifier() else {
        return;
    };
    let own = std::process::id();
    let running = NSRunningApplication::runningApplicationsWithBundleIdentifier(&bundle_id);
    for app in running.iter() {
        if app.processIdentifier() as u32 == own {
            continue;
        }
        // The deprecated `ActivateIgnoringOtherApps` flag is a no-op on
        // macOS 14+; plain activation is the supported request (the running
        // instance answers it with its own window, per AppKit's rules).
        app.activateWithOptions(NSApplicationActivationOptions::empty());
        break;
    }
}

/// Linux/BSD: the lock already prevents the duplicate; there is no portable
/// way to raise another client's window, so the newcomer just exits.
#[cfg(all(unix, not(target_os = "macos")))]
fn focus_running() {}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn the_lock_is_exclusive_and_released_on_drop() {
        let dir = std::env::temp_dir().join(format!("steer-instance-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("the temp dir is created");
        let path = dir.join("instance.lock");

        let first = try_lock(&path).expect("the first claim succeeds");
        assert!(try_lock(&path).is_none(), "the second claim is refused");
        drop(first);
        assert!(try_lock(&path).is_some(), "dropping releases the slot");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
