//! The client-side project registry behind the sidebar's project grouping:
//! the removed-projects overlay, the added-at stamps, and the manual group
//! order.
//!
//! AHP carries no project-registry concept: a session's project binding is
//! part of its own state, so unlike the retired v2 store there is no host
//! account to unregister a folder from. The app therefore owns the registry
//! itself, persisted under its own keys in the shared `~/.manox/settings.toml`
//! (read-modify-written with the same only-touch-my-keys discipline as the
//! `ui_language` accessor in `steer_i18n` — every other key is preserved):
//!
//! - `removed_projects`: paths whose rows group as loose ("Chats") instead of
//!   under their project. Conversation history is never touched.
//! - `project_added`: path → unix seconds — a path never seen before is
//!   stamped on its first appearance; re-binding does not move the stamp.
//!   The DEFAULT project order is newest-added first.
//! - `project_order`: the manual drag order. It decides the final order of
//!   the projects it lists; a project it does not list falls back to the
//!   default (newest first) and lands ABOVE the manually ordered ones.
//!
//! Re-binding a session to a removed path (a new thread with that project)
//! clears the removal and drops its manual-order entry: launching into a
//! folder is the re-registration, and the first-appearance stamp is not
//! part of it.

use std::collections::{HashMap, HashSet};
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

/// The whole registry, cached for the process lifetime: the sidebar projects
/// on every multiplexer notify, and each projection consults it — a file read
/// per frame would be waste, and every mutation site goes through this
/// module, so the cache cannot drift from the file while the app runs.
#[derive(Default)]
struct Registry {
    removed: HashSet<String>,
    /// path → unix seconds of first appearance.
    added: HashMap<String, i64>,
    /// The manual drag order, as recorded.
    order: Vec<String>,
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

fn cached() -> std::sync::MutexGuard<'static, Option<Registry>> {
    REGISTRY.lock().expect("project registry lock")
}

/// The removed set, loading the registry from settings.toml on first consult.
pub fn removed_projects() -> HashSet<String> {
    let mut slot = cached();
    slot.get_or_insert_with(load).removed.clone()
}

/// Remove a project from the sidebar's grouping (its rows fall back to the
/// loose "Chats" bucket) and persist the decision. Idempotent.
pub fn forget_project(path: &str) {
    if path.is_empty() {
        return;
    }
    let mut slot = cached();
    let reg = slot.get_or_insert_with(load);
    if reg.removed.insert(path.to_string()) {
        persist(reg);
    }
}

/// Re-register a project: a session bound to the path clears its removal
/// and drops its manual-order entry — the group floats back above the
/// manually ordered block. The first-appearance stamp stays where it is:
/// it names when the folder first appeared, not when it came back.
pub fn register_project(path: &str) {
    if path.is_empty() {
        return;
    }
    let mut slot = cached();
    let reg = slot.get_or_insert_with(load);
    let mut changed = reg.removed.remove(path);
    if !reg.added.contains_key(path) {
        reg.added.insert(path.to_string(), now_secs());
        changed = true;
    }
    let before = reg.order.len();
    reg.order.retain(|k| k != path);
    if reg.order.len() != before {
        changed = true;
    }
    if changed {
        persist(reg);
    }
}

/// Stamp every not-yet-seen path with now (its first appearance in the
/// sidebar). Persists at most once per call, and only when something was
/// stamped.
pub fn note_seen_projects(paths: &[String]) {
    let mut slot = cached();
    let reg = slot.get_or_insert_with(load);
    // One clock read for the whole batch: per-path reads could straddle a
    // second boundary and stamp one batch with an ordering it never had.
    let now = now_secs();
    let mut changed = false;
    for path in paths.iter().filter(|p| !p.is_empty()) {
        changed |= reg.added.insert(path.clone(), now).is_none();
    }
    if changed {
        persist(reg);
    }
}

/// The recorded manual order (verbatim; may name projects with no rows).
pub fn manual_order() -> Vec<String> {
    let mut slot = cached();
    slot.get_or_insert_with(load).order.clone()
}

/// Replace the recorded manual order.
pub fn set_manual_order(order: Vec<String>) {
    let mut slot = cached();
    let reg = slot.get_or_insert_with(load);
    if reg.order != order {
        reg.order = order;
        persist(reg);
    }
}

/// The display order for the currently existing group keys: the keys the
/// manual order does not list come first, newest-added first (keys without a
/// stamp — the loose "Chats" bucket — read as the oldest, so they trail);
/// then the listed keys in their recorded order.
pub fn order_groups(keys: &[String]) -> Vec<String> {
    let mut slot = cached();
    order_in(slot.get_or_insert_with(load), keys)
}

