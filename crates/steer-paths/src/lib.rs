//! Steer platform state root: `~/.steer`.
//!
//! Steer is a multi-harness desktop platform and the manox runtime is one
//! harness among others, so persistent state splits in two layers:
//!
//! - `~/.steer` — the application layer the Steer platform owns outright:
//!   `settings.toml` (`ui_language`), `projects.toml` (project registry and
//!   membership), and later harness-neutral subtrees.
//! - `<steer_home>/.manox` — the manox harness layer (journal, session
//!   state, runtime settings). It is never built by hand: [`install`]
//!   injects it as `MANOX_HOME` at process start, and the runtime's own
//!   `manox_home()` resolves every path beneath it.
//!
//! A root is read and written where it is; nothing here moves, copies or
//! drains state between roots. The layer split was a one-off manual step, and
//! code that re-runs such a move becomes a second writer of the runtime's own
//! tree — the destructive kind, because a stale root can hold same-named
//! paths that mean nothing for the live one.

use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

/// Steer platform state root: `~/.steer`. `STEER_HOME` overrides it wholesale
/// (same precedence discipline as `MANOX_HOME`; the override exists for
/// hermetic tests and isolated dev instances). An override names its own root
/// and is used as given. `None` means no resolvable home — callers fall back
/// to defaults instead of guessing a path.
pub fn steer_home() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("STEER_HOME").filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    dirs::home_dir().map(|home| home.join(".steer"))
}

/// The effective manox runtime root: `MANOX_HOME` once `install()` injected
/// it (or a user preset it), otherwise `<steer_home>/.manox`. Callers that
/// resolve manox paths by hand must go through here so they follow the same
/// root the runtime resolves.
pub fn manox_root() -> Option<PathBuf> {
    if let Some(root) = env::var_os("MANOX_HOME").filter(|root| !root.is_empty()) {
        return Some(PathBuf::from(root));
    }
    steer_home().map(|home| home.join(".manox"))
}

/// Install the process-wide state-root contract: create `<steer_home>` and
/// its manox layer, then inject `MANOX_HOME` — all before any manox path
/// resolution (the runtime resolves paths lazily, but the injection must land
/// first to be authoritative). Call once at process start, from `main()`.
///
/// A preset `MANOX_HOME` is an isolation contract (developers, hermetic
/// tests): respected as-is, nothing is injected.
pub fn install() -> io::Result<()> {
    if env::var_os("MANOX_HOME").is_some_and(|root| !root.is_empty()) {
        return Ok(());
    }
    let home = steer_home().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no home directory; nowhere to root ~/.steer",
        )
    })?;
    let root = install_root(&home)?;
    // SAFETY: process-startup, before any thread exists or any manox path is
    // resolved; the runtime reads this lazily at call time.
    unsafe { env::set_var("MANOX_HOME", &root) };
    Ok(())
}

/// The path-explicit core of [`install`]: create the app home and its manox
/// layer, and return that layer's path. Nothing outside `home` is read, moved
/// or created.
fn install_root(home: &std::path::Path) -> io::Result<PathBuf> {
    fs::create_dir_all(home)?;
    let root = home.join(".manox");
    fs::create_dir_all(&root)?;
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn temp_root(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new().prefix(name).tempdir().unwrap()
    }

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn installing_creates_the_app_home_and_the_manox_layer() {
        let root = temp_root("steer-paths-install");
        let home = root.path().join(".steer");
        assert_eq!(install_root(&home).unwrap(), home.join(".manox"));
        assert!(home.is_dir());
        assert!(home.join(".manox").is_dir());
    }

    /// A pre-split `~/.manox` beside the app home is left exactly as it is:
    /// state is never carried between roots, and a mover here could drain the
    /// live root whenever the stale one held same-named paths.
    #[test]
    fn installing_never_touches_a_pre_split_root() {
        let root = temp_root("steer-paths-legacy");
        let legacy = root.path().join(".manox");
        write(&legacy.join("sessions/keep.jsonl"), "conversation");
        let home = root.path().join(".steer");

        install_root(&home).unwrap();

        assert_eq!(
            std::fs::read_to_string(legacy.join("sessions/keep.jsonl")).unwrap(),
            "conversation"
        );
        assert_eq!(std::fs::read_dir(&legacy).unwrap().count(), 1);
    }
}
