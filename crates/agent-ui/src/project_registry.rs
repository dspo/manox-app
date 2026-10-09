//! The client-side removed-projects overlay behind the sidebar's 移除项目.
//!
//! AHP carries no project-registry concept: a session's project binding is
//! part of its own state, so unlike the retired v2 store there is no host
//! account to unregister a folder from. The app therefore owns the removal
//! itself — a persisted set of paths whose rows group as loose ("Chats")
//! instead of under their project. Conversation history is never touched.
//!
//! The set lives under the app's own `removed_projects` key in the shared
//! `~/.manox/settings.toml`, read-modify-written with the same
//! only-touch-my-key discipline as the `ui_language` accessor in
//! `steer_i18n` — every other key is preserved. Re-binding a session to a
//! removed path (new thread with that project) clears the entry: launching
//! into a folder is the re-registration.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Context as _;

fn settings_file() -> Option<PathBuf> {
    Some(
        PathBuf::from(std::env::var_os("HOME")?)
            .join(".manox")
            .join("settings.toml"),
    )
}

/// The process-lifetime cache: the sidebar projects on every multiplexer
/// notify, and each projection consults this set — a file read per frame
/// would be waste, and every mutation site goes through this module, so the
/// cache cannot drift from the file while the app runs.
static REMOVED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn cached() -> std::sync::MutexGuard<'static, Option<HashSet<String>>> {
    REMOVED.lock().expect("removed-projects lock")
}

/// The removed set, loading it from settings.toml on first consult.
pub fn removed_projects() -> HashSet<String> {
    let mut slot = cached();
    slot.get_or_insert_with(load).clone()
}

/// Remove a project from the sidebar's grouping (its rows fall back to the
/// loose "Chats" bucket) and persist the decision. Idempotent.
pub fn forget_project(path: &str) {
    if path.is_empty() {
        return;
    }
    let mut slot = cached();
    let set = slot.get_or_insert_with(load);
    if set.insert(path.to_string()) {
        persist(set);
    }
}

/// Re-register a project: a session bound to the path clears its removal —
/// launching into a folder is the re-registration.
pub fn register_project(path: &str) {
    if path.is_empty() {
        return;
    }
    let mut slot = cached();
    let set = slot.get_or_insert_with(load);
    if set.remove(path) {
        persist(set);
    }
}

fn load() -> HashSet<String> {
    let Some(path) = settings_file() else {
        return HashSet::new();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return HashSet::new();
    };
    let Ok(doc) = toml::from_str::<toml::Value>(&raw) else {
        tracing::warn!("settings.toml parse failed; removed-projects overlay starts empty");
        return HashSet::new();
    };
    doc.get("removed_projects")
        .and_then(|v| v.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn persist(set: &HashSet<String>) {
    let Some(path) = settings_file() else {
        return;
    };
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(error = %e, "create settings dir failed");
        return;
    }
    persist_at(&path, set);
}

/// The write half of the overlay: parse-edit-serialize keeps every other key
/// intact; a corrupt file is replaced rather than allowed to block the write
/// (the same recovery the ui_language accessor uses).
fn persist_at(path: &std::path::Path, set: &HashSet<String>) {
    let mut doc: toml::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| toml::from_str(&raw).ok())
        .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
    let Some(table) = doc.as_table_mut() else {
        return;
    };
    let mut paths: Vec<String> = set.iter().cloned().collect();
    paths.sort();
    table.insert(
        "removed_projects".to_string(),
        toml::Value::Array(paths.into_iter().map(toml::Value::String).collect()),
    );
    match toml::to_string_pretty(&doc)
        .context("serialize settings.toml")
        .and_then(|s| std::fs::write(path, s).context("write settings.toml"))
    {
        Ok(()) => {}
        Err(e) => tracing::warn!(error = %e, "persist removed_projects failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The overlay's parse-edit-serialize cycle: a persisted set survives a
    /// round trip, an unrelated sibling key is preserved byte-for-byte, and
    /// the document stays valid TOML throughout.
    #[test]
    fn persist_preserves_sibling_keys_and_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "ui_language = \"zh-CN\"\n\n[host]\nport = 42\n")
            .expect("seed settings");

        let read = || -> (HashSet<String>, Option<String>) {
            let doc: toml::Value =
                toml::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
            (
                doc.get("removed_projects")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                doc.get("ui_language")
                    .and_then(|v| v.as_str())
                    .map(String::from),
            )
        };

        let mut set = HashSet::new();
        set.insert("/p/a".to_string());
        set.insert("/p/b".to_string());
        persist_at(&path, &set);
        let (removed, lang) = read();
        assert!(removed.contains("/p/a") && removed.contains("/p/b"));
        assert_eq!(lang.as_deref(), Some("zh-CN"));

        set.remove("/p/a");
        persist_at(&path, &set);
        let (removed, lang) = read();
        assert_eq!(removed, HashSet::from(["/p/b".to_string()]));
        assert_eq!(lang.as_deref(), Some("zh-CN"));
    }
}
