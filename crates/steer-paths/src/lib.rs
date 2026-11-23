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
//! into the default `<steer_home>/.manox`. That is a filesystem move, not a
//! schema migration: documents are carried verbatim and the app-layer keys
//! are extracted to their new documents by the owners (`steer-i18n`,
//! `agent-ui::projects`), each on first touch of its own document. A
//! `STEER_HOME` override never runs the migration — the override directory
//! is a fresh root by construction (see [`steer_home`]).

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Steer platform state root: `~/.steer`. `STEER_HOME` overrides it wholesale
/// (same precedence discipline as `MANOX_HOME`; the override exists for
/// hermetic tests and isolated dev instances). An override names a fresh
/// root by construction: nothing from the default root is carried into it —
/// in particular [`install`] skips the pre-split `~/.manox` migration, so an
/// isolated instance can never rename the real runtime root away from the
/// default installation. `None` means no resolvable home — callers fall back
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
/// tests): respected as-is, no injection, no migration. A `STEER_HOME`
/// override is the same contract against the app layer: the override
/// directory is a fresh root (see [`steer_home`]), so only `MANOX_HOME` is
/// injected and the pre-split migration never runs — an isolated instance
/// must not pull the real `~/.manox` into the override directory. A
/// migration failure falls back to the legacy root instead of abandoning
/// the user's data — the next launch retries the rename.
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
    // The migration only ever runs against the default home; an overridden
    // root has no pre-split history to carry (see above). `None` is also the
    // shape when no legacy root exists on disk.
    let legacy = if env::var_os("STEER_HOME").is_some_and(|dir| !dir.is_empty()) {
        None
    } else {
        dirs::home_dir()
            .map(|home| home.join(".manox"))
            .filter(|root| root.exists())
    };
    let root = install_root(&home, legacy.as_deref())?;
    // SAFETY: process-startup, before any thread exists or any manox path is
    // resolved; the runtime reads this lazily at call time.
    unsafe { env::set_var("MANOX_HOME", &root) };
    Ok(())
}

/// The path-explicit core of [`install`]: create `<home>`, carry `legacy`
/// into `<home>/.manox` when given, and return the effective runtime root.
/// `legacy` is `None` when nothing may be carried — an overridden root (a
/// fresh root by construction) or an absent legacy root.
fn install_root(home: &Path, legacy: Option<&Path>) -> io::Result<PathBuf> {
    fs::create_dir_all(home)?;
    let target = home.join(".manox");
    let root = match legacy {
        Some(legacy) => match migrate_legacy_root(legacy, &target) {
            Ok(()) => target,
            Err(error) => {
                eprintln!(
                    "steer: state migration from {} to {} failed: {error}; using the legacy root",
                    legacy.display(),
                    target.display()
                );
                legacy.to_path_buf()
            }
        },
        None => target,
    };
    fs::create_dir_all(&root)?;
    Ok(root)
}

/// Per-entry move for the shape a whole rename cannot cover: the target
/// already exists (an interrupted earlier migration) or the rename failed.
/// An entry present on both sides resolves to the LEGACY copy: whenever both
/// sides hold an entry, the migration had fallen back to the legacy root
/// (failed migration, crash or partial copy mid-move), and while that
/// fallback was live the legacy root is the one every write landed in — the
/// target's copy is stale data from the interrupted attempt. Removing the
/// target's copy first also lets the rename below replace it instead of
/// landing inside it. Entries that rename cannot carry (e.g. across a
/// device boundary) fall back to a recursive copy + delete.
fn move_entries(legacy: &Path, target: &Path) -> io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(legacy)? {
        let entry = entry?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        // `symlink_metadata` keeps a symlink on the target side a plain
        // unlink (never a descent into whatever it points at).
        if let Ok(meta) = fs::symlink_metadata(&to) {
            if meta.is_dir() {
                fs::remove_dir_all(&to)?;
            } else {
                fs::remove_file(&to)?;
            }
        }
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

    /// An entry present on both sides resolves to the legacy copy: the app
    /// wrote into the legacy root while the migration was falling back to
    /// it, so the target's copy is stale. Entries only the legacy side has
    /// still move in.
    #[test]
    fn an_entry_on_both_sides_resolves_to_the_legacy_copy() {
        let root = temp_root("steer-paths-sweep");
        touch(
            &root.path().join("legacy/journal/log.v1.jsonl"),
            "written during the fallback",
        );
        touch(
            &root.path().join("target/journal/log.v1.jsonl"),
            "stale from the interrupted attempt",
        );
        touch(&root.path().join("legacy/settings.toml"), "key = 1");
        migrate_legacy_root(&root.path().join("legacy"), &root.path().join("target")).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("target/journal/log.v1.jsonl")).unwrap(),
            "written during the fallback"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("target/settings.toml")).unwrap(),
            "key = 1"
        );
        assert!(!root.path().join("legacy/journal/log.v1.jsonl").exists());
        assert!(!root.path().join("legacy/settings.toml").exists());
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

    /// The default-home shape of `install_root`: the legacy root is carried
    /// into `<home>/.manox` and drained.
    #[test]
    fn the_default_home_carries_the_legacy_root() {
        let root = temp_root("steer-paths-install");
        let legacy = root.path().join("legacy-manox");
        touch(&legacy.join("journal/log.v1.jsonl"), "data");
        let home = root.path().join("home");
        assert_eq!(
            install_root(&home, Some(&legacy)).unwrap(),
            home.join(".manox")
        );
        assert_eq!(
            std::fs::read_to_string(home.join(".manox/journal/log.v1.jsonl")).unwrap(),
            "data"
        );
        assert!(!legacy.join("journal/log.v1.jsonl").exists());
    }

    /// The override shape (`legacy: None`): a fresh `.manox` is created and
    /// the real legacy root is left exactly as it was — an isolated instance
    /// must never rename the default installation's runtime root away.
    #[test]
    fn an_override_root_never_touches_the_legacy_root() {
        let root = temp_root("steer-paths-override");
        let legacy = root.path().join("real-manox");
        touch(&legacy.join("journal/log.v1.jsonl"), "real data");
        let home = root.path().join("override-home");
        install_root(&home, None).unwrap();
        assert_eq!(
            std::fs::read_to_string(legacy.join("journal/log.v1.jsonl")).unwrap(),
            "real data"
        );
        assert!(home.join(".manox").is_dir());
        assert_eq!(std::fs::read_dir(home.join(".manox")).unwrap().count(), 0);
    }
}
