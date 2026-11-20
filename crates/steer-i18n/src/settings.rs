//! Minimal `ui_language` access on the app-layer `<steer_home>/settings.toml`.
//!
//! The Steer platform owns the application state root and this crate owns the
//! `ui_language` key of its settings document. Reads and writes touch only
//! that key: the file's other keys are preserved byte-for-byte in value,
//! because future app keys may share the document and this crate must never
//! round-trip a document it does not fully model.
//!
//! The pre-split layout kept the key on the runtime's settings document
//! (`<manox_root>/settings.toml`). The first touch of the new document
//! extracts the key from there and deletes it from the legacy document —
//! one-time, idempotent, and skipping silently when the new document already
//! exists or the legacy one never carried the key.

use anyhow::{Context as _, Result};
use std::path::{Path, PathBuf};

use crate::Language;
use steer_paths::{manox_root, steer_home};

/// Return the app-layer settings document: `<steer_home>/settings.toml`.
fn app_settings_file() -> Result<PathBuf> {
    steer_home()
        .map(|home| home.join("settings.toml"))
        .context("no home directory; nowhere to root the app settings")
}

/// The runtime's settings document — the extraction source only; after the
/// first extraction nothing here reads it again.
fn legacy_settings_file() -> Option<PathBuf> {
    manox_root().map(|root| root.join("settings.toml"))
}

/// Read the persisted UI language. A missing file, a missing key, or an
/// unrecognized token all resolve to [`Language::default`] (Simplified
/// Chinese). An unrecognized token warns once rather than silently coercing, so
/// a typo in a hand-edited file is diagnosable.
pub fn load_ui_language() -> Language {
    let Ok(path) = app_settings_file() else {
        return Language::default();
    };
    if !path.exists() {
        extract_legacy_language(&path);
    }
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

/// One-time extraction of `ui_language` from the legacy runtime settings
/// document. Best effort by construction: every failure just leaves the
/// default language and a warning behind, and the extraction retries on the
/// next launch until the new document exists.
fn extract_legacy_language(new_path: &Path) {
    let Some(old_path) = legacy_settings_file() else {
        return;
    };
    extract_legacy_language_at(new_path, &old_path);
}

/// The path-explicit extraction core (the env-resolving wrapper is a thin
/// shell over it, so tests exercise the same code without touching env).
fn extract_legacy_language_at(new_path: &Path, old_path: &Path) {
    let Ok(raw) = std::fs::read_to_string(old_path) else {
        return;
    };
    let Ok(doc) = toml::from_str::<toml::Value>(&raw) else {
        tracing::warn!(
            path = %old_path.display(),
            "legacy settings.toml does not parse; ui_language extraction skipped"
        );
        return;
    };
    let Some(token) = doc.get("ui_language").and_then(|v| v.as_str()) else {
        return;
    };
    let mut new_doc = toml::Value::Table(toml::map::Map::new());
    new_doc
        .as_table_mut()
        .expect("freshly built document is a table")
        .insert(
            "ui_language".to_string(),
            toml::Value::String(token.to_string()),
        );
    if write_document(new_path, &new_doc).is_err() {
        return;
    }
    drop_key_from_document(old_path, "ui_language");
}

/// Persist `ui_language`, rewriting only that key. Creates the file (and its
/// parent directory) when absent.
pub fn persist_ui_language(lang: Language) -> Result<()> {
    let path = app_settings_file()?;
    persist_at(&path, lang)
}

/// The path-explicit persistence core (see `extract_legacy_language_at`).
fn persist_at(path: &Path, lang: Language) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create settings dir {}", parent.display()))?;
    }
    // Parse-edit-serialize keeps every other key intact; a parse failure is
    // recovered by starting from an empty document so a corrupt file cannot
    // permanently block a language change.
    let mut doc: toml::Value = match std::fs::read_to_string(path) {
        Ok(raw) => toml::from_str(&raw).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "settings.toml parse failed; rewriting it from scratch");
            toml::Value::Table(toml::map::Map::new())
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            toml::Value::Table(toml::map::Map::new())
        }
        Err(e) => return Err(e).context("read settings.toml"),
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

/// Remove one key from a document in place. The document may carry keys this
/// crate does not model (the runtime shares the legacy file), so a parse
/// failure is refused rather than rewritten from an empty doc.
fn drop_key_from_document(path: &Path, key: &str) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(mut doc) = toml::from_str::<toml::Value>(&raw) else {
        tracing::warn!(
            path = %path.display(),
            "{key} deletion skipped: document does not parse"
        );
        return;
    };
    if doc
        .as_table_mut()
        .map(|table| table.remove(key).is_none())
        .unwrap_or(true)
    {
        return;
    }
    if let Err(e) = write_document(path, &doc) {
        tracing::warn!(error = %e, "failed to rewrite the legacy settings document");
    }
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
    fn extraction_moves_the_key_and_deletes_it_from_the_legacy_document() {
        let root = tempfile::tempdir().unwrap();
        let new = root.path().join("steer/settings.toml");
        let old = root.path().join("manox/settings.toml");
        write_file(
            &old,
            "ui_language = \"en\"\n[journal]\ncompact = true\n[[project]]\nid = \"p\"\n",
        );
        extract_legacy_language_at(&new, &old);
        assert!(new.is_file());
        assert_eq!(read_language_at(&new), Language::En);
        let old_raw = std::fs::read_to_string(&old).unwrap();
        assert!(!old_raw.contains("ui_language"));
        assert!(old_raw.contains("[journal]"), "sibling keys survive");
        assert!(
            old_raw.contains("[[project]]"),
            "other owners' keys survive"
        );
    }

    #[test]
    fn extraction_skips_when_the_legacy_document_lacks_the_key() {
        let root = tempfile::tempdir().unwrap();
        let new = root.path().join("steer/settings.toml");
        let old = root.path().join("manox/settings.toml");
        write_file(&old, "[journal]\ncompact = true\n");
        extract_legacy_language_at(&new, &old);
        assert!(!new.exists(), "no key, no new document");
        assert_eq!(
            std::fs::read_to_string(&old).unwrap(),
            "[journal]\ncompact = true\n"
        );
    }

    #[test]
    fn extraction_is_refused_by_an_unparseable_legacy_document() {
        let root = tempfile::tempdir().unwrap();
        let new = root.path().join("steer/settings.toml");
        let old = root.path().join("manox/settings.toml");
        write_file(&old, "ui_language = \"en\"\nbroken ====");
        extract_legacy_language_at(&new, &old);
        assert!(!new.exists());
        // The legacy document is left exactly as found.
        assert_eq!(
            std::fs::read_to_string(&old).unwrap(),
            "ui_language = \"en\"\nbroken ===="
        );
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
}
