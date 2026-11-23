//! The single-instance guard.
//!
//! A second launch must not start a second workbench: it hands focus to the
//! running one and exits. The slot is `~/.steer/instance.lock` held with a
//! non-blocking `flock` — the kernel drops it with the process, so a crash
//! never strands a stale lock (no PID probing, no `/proc` parsing). The guard
//! is best-effort: only a live `flock` contention counts as "another instance
//! is running"; when the slot itself cannot be opened or locked (bad
//! permissions, a filesystem without `flock`, an unresolvable state root) the
//! app starts without the guard rather than refuse to launch — the same
//! posture as `steer_paths::install`. macOS additionally raises the running
//! app through AppKit's running-application table, so the newcomer's
//! activation lands on the existing windows instead of a dock icon that does
//! nothing.

// The only consumer is the unix-guarded `lock_path`; ungated, the import is
// dead on platforms without the guard (Windows) and fails `-D warnings`.
#[cfg(unix)]
use std::path::PathBuf;

/// Claims the process's single-instance slot: the first instance keeps
/// running; a second one focuses the first (macOS) and exits. A slot that
/// cannot be opened or locked for environmental reasons starts without the
/// guard.
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
    match claim(&path) {
        // The lock is the process's lifetime; keeping the descriptor open is
        // the point (dropping it would release the slot).
        LockClaim::Held(held) => std::mem::forget(held),
        LockClaim::TakenByOther => {
            focus_running();
            std::process::exit(0);
        }
        LockClaim::Unavailable(error) => {
            // Not evidence of a running instance: diagnose and start without
            // the guard instead of silently exiting on every launch.
            eprintln!(
                "steer: instance lock {} unavailable ({error}); starting without the single-instance guard",
                path.display()
            );
        }
    }
}

#[cfg(unix)]
fn lock_path() -> Option<PathBuf> {
    let home = steer_paths::steer_home()?;
    std::fs::create_dir_all(&home).ok()?;
    Some(home.join("instance.lock"))
}

/// The outcome of one non-blocking attempt to take the instance lock.
#[cfg(unix)]
#[derive(Debug)]
enum LockClaim {
    /// This process holds the exclusive lock; the `File` must stay alive for
    /// the process lifetime (dropping it releases the slot).
    Held(std::fs::File),
    /// Another live instance holds the lock (`flock` returned
    /// `EWOULDBLOCK`/`EAGAIN`).
    TakenByOther,
    /// The slot could not be opened or locked for a reason other than
    /// contention — an environment failure, not a running instance.
    Unavailable(std::io::Error),
}

/// Classifies a non-blocking attempt to take the exclusive lock at `path`.
/// Process-level side effects (focusing the running instance, exiting) live
/// with the caller, not here, so the classification is directly testable.
#[cfg(unix)]
fn claim(path: &std::path::Path) -> LockClaim {
    use std::os::unix::io::AsRawFd;

    let file = match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
    {
        Ok(file) => file,
        Err(error) => return LockClaim::Unavailable(error),
    };
    // SAFETY: `as_raw_fd` is valid for this call and `flock` takes no
    // pointers.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return LockClaim::Held(file);
    }
    let error = std::io::Error::last_os_error();
    match error.kind() {
        // Both `EWOULDBLOCK` and `EAGAIN` decode to `WouldBlock`; either
        // errno means another holder, everything else is environmental.
        std::io::ErrorKind::WouldBlock => LockClaim::TakenByOther,
        _ => LockClaim::Unavailable(error),
    }
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

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("steer-instance-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("the temp dir is created");
        dir
    }

    #[test]
    fn the_lock_is_exclusive_and_released_on_drop() {
        let dir = temp_dir("exclusive");
        let path = dir.join("instance.lock");

        let first = match claim(&path) {
            LockClaim::Held(file) => file,
            other => panic!("the first claim succeeds, got {other:?}"),
        };
        assert!(
            matches!(claim(&path), LockClaim::TakenByOther),
            "the second claim is refused"
        );
        drop(first);
        assert!(
            matches!(claim(&path), LockClaim::Held(_)),
            "dropping releases the slot"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unlockable_path_is_unavailable_not_taken() {
        let dir = temp_dir("unlockable");

        // A directory cannot be opened for writing, so the claim fails before
        // `flock` is ever reached: that is an environment failure, not lock
        // contention, and must not be read as "another instance is running".
        // (Works under root too, where permission bits would not.)
        assert!(
            matches!(claim(&dir), LockClaim::Unavailable(_)),
            "an unopenable slot is Unavailable, not TakenByOther"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