fn order_in(reg: &Registry, keys: &[String]) -> Vec<String> {
    let mut unrecorded: Vec<&String> = keys.iter().filter(|k| !reg.order.contains(k)).collect();
    unrecorded.sort_by(|a, b| {
        let ta = reg.added.get(a.as_str()).copied().unwrap_or(i64::MIN);
        let tb = reg.added.get(b.as_str()).copied().unwrap_or(i64::MIN);
        tb.cmp(&ta)
    });
    let recorded: Vec<&String> = reg
        .order
        .iter()
        .filter_map(|k| keys.iter().find(|e| e == &k))
        .collect();
    unrecorded.into_iter().chain(recorded).cloned().collect()
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn load() -> Registry {
    settings_file()
        .map(|path| load_at(&path))
        .unwrap_or_default()
}

/// The registry parse of one settings file — the loader the cache uses and
/// the round-trip test asserts against (one parse, not two that can drift).
fn load_at(path: &std::path::Path) -> Registry {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Registry::default();
    };
    let Ok(doc) = toml::from_str::<toml::Value>(&raw) else {
        tracing::warn!("settings.toml parse failed; project registry starts empty");
        return Registry::default();
    };
    Registry {
        removed: string_set(doc.get("removed_projects")),
        added: doc
            .get("project_added")
            .and_then(|v| v.as_table())
            .map(|table| {
                table
                    .iter()
                    .filter_map(|(k, v)| v.as_integer().map(|secs| (k.clone(), secs)))
                    .collect()
            })
            .unwrap_or_default(),
        order: doc
            .get("project_order")
            .and_then(|v| v.as_array())
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn string_set(value: Option<&toml::Value>) -> HashSet<String> {
    value
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

fn persist(reg: &Registry) {
    let Some(path) = settings_file() else {
        return;
    };
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(error = %e, "create settings dir failed");
        return;
    }
    persist_at(&path, reg);
}

/// The write half: parse-edit-serialize keeps every other key intact; a
/// corrupt file is replaced rather than allowed to block the write (the same
/// recovery the ui_language accessor uses).
fn persist_at(path: &std::path::Path, reg: &Registry) {
    let mut doc: toml::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| toml::from_str(&raw).ok())
        .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
    let Some(table) = doc.as_table_mut() else {
        return;
    };
    let mut removed: Vec<String> = reg.removed.iter().cloned().collect();
    removed.sort();
    table.insert(
        "removed_projects".to_string(),
        toml::Value::Array(removed.into_iter().map(toml::Value::String).collect()),
    );
    let mut added: Vec<(&String, &i64)> = reg.added.iter().collect();
    added.sort_by(|a, b| a.0.cmp(b.0));
    let added_table: toml::map::Map<String, toml::Value> = added
        .into_iter()
        .map(|(k, v)| (k.clone(), toml::Value::Integer(*v)))
        .collect();
    table.insert("project_added".to_string(), toml::Value::Table(added_table));
    table.insert(
        "project_order".to_string(),
        toml::Value::Array(reg.order.iter().cloned().map(toml::Value::String).collect()),
    );
    match toml::to_string_pretty(&doc)
        .context("serialize settings.toml")
        .and_then(|s| std::fs::write(path, s).context("write settings.toml"))
    {
        Ok(()) => {}
        Err(e) => tracing::warn!(error = %e, "persist project registry failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_settings() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "ui_language = \"zh-CN\"\n\n[host]\nport = 42\n").expect("seed");
        (dir, path)
    }

    /// The parse-edit-serialize cycle: a persisted registry survives a round
    /// trip and an unrelated sibling key is preserved.
    #[test]
    fn persist_preserves_sibling_keys_and_round_trips() {
        let (_dir, path) = temp_settings();
        let reg = Registry {
            removed: ["/p/a".to_string()].into_iter().collect(),
            added: HashMap::from([("/p/b".to_string(), 7)]),
            order: vec!["/p/b".to_string(), "Chats".to_string()],
        };
        persist_at(&path, &reg);

        // Round-trip through the same loader shape the cache uses.
        std::fs::write(
            &path,
            std::fs::read_to_string(&path)
                .unwrap()
                .replace("/p/a", "/p/c"),
        )
        .unwrap();
        let loaded = load_at(&path);
        assert_eq!(loaded.removed, ["/p/c".to_string()].into_iter().collect());
        assert_eq!(loaded.added.get("/p/b"), Some(&7));
        assert_eq!(loaded.order, vec!["/p/b".to_string(), "Chats".to_string()]);
        let doc: toml::Value =
            toml::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
        assert_eq!(
            doc.get("ui_language").and_then(|v| v.as_str()),
            Some("zh-CN")
        );
    }

    /// The display order: keys the manual order does not list come first,
    /// newest-added first (unstamped keys trail), then the listed keys in
    /// their recorded order — and a listed key with no current rows is
    /// simply absent.
    #[test]
    fn order_in_newest_unlisted_first_then_manual_order() {
        let mut reg = Registry::default();
        reg.added.insert("/p/old".into(), 100);
        reg.added.insert("/p/new".into(), 300);
        reg.added.insert("/p/mid".into(), 200);
        reg.order = vec!["/p/mid".into(), "/p/gone".into(), "Chats".into()];
        let keys = vec![
            "/p/old".to_string(),
            "Chats".to_string(),
            "/p/new".to_string(),
            "/p/mid".to_string(),
        ];
        assert_eq!(
            order_in(&reg, &keys),
            vec!["/p/new", "/p/old", "/p/mid", "Chats"]
        );

        let keys = vec!["Chats".to_string(), "/p/mid".to_string()];
        assert_eq!(
            order_in(&reg, &keys),
            vec!["/p/mid", "Chats"],
            "recorded keys that vanished leave the rest intact"
        );
    }
}
