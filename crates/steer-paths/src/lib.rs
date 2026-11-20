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
//! [`install`] also performs the one-time rename of a pre-split `~/.manox`
//! into `<steer_home>/.manox`. That is a filesystem move, not a schema
//! migration: documents are carried verbatim and the app-layer keys are
//! extracted to their new documents by the owners (`steer-i18n`,
//! `agent-ui::projects`), each on first touch of its own document.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Steer platform state root: `~/.steer`. `STEER_HOME` overrides it wholesale
/// (same precedence discipline as `MANOX_HOME`; the override exists for
/// hermetic tests). `None` means no resolvable home — callers fall back to
/// defaults instead of guessing a path.
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

/// Move the pre-split legacy root into the layered one. A whole-directory
/// rename when the target is absent; otherwise a per-entry move that leaves
/// already-migrated entries alone (an interrupted earlier run is repaired,
/// not duplicated). Idempotent: no legacy root, nothing to do.
pub fn migrate_legacy_root(legacy: &Path, target: &Path) -> io::Result<()> {
    if !legacy.exists() {
        return Ok(());
    }
    if !target.exists() && fs::rename(legacy, target).is_ok() {
        return Ok(());
    }
    move_entries(legacy, target)
}

/// Install the process-wide state-root contract: create `<steer_home>`,
/// migrate a pre-split `~/.manox` into it, and inject `MANOX_HOME` — all
/// before any manox path resolution (the runtime resolves paths lazily, but
/// the injection must land first to be authoritative). Call once at process
/// start, from `main()`.
///
/// A preset `MANOX_HOME` is an isolation contract (developers, hermetic
/// tests): respected as-is, no injection, no migration. A migration failure
/// falls back to the legacy root instead of abandoning the user's data —
/// the next launch retries the rename.
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
    fs::create_dir_all(&home)?;
    let target = home.join(".manox");
    let root = match dirs::home_dir().map(|home| home.join(".manox")) {
        Some(legacy) if legacy.exists() => match migrate_legacy_root(&legacy, &target) {
            Ok(()) => target,
            Err(error) => {
                eprintln!(
                    "steer: state migration from {} to {} failed: {error}; using the legacy root",
                    legacy.display(),
                    target.display()
                );
                legacy
            }
        },
        _ => target,
    };
    fs::create_dir_all(&root)?;
    // SAFETY: process-startup, before any thread exists or any manox path is
    // resolved; the runtime reads this lazily at call time.
    unsafe { env::set_var("MANOX_HOME", &root) };
    Ok(())
}

/// Per-entry move for the shape a whole rename cannot cover: the target
/// already exists (an interrupted earlier migration) or the rename failed.
/// Missing entries move in; present ones stay. Entries that rename cannot
/// carry (e.g. across a device boundary) fall back to a recursive copy +
/// delete.
fn move_entries(legacy: &Path, target: &Path) -> io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(legacy)? {
        let entry = entry?;
        let to = target.join(entry.file_name());
        if to.exists() {
            continue;
        }
        let from = entry.path();
        let is_dir = entry.file_type().is_ok_and(|ty| ty.is_dir());
        match fs::rename(&from, &to) {
            Ok(()) => {}
            Err(_) if is_dir => {
                // A non-renameable directory is copied recursively and the
                // original removed only after the copy is complete; the
                // source stays behind on failure, keeping the legacy root
                // consistent.
                copy_tree(&from, &to)?;
                fs::remove_dir_all(&from)?;
            }
            Err(_) => {
                fs::copy(&from, &to)?;
                fs::remove_file(&from)?;
            }
        }
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &dest)?;
        } else {
            fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new().prefix(name).tempdir().unwrap()
    }

    fn touch(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn legacy_absent_is_a_no_op() {
        let root = temp_root("steer-paths-noop");
        assert!(
            migrate_legacy_root(&root.path().join("legacy"), &root.path().join("target")).is_ok()
        );
    }

    #[test]
    fn legacy_renames_into_an_absent_target() {
        let root = temp_root("steer-paths-rename");
        touch(&root.path().join("legacy/journal/log.v1.jsonl"), "{}");
        migrate_legacy_root(&root.path().join("legacy"), &root.path().join("target")).unwrap();
        assert!(!root.path().join("legacy").exists());
        assert!(root.path().join("target/journal/log.v1.jsonl").is_file());
    }

    #[test]
    fn an_existing_target_sweeps_only_missing_entries() {
        let root = temp_root("steer-paths-sweep");
        touch(&root.path().join("legacy/journal/log.v1.jsonl"), "moved");
        touch(&root.path().join("legacy/settings.toml"), "key = 1");
        touch(&root.path().join("target/journal/log.v1.jsonl"), "kept");
        migrate_legacy_root(&root.path().join("legacy"), &root.path().join("target")).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("target/journal/log.v1.jsonl")).unwrap(),
            "kept"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("target/settings.toml")).unwrap(),
            "key = 1"
        );
    }

    #[test]
    fn migration_is_idempotent() {
        let root = temp_root("steer-paths-idempotent");
        touch(&root.path().join("legacy/journal/log.v1.jsonl"), "{}");
        let (legacy, target) = (root.path().join("legacy"), root.path().join("target"));
        migrate_legacy_root(&legacy, &target).unwrap();
        migrate_legacy_root(&legacy, &target).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("journal/log.v1.jsonl")).unwrap(),
            "{}"
        );
    }
}
