//! Minimal `ui_language` access on `~/.manox/settings.toml`.
//!
//! The manox runtime no longer knows about language at all, so this app owns
//! the `ui_language` key in the shared settings file. Reads and writes touch
//! only that key: the file's other tables and any unknown keys are preserved
//! byte-for-byte in value, because the runtime owns the rest of the schema and
//! this crate must never round-trip a document it does not fully model.

use anyhow::{Context as _, Result};
use std::path::PathBuf;

use crate::Language;

/// Return `~/.manox/settings.toml`.
fn settings_file() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".manox").join("settings.toml"))
}

/// Read the persisted UI language. A missing file, a missing key, or an
/// unrecognized token all resolve to [`Language::default`] (Simplified
/// Chinese). An unrecognized token warns once rather than silently coercing, so
/// a typo in a hand-edited file is diagnosable.
pub fn load_ui_language() -> Language {
    let Ok(path) = settings_file() else {
        return Language::default();
    };
    let raw = match std::fs::read_to_string(&path) {
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
    let path = settings_file()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create settings dir {}", parent.display()))?;
    }
    // Parse-edit-serialize keeps every other key intact; a parse failure is
    // recovered by starting from an empty document so a corrupt file cannot
    // permanently block a language change.
    let mut doc: toml::Value = match std::fs::read_to_string(&path) {
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
    let serialized = toml::to_string_pretty(&doc).context("serialize settings.toml")?;
    std::fs::write(&path, serialized).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}
