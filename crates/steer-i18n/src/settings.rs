//! Minimal `ui_language` access on the app-layer `<steer_home>/settings.toml`.
//!
//! The Steer platform owns the application state root and this crate owns the
//! `ui_language` key of its settings document. Reads and writes touch only
//! that key: the file's other keys are preserved byte-for-byte in value,
//! because future app keys may share the document and this crate must never
//! round-trip a document it does not fully model — a document that does not
//! parse is refused rather than rewritten, so persisting can never wipe the
//! keys it does not model.
//!
//! The document is the app layer's own: no other document is read, and
//! nothing is carried between documents.

use anyhow::{Context as _, Result};
use std::path::{Path, PathBuf};

use crate::Language;
use steer_paths::steer_home;

/// Return the app-layer settings document: `<steer_home>/settings.toml`.
fn app_settings_file() -> Result<PathBuf> {
    steer_home()
        .map(|home| home.join("settings.toml"))
        .context("no home directory; nowhere to root the app settings")
}

/// Read the persisted UI language. A missing file, a missing key, or an
/// unrecognized token all resolve to [`Language::default`] (Simplified
/// Chinese). An unrecognized token warns once rather than silently coercing, so
/// a typo in a hand-edited file is diagnosable.
pub fn load_ui_language() -> Language {
    let Ok(path) = app_settings_file() else {
        return Language::default();
    };
    read_language_at(&path)
}

fn read_language_at(path: &Path) -> Language {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Language::default(),
        Err(e) => {
            tracing::warn!(error = %e, "settings.toml read failed; using default UI language");
            return Language::default();
        }
    };
    let doc: toml::Value = match toml::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "settings.toml parse failed; using default UI language");
            return Language::default();
        }
    };
    let Some(token) = doc.get("ui_language").and_then(|v| v.as_str()) else {
        return Language::default();
    };
    match Language::from_token(token) {
        Some(lang) => lang,
        None => {
            tracing::warn!(token, "unknown ui_language token; defaulting to zh-CN");
            Language::default()
        }
    }
}

/// Persist `ui_language`, rewriting only that key. Creates the file (and its
/// parent directory) when absent.
pub fn persist_ui_language(lang: Language) -> Result<()> {
    let path = app_settings_file()?;
    persist_at(&path, lang)
}

/// The path-explicit persistence core (tests call this directly so the
/// env-resolving wrapper never needs a mutated environment).
fn persist_at(path: &Path, lang: Language) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create settings dir {}", parent.display()))?;
    }
    // Parse-edit-serialize keeps every other key intact. A file that does
    // not parse is refused, never rewritten from an empty doc: the empty
    // start would silently wipe the keys this crate does not model. A
    // missing file is the first-boot shape — an empty doc is the whole
    // truth there.
    let mut doc: toml::Value = match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            toml::Value::Table(toml::map::Map::new())
        }
        raw => toml::from_str(&raw.context("read settings.toml")?)
            .context("settings.toml does not parse; refusing an empty-doc rewrite")?,
    };
    let table = doc
        .as_table_mut()
        .context("settings.toml top level is not a table")?;
    table.insert(
        "ui_language".to_string(),
        toml::Value::String(lang.token().to_string()),
    );
    write_document(path, &doc)?;
    Ok(())
}

/// Serialize and swap a document into place atomically (sibling temp file +
/// rename): a crash mid-write leaves the previous content intact instead of a
/// truncated file the next read would treat as empty.
fn write_document(path: &Path, doc: &toml::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create settings dir {}", parent.display()))?;
    }
    let serialized = toml::to_string_pretty(doc).context("serialize settings.toml")?;
    let temp = path.with_extension("toml.tmp");
    std::fs::write(&temp, serialized).with_context(|| format!("write {}", temp.display()))?;
    std::fs::rename(&temp, path).with_context(|| format!("swap {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn write_file(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn persistence_round_trips_through_the_app_document() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("steer/settings.toml");
        persist_at(&path, Language::En).unwrap();
        assert_eq!(read_language_at(&path), Language::En);
        persist_at(&path, Language::ZhCn).unwrap();
        assert_eq!(read_language_at(&path), Language::ZhCn);
    }

    /// A file that does not parse must never be rewritten from an empty doc:
    /// other keys may share the settings document, and the empty-doc start
    /// would silently wipe them. persist_at refuses and the corrupt bytes
    /// stay untouched; a missing file is the first-boot shape and persists
    /// fine.
    #[test]
    fn persistence_refuses_an_unparseable_file_instead_of_wiping_it() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("steer/settings.toml");
        let corrupt = "ui_language = \"en\"\nbroken ====";
        write_file(&path, corrupt);
        assert!(
            persist_at(&path, Language::ZhCn).is_err(),
            "an unparseable file blocks the write"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), corrupt);

        let fresh = root.path().join("fresh/settings.toml");
        persist_at(&fresh, Language::En).unwrap();
        assert_eq!(read_language_at(&fresh), Language::En);
    }
}
