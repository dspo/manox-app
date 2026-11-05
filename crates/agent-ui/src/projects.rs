//! The project registry (design PR-1): explicit multi-root project entities
//! behind the sidebar's project grouping. A project owns at least one
//! filesystem root with exactly one primary, an optional display name, a
//! sidebar position, and a membership table that attaches containers to it —
//! grouping is never inferred from cwds, so the same directory can sit in
//! several projects and a container without a membership falls to the loose
//! "Chats" bucket.
//!
//! Persistence follows the [`crate::project_registry`] shape: the app-owned
//! keys of the shared `~/.manox/settings.toml`, read-modify-written with the
//! only-touch-my-keys discipline — every other key (runtime ones included)
//! survives verbatim, and a corrupt file is replaced rather than allowed to
//! block the write. The keys:
//!
//! - `[[project]]`: one entry per project with nested `[[project.roots]]`
//!   rows (`path` + `primary`).
//! - `[project_membership]`: container id → project id; `external:`-prefixed
//!   keys are legal (TOML quotes them).
//!
//! The write face enforces every invariant (roots exist as directories,
//! dedup + canonicalization, exactly one primary, primary among the roots);
//! a hand-edited file that lost them is repaired on load, never trusted.
//! The retired path-overlay keys of `project_registry` are neither read nor
//! migrated: path grouping has no counterpart in this model.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Context as _;

/// One filesystem root of a project; `primary` names the root new sessions
/// cwd into, and exactly one root per project carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectRoot {
    pub path: PathBuf,
    pub primary: bool,
}

/// A user-created project entity: named or display-named by its primary
/// root, ordered for the sidebar, and joined to containers by membership.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    /// The custom name; empty while the display name follows the primary
    /// root's basename.
    pub name: String,
    /// Once the user names the project, a primary change no longer renames
    /// it.
    pub name_overridden: bool,
    pub roots: Vec<ProjectRoot>,
    /// The sidebar order; a new project pre-inserts at `min(position) - 1`.
    pub position: i64,
    /// Unix seconds.
    pub created_at: i64,
}

/// The whole registry, cached for the process lifetime: the sidebar and the
/// launcher consult it on every render, and every mutation site goes through
/// this module, so the cache cannot drift from the file while the app runs.
#[derive(Default)]
struct Registry {
    projects: Vec<Project>,
    /// container id → project id — the grouping fact source.
    membership: BTreeMap<String, String>,
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

/// The one whole-stack test redirects the settings file here; every other
/// test works on `Registry` values directly.
#[cfg(test)]
static TEST_SETTINGS_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

fn cached() -> std::sync::MutexGuard<'static, Option<Registry>> {
    REGISTRY.lock().expect("project registry lock")
}

fn settings_file() -> Option<PathBuf> {
    #[cfg(test)]
    {
        if let Some(path) = TEST_SETTINGS_FILE
            .lock()
            .expect("test settings lock")
            .clone()
        {
            return Some(path);
        }
    }
    Some(
        PathBuf::from(std::env::var_os("HOME")?)
            .join(".manox")
            .join("settings.toml"),
    )
}

// ── reads ─────────────────────────────────────────────────────────────

/// All projects, sidebar order (position ascending).
pub fn list_projects() -> Vec<Project> {
    let mut slot = cached();
    let mut projects = slot.get_or_insert_with(load).projects.clone();
    projects.sort_by_key(|p| p.position);
    projects
}

/// One project by id.
pub fn get_project(id: &str) -> Option<Project> {
    let mut slot = cached();
    project(slot.get_or_insert_with(load), id).cloned()
}

/// The sidebar header name: the custom name once overridden, otherwise the
/// primary root's basename (so a primary change silently renames an
/// uncustomized project).
pub fn display_name(project: &Project) -> String {
    if project.name_overridden {
        return project.name.clone();
    }
    let primary = project.roots.iter().find(|root| root.primary);
    match primary.and_then(|root| root.path.file_name()) {
        Some(name) => name.to_string_lossy().into_owned(),
        // A path with no file name ("/", a drive root): show it whole.
        None => primary
            .map(|root| root.path.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

// ── project writes ────────────────────────────────────────────────────

/// Create a project and pre-insert it at the front of the sidebar order.
/// Every root must exist as a directory; duplicates collapse to the first
/// occurrence; `primary` defaults to the first root. A `name` that is blank
/// once trimmed means "not customized" — the display name follows the
/// primary root.
pub fn create_project(
    name: Option<String>,
    roots: Vec<PathBuf>,
    primary: Option<PathBuf>,
) -> anyhow::Result<Project> {
    let mut slot = cached();
    let reg = slot.get_or_insert_with(load);
    let project = create_in(reg, name, roots, primary)?;
    persist(reg);
    Ok(project)
}

/// Set (`Some`) or clear (`None`) the custom name; cleared, the display
/// name follows the primary root again.
pub fn update_name(id: &str, name: Option<String>) -> anyhow::Result<()> {
    let mut slot = cached();
    let reg = slot.get_or_insert_with(load);
    update_name_in(reg, id, name)?;
    persist(reg);
    Ok(())
}

/// Replace a project's root set (same validation as creation) and persist.
pub fn set_roots(id: &str, roots: Vec<PathBuf>, primary: Option<PathBuf>) -> anyhow::Result<()> {
    let mut slot = cached();
    let reg = slot.get_or_insert_with(load);
    set_roots_in(reg, id, roots, primary)?;
    persist(reg);
    Ok(())
}

/// Move the primary badge to `path` — the root order is untouched. The path
/// must already be one of the project's roots and exist as a directory.
pub fn set_primary(id: &str, path: &Path) -> anyhow::Result<()> {
    let mut slot = cached();
    let reg = slot.get_or_insert_with(load);
    set_primary_in(reg, id, path)?;
    persist(reg);
    Ok(())
}

/// Delete a project and cascade its memberships. The containers themselves
/// — sessions, journals — are untouched and fall back to the loose bucket.
pub fn delete_project(id: &str) {
    let mut slot = cached();
    let reg = slot.get_or_insert_with(load);
    if delete_in(reg, id) {
        persist(reg);
    }
}

/// Rewrite the sidebar order: the listed ids take positions `0..n` in the
/// given order (ids naming no project are ignored); projects the input does
/// not list trail it in their previous relative order, so the positions
/// stay a permutation of `0..n`.
pub fn reorder(ids_in_new_order: &[String]) {
    let mut slot = cached();
    let reg = slot.get_or_insert_with(load);
    if reorder_in(reg, ids_in_new_order) {
        persist(reg);
    }
}

fn create_in(
    reg: &mut Registry,
    name: Option<String>,
    roots: Vec<PathBuf>,
    primary: Option<PathBuf>,
) -> anyhow::Result<Project> {
    let (name, name_overridden) = customized_name(name);
    let roots = build_roots(roots, primary.as_deref())?;
    let project = Project {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        name_overridden,
        roots,
        position: reg.projects.iter().map(|p| p.position).min().unwrap_or(0) - 1,
        created_at: now_secs(),
    };
    reg.projects.push(project.clone());
    Ok(project)
}

fn update_name_in(reg: &mut Registry, id: &str, name: Option<String>) -> anyhow::Result<()> {
    let index = reg
        .projects
        .iter()
        .position(|p| p.id == id)
        .with_context(|| format!("unknown project {id}"))?;
    let (name, name_overridden) = customized_name(name);
    reg.projects[index].name = name;
    reg.projects[index].name_overridden = name_overridden;
    Ok(())
}

fn set_roots_in(
    reg: &mut Registry,
    id: &str,
    roots: Vec<PathBuf>,
    primary: Option<PathBuf>,
) -> anyhow::Result<()> {
    let index = reg
        .projects
        .iter()
        .position(|p| p.id == id)
        .with_context(|| format!("unknown project {id}"))?;
    let roots = build_roots(roots, primary.as_deref())?;
    reg.projects[index].roots = roots;
    Ok(())
}

fn set_primary_in(reg: &mut Registry, id: &str, path: &Path) -> anyhow::Result<()> {
    let index = reg
        .projects
        .iter()
        .position(|p| p.id == id)
        .with_context(|| format!("unknown project {id}"))?;
    anyhow::ensure!(
        path.is_dir(),
        "primary root is not an existing directory: {}",
        path.display()
    );
    let path = path
        .canonicalize()
        .with_context(|| format!("canonicalize primary root {}", path.display()))?;
    let roots = &mut reg.projects[index].roots;
    let target = roots
        .iter()
        .position(|root| root.path == path)
        .with_context(|| format!("primary root is not among the roots: {}", path.display()))?;
    for (i, root) in roots.iter_mut().enumerate() {
        root.primary = i == target;
    }
    Ok(())
}

fn delete_in(reg: &mut Registry, id: &str) -> bool {
    let before = reg.projects.len();
    reg.projects.retain(|p| p.id != id);
    if reg.projects.len() == before {
        return false;
    }
    reg.membership.retain(|_, project| project.as_str() != id);
    true
}

fn reorder_in(reg: &mut Registry, ids: &[String]) -> bool {
    let rank = |id: &str| ids.iter().position(|listed| listed == id);
    // Listed projects first in the given order, then the unlisted in their
    // previous order.
    let mut order: Vec<usize> = (0..reg.projects.len()).collect();
    order.sort_by_key(|&index| match rank(&reg.projects[index].id) {
        Some(rank) => (0, rank as i64),
        None => (1, reg.projects[index].position),
    });
    let mut changed = false;
    for (position, &index) in order.iter().enumerate() {
        let project = &mut reg.projects[index];
        let position = position as i64;
        if project.position != position {
            project.position = position;
            changed = true;
        }
    }
    changed
}

/// (`name`, overridden) from the dialog's optional custom name: blank means
/// "follow the primary root", a trimmed non-empty string is the override.
fn customized_name(name: Option<String>) -> (String, bool) {
    match name.map(|n| n.trim().to_string()) {
        Some(n) if !n.is_empty() => (n, true),
        _ => (String::new(), false),
    }
}

/// Validate + normalize a root set: every path must exist as a directory,
/// every survivor is its canonical absolute form, duplicates collapse to
/// the first occurrence, and nested parent/child roots are both kept.
/// `primary` must name one of the surviving roots (default: the first).
fn build_roots(roots: Vec<PathBuf>, primary: Option<&Path>) -> anyhow::Result<Vec<ProjectRoot>> {
    anyhow::ensure!(!roots.is_empty(), "a project needs at least one root");
    let mut canonical: Vec<PathBuf> = Vec::new();
    for root in roots {
        anyhow::ensure!(
            root.is_dir(),
            "project root is not an existing directory: {}",
            root.display()
        );
        let path = root
            .canonicalize()
            .with_context(|| format!("canonicalize project root {}", root.display()))?;
        if !canonical.contains(&path) {
            canonical.push(path);
        }
    }
    let primary_index = match primary {
        None => 0,
        Some(path) => {
            anyhow::ensure!(
                path.is_dir(),
                "primary root is not an existing directory: {}",
                path.display()
            );
            let path = path
                .canonicalize()
                .with_context(|| format!("canonicalize primary root {}", path.display()))?;
            canonical.iter().position(|p| p == &path).with_context(|| {
                format!("primary root is not among the roots: {}", path.display())
            })?
        }
    };
    Ok(canonical
        .into_iter()
        .enumerate()
        .map(|(i, path)| ProjectRoot {
            path,
            primary: i == primary_index,
        })
        .collect())
}

fn project<'a>(reg: &'a Registry, id: &str) -> Option<&'a Project> {
    reg.projects.iter().find(|p| p.id == id)
}

// ── membership ────────────────────────────────────────────────────────

/// Attach a container to a project — the grouping fact source. No-op when
/// the container id is empty or the project does not exist, so the file
/// never carries a dangling entry.
pub fn set_membership(container_id: &str, project_id: &str) {
    if container_id.is_empty() {
        return;
    }
    let mut slot = cached();
    let reg = slot.get_or_insert_with(load);
    if set_membership_in(reg, container_id, project_id) {
        persist(reg);
    }
}

/// The project a container belongs to, if any.
pub fn membership_of(container_id: &str) -> Option<String> {
    let mut slot = cached();
    slot.get_or_insert_with(load)
        .membership
        .get(container_id)
        .cloned()
}

/// The containers attached to a project, container id order.
pub fn containers_of_project(project_id: &str) -> Vec<String> {
    let mut slot = cached();
    containers_in(slot.get_or_insert_with(load), project_id)
}

/// Detach a container. Idempotent.
pub fn remove_membership(container_id: &str) {
    let mut slot = cached();
    let reg = slot.get_or_insert_with(load);
    if remove_membership_in(reg, container_id) {
        persist(reg);
    }
}

/// Batch detach — the container-teardown prune. Persists at most once.
pub fn prune_memberships(container_ids: &[String]) {
    let mut slot = cached();
    let reg = slot.get_or_insert_with(load);
    if prune_in(reg, container_ids) {
        persist(reg);
    }
}

fn set_membership_in(reg: &mut Registry, container_id: &str, project_id: &str) -> bool {
    if project(reg, project_id).is_none() {
        return false;
    }
    let changed = reg.membership.get(container_id).map(String::as_str) != Some(project_id);
    if changed {
        reg.membership
            .insert(container_id.to_string(), project_id.to_string());
    }
    changed
}

fn remove_membership_in(reg: &mut Registry, container_id: &str) -> bool {
    reg.membership.remove(container_id).is_some()
}

fn prune_in(reg: &mut Registry, container_ids: &[String]) -> bool {
    let ids: HashSet<&str> = container_ids.iter().map(String::as_str).collect();
    let before = reg.membership.len();
    reg.membership
        .retain(|container, _| !ids.contains(container.as_str()));
    reg.membership.len() != before
}

fn containers_in(reg: &Registry, project_id: &str) -> Vec<String> {
    reg.membership
        .iter()
        .filter(|(_, project)| project.as_str() == project_id)
        .map(|(container, _)| container.clone())
        .collect()
}

// ── persistence ───────────────────────────────────────────────────────

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
fn load_at(path: &Path) -> Registry {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Registry::default();
    };
    let Ok(doc) = toml::from_str::<toml::Value>(&raw) else {
        tracing::warn!("settings.toml parse failed; project registry starts empty");
        return Registry::default();
    };
    Registry {
        projects: doc
            .get("project")
            .and_then(|v| v.as_array())
            .map(|entries| entries.iter().filter_map(parse_project).collect())
            .unwrap_or_default(),
        membership: doc
            .get("project_membership")
            .and_then(|v| v.as_table())
            .map(|table| {
                table
                    .iter()
                    .filter_map(|(k, v)| v.as_str().map(|project| (k.clone(), project.to_string())))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// One `[[project]]` entry; malformed entries are skipped and a surviving
/// entry keeps the write face's invariants (at least one root, exactly one
/// primary).
fn parse_project(entry: &toml::Value) -> Option<Project> {
    let table = entry.as_table()?;
    Some(Project {
        id: table.get("id")?.as_str()?.to_string(),
        name: table
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        name_overridden: table
            .get("name_overridden")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        roots: parse_roots(table.get("roots"))?,
        position: table
            .get("position")
            .and_then(|v| v.as_integer())
            .unwrap_or(0),
        created_at: table
            .get("created_at")
            .and_then(|v| v.as_integer())
            .unwrap_or(0),
    })
}

fn parse_roots(value: Option<&toml::Value>) -> Option<Vec<ProjectRoot>> {
    let entries = value?.as_array()?;
    let mut roots: Vec<ProjectRoot> = entries
        .iter()
        .filter_map(|entry| {
            let table = entry.as_table()?;
            Some(ProjectRoot {
                path: PathBuf::from(table.get("path")?.as_str()?),
                primary: table
                    .get("primary")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            })
        })
        .collect();
    if roots.is_empty() {
        // A rootless project is unrepresentable; drop the entry.
        return None;
    }
    // The write face keeps exactly one primary; a hand-edited file may not —
    // repair (first root wins) instead of trusting it.
    let mut primary_seen = false;
    for root in &mut roots {
        let take = root.primary && !primary_seen;
        root.primary = take;
        primary_seen |= take;
    }
    if !primary_seen {
        roots[0].primary = true;
    }
    Some(roots)
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
/// recovery the ui_language accessor and `project_registry` use).
fn persist_at(path: &Path, reg: &Registry) {
    let mut doc: toml::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| toml::from_str(&raw).ok())
        .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
    let Some(table) = doc.as_table_mut() else {
        return;
    };
    table.insert(
        "project".to_string(),
        toml::Value::Array(reg.projects.iter().map(project_row).collect()),
    );
    let membership: toml::map::Map<String, toml::Value> = reg
        .membership
        .iter()
        .map(|(container, project)| (container.clone(), toml::Value::String(project.clone())))
        .collect();
    table.insert(
        "project_membership".to_string(),
        toml::Value::Table(membership),
    );
    match toml::to_string_pretty(&doc)
        .context("serialize settings.toml")
        .and_then(|s| std::fs::write(path, s).context("write settings.toml"))
    {
        Ok(()) => {}
        Err(e) => tracing::warn!(error = %e, "persist project registry failed"),
    }
}

fn project_row(project: &Project) -> toml::Value {
    let mut table = toml::map::Map::new();
    table.insert("id".to_string(), toml::Value::String(project.id.clone()));
    table.insert(
        "name".to_string(),
        toml::Value::String(project.name.clone()),
    );
    table.insert(
        "name_overridden".to_string(),
        toml::Value::Boolean(project.name_overridden),
    );
    table.insert(
        "position".to_string(),
        toml::Value::Integer(project.position),
    );
    table.insert(
        "created_at".to_string(),
        toml::Value::Integer(project.created_at),
    );
    table.insert(
        "roots".to_string(),
        toml::Value::Array(
            project
                .roots
                .iter()
                .map(|root| {
                    let mut row = toml::map::Map::new();
                    row.insert(
                        "path".to_string(),
                        toml::Value::String(root.path.to_string_lossy().into_owned()),
                    );
                    row.insert("primary".to_string(), toml::Value::Boolean(root.primary));
                    toml::Value::Table(row)
                })
                .collect(),
        ),
    );
    toml::Value::Table(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join(label);
        std::fs::create_dir(&root).expect("mkdir");
        (dir, root)
    }

    fn canon(path: &Path) -> PathBuf {
        path.canonicalize().expect("canonicalize")
    }

    fn project_at<'a>(reg: &'a Registry, id: &str) -> &'a Project {
        project(reg, id).expect("project exists")
    }

    fn primary_flags(reg: &Registry, id: &str) -> Vec<bool> {
        project_at(reg, id)
            .roots
            .iter()
            .map(|r| r.primary)
            .collect()
    }

    fn ids_by_position(reg: &Registry) -> Vec<String> {
        let mut projects = reg.projects.clone();
        projects.sort_by_key(|p| p.position);
        projects.into_iter().map(|p| p.id).collect()
    }

    /// The parse-edit-serialize cycle: persisted projects, their nested
    /// root rows and memberships survive a round trip, an `external:` key
    /// lands quoted, and the unrelated sibling keys are preserved verbatim.
    #[test]
    fn persist_preserves_sibling_keys_and_round_trips() {
        let (dir, root_a) = temp_root("a");
        let root_b = root_a.parent().expect("parent").join("b");
        std::fs::create_dir(&root_b).expect("mkdir");
        let settings = dir.path().join("settings.toml");
        std::fs::write(&settings, "ui_language = \"zh-CN\"\n\n[host]\nport = 42\n").expect("seed");

        let mut reg = Registry::default();
        let first = create_in(
            &mut reg,
            Some("Steer".into()),
            vec![root_a.clone(), root_b.clone()],
            Some(root_b.clone()),
        )
        .expect("create");
        let _second = create_in(&mut reg, None, vec![root_a.clone()], None).expect("create");
        assert!(set_membership_in(&mut reg, "session-1", &first.id));
        assert!(set_membership_in(
            &mut reg,
            "external:claude:abc",
            &first.id
        ));
        persist_at(&settings, &reg);

        let loaded = load_at(&settings);
        assert_eq!(loaded.projects, reg.projects);
        assert_eq!(loaded.membership, reg.membership);

        let raw = std::fs::read_to_string(&settings).expect("read");
        assert!(raw.contains("[[project]]"));
        assert!(raw.contains("[[project.roots]]"));
        assert!(raw.contains("\"external:claude:abc\""), "quoted key");
        let doc: toml::Value = toml::from_str(&raw).expect("parse");
        assert_eq!(
            doc.get("ui_language").and_then(|v| v.as_str()),
            Some("zh-CN")
        );
        assert_eq!(
            doc.get("host")
                .and_then(|v| v.get("port"))
                .and_then(|v| v.as_integer()),
            Some(42)
        );
    }

    /// A hand-edited file is repaired on load: the first root wins the
    /// primary badge and a rootless entry is dropped as unrepresentable.
    #[test]
    fn load_repairs_a_hand_edited_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = dir.path().join("settings.toml");
        std::fs::write(
            &settings,
            "ui_language = \"en-US\"\n\
             [[project]]\n\
             id = \"p1\"\n\
             name = \"X\"\n\
             name_overridden = true\n\
             position = 5\n\
             created_at = 7\n\
             [[project.roots]]\n\
             path = \"/somewhere/manox\"\n\
             primary = true\n\
             [[project.roots]]\n\
             path = \"/somewhere/manox-app\"\n\
             primary = true\n\
             [[project]]\n\
             id = \"rootless\"\n\
             name = \"\"\n\
             position = 0\n\
             created_at = 0\n",
        )
        .expect("seed");
        let loaded = load_at(&settings);
        assert_eq!(loaded.projects.len(), 1, "the rootless entry is dropped");
        let p = &loaded.projects[0];
        assert_eq!(p.id, "p1");
        let flags: Vec<bool> = p.roots.iter().map(|r| r.primary).collect();
        assert_eq!(flags, vec![true, false], "first root wins the badge");
    }

    /// Creation refuses missing directories, plain files, and empty root
    /// sets, leaving nothing behind.
    #[test]
    fn create_rejects_missing_directories_and_empty_roots() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("real");
        std::fs::create_dir(&real).expect("mkdir");
        let file = dir.path().join("file");
        std::fs::write(&file, "x").expect("write");
        let mut reg = Registry::default();

        let missing = dir.path().join("absent");
        assert!(create_in(&mut reg, None, vec![real.clone(), missing.clone()], None).is_err());
        assert!(reg.projects.is_empty(), "a failed create leaves nothing");
        assert!(create_in(&mut reg, None, vec![file.clone()], None).is_err());
        assert!(create_in(&mut reg, None, vec![], None).is_err());
        assert!(reg.projects.is_empty());

        let project = create_in(&mut reg, None, vec![real], None).expect("create");
        assert_eq!(project.roots.len(), 1);
    }

    /// Duplicates collapse to the first occurrence, spellings land on the
    /// canonical absolute form, nested parent/child roots are both kept,
    /// and the primary badge lands on the requested root.
    #[test]
    fn create_dedups_canonicalizes_and_keeps_nested_roots() {
        let (_dir, root_a) = temp_root("a");
        let nested = root_a.join("nested");
        std::fs::create_dir(&nested).expect("mkdir");
        let mut reg = Registry::default();

        let project = create_in(
            &mut reg,
            None,
            vec![root_a.clone(), root_a.clone(), nested.clone()],
            None,
        )
        .expect("create");
        let paths: Vec<PathBuf> = project.roots.iter().map(|r| r.path.clone()).collect();
        assert_eq!(paths, vec![canon(&root_a), canon(&nested)]);
        assert_eq!(primary_flags(&reg, &project.id), vec![true, false]);

        // An explicit primary is honored; a blank name is "not customized".
        let project = create_in(
            &mut reg,
            Some("  P  ".into()),
            vec![root_a.clone(), nested.clone()],
            Some(nested.clone()),
        )
        .expect("create");
        assert_eq!(project.name, "P");
        assert!(project.name_overridden);
        assert_eq!(primary_flags(&reg, &project.id), vec![false, true]);

        // A primary outside the roots is refused.
        let elsewhere = temp_root("elsewhere").1;
        assert!(create_in(&mut reg, None, vec![root_a.clone()], Some(elsewhere)).is_err());
    }

    /// The badge moves and never duplicates: after set_primary and
    /// set_roots exactly one root is primary, and refusals leave the
    /// project untouched.
    #[test]
    fn set_primary_and_set_roots_keep_exactly_one_primary() {
        let (dir, a) = temp_root("a");
        let b = dir.path().join("b");
        let c = dir.path().join("c");
        std::fs::create_dir(&b).expect("mkdir");
        std::fs::create_dir(&c).expect("mkdir");
        let mut reg = Registry::default();
        let p =
            create_in(&mut reg, None, vec![a.clone(), b.clone(), c.clone()], None).expect("create");
        assert_eq!(primary_flags(&reg, &p.id), vec![true, false, false]);

        set_primary_in(&mut reg, &p.id, &b).expect("set_primary");
        assert_eq!(primary_flags(&reg, &p.id), vec![false, true, false]);

        let missing = dir.path().join("absent");
        assert!(set_primary_in(&mut reg, &p.id, &missing).is_err());
        assert!(
            set_primary_in(&mut reg, &p.id, &a).is_ok(),
            "back to a root"
        );
        set_primary_in(&mut reg, &p.id, &b).expect("set_primary");
        assert!(set_primary_in(&mut reg, "no-such-id", &a).is_err());
        assert_eq!(primary_flags(&reg, &p.id), vec![false, true, false]);

        set_roots_in(&mut reg, &p.id, vec![c.clone(), b.clone()], None).expect("set_roots");
        assert_eq!(
            primary_flags(&reg, &p.id),
            vec![true, false],
            "primary resets to the first new root"
        );
        set_roots_in(&mut reg, &p.id, vec![a.clone(), b.clone()], Some(b.clone()))
            .expect("set_roots");
        assert_eq!(primary_flags(&reg, &p.id), vec![false, true]);

        assert!(set_roots_in(&mut reg, &p.id, vec![], None).is_err());
        assert!(set_roots_in(&mut reg, &p.id, vec![missing], None).is_err());
        assert_eq!(project_at(&reg, &p.id).roots.len(), 2, "unchanged");
        assert_eq!(primary_flags(&reg, &p.id), vec![false, true]);
    }

    /// The display name follows the primary root's basename until the user
    /// overrides it; clearing the override resumes following.
    #[test]
    fn display_name_follows_primary_until_overridden() {
        let (dir, a) = temp_root("widget");
        let b = dir.path().join("anvil");
        std::fs::create_dir(&b).expect("mkdir");
        let mut reg = Registry::default();
        let p = create_in(&mut reg, None, vec![a.clone(), b.clone()], None).expect("create");
        let name = |reg: &Registry| display_name(project_at(reg, &p.id));
        assert_eq!(name(&reg), "widget");

        set_primary_in(&mut reg, &p.id, &b).expect("set_primary");
        assert_eq!(name(&reg), "anvil", "uncustomized name follows the root");

        update_name_in(&mut reg, &p.id, Some("Steer".into())).expect("update_name");
        assert_eq!(name(&reg), "Steer");
        set_primary_in(&mut reg, &p.id, &a).expect("set_primary");
        assert_eq!(name(&reg), "Steer", "overridden name does not follow");

        update_name_in(&mut reg, &p.id, Some("   ".into())).expect("update_name");
        assert!(!project_at(&reg, &p.id).name_overridden, "blank clears");
        assert_eq!(name(&reg), "widget");
        update_name_in(&mut reg, &p.id, None).expect("update_name");
        assert_eq!(name(&reg), "widget");
        assert!(update_name_in(&mut reg, "ghost", Some("x".into())).is_err());
    }

    /// Deleting a project removes its memberships with it; other
    /// projects' memberships survive, and a second delete is a no-op.
    #[test]
    fn delete_project_cascades_memberships() {
        let (_dir_a, a) = temp_root("a");
        let (_dir_b, b) = temp_root("b");
        let mut reg = Registry::default();
        let p1 = create_in(&mut reg, None, vec![a], None).expect("create");
        let p2 = create_in(&mut reg, None, vec![b], None).expect("create");
        assert!(set_membership_in(&mut reg, "s1", &p1.id));
        assert!(set_membership_in(&mut reg, "s2", &p1.id));
        assert!(set_membership_in(&mut reg, "s3", &p2.id));

        assert!(delete_in(&mut reg, &p1.id));
        assert!(reg.projects.iter().all(|p| p.id != p1.id));
        assert!(!reg.membership.contains_key("s1"));
        assert!(!reg.membership.contains_key("s2"));
        assert_eq!(
            reg.membership.get("s3").map(String::as_str),
            Some(p2.id.as_str())
        );
        assert!(!delete_in(&mut reg, &p1.id), "second delete is a no-op");
    }

    /// The listed ids take positions 0..n in order, unknown ids are
    /// ignored, and unlisted projects trail in their previous relative
    /// order — so positions stay a permutation of 0..n.
    #[test]
    fn reorder_rewrites_positions_zero_to_n() {
        let (dir, a) = temp_root("a");
        let b = dir.path().join("b");
        let c = dir.path().join("c");
        let d = dir.path().join("d");
        for root in [&b, &c, &d] {
            std::fs::create_dir(root).expect("mkdir");
        }
        let mut reg = Registry::default();
        let p1 = create_in(&mut reg, None, vec![a.clone()], None).expect("create");
        let p2 = create_in(&mut reg, None, vec![b.clone()], None).expect("create");
        let p3 = create_in(&mut reg, None, vec![c.clone()], None).expect("create");
        let p4 = create_in(&mut reg, None, vec![d.clone()], None).expect("create");
        // Pre-insertion: newest first before any manual order.
        assert_eq!(
            ids_by_position(&reg),
            vec![p4.id.clone(), p3.id.clone(), p2.id.clone(), p1.id.clone()]
        );

        reorder_in(&mut reg, &[p1.id.clone(), p2.id.clone()]);
        assert_eq!(
            positions_of(&reg),
            vec![
                (0, p1.id.clone()),
                (1, p2.id.clone()),
                (2, p4.id.clone()),
                (3, p3.id.clone())
            ],
            "listed ids take 0..n, unlisted trail by old position"
        );

        // Unknown ids are ignored; the unlisted keep their relative order.
        reorder_in(
            &mut reg,
            &["ghost".to_string(), p3.id.clone(), p1.id.clone()],
        );
        assert_eq!(
            positions_of(&reg),
            vec![
                (0, p3.id.clone()),
                (1, p1.id.clone()),
                (2, p2.id.clone()),
                (3, p4.id.clone())
            ]
        );
    }

    fn positions_of(reg: &Registry) -> Vec<(i64, String)> {
        let mut projects = reg.projects.clone();
        projects.sort_by_key(|p| p.position);
        projects.into_iter().map(|p| (p.position, p.id)).collect()
    }

    /// Membership CRUD: re-pointing, sorted reads, idempotent removal, and
    /// the batch prune.
    #[test]
    fn membership_crud() {
        let (_dir_a, a) = temp_root("a");
        let (_dir_b, b) = temp_root("b");
        let mut reg = Registry::default();
        let p1 = create_in(&mut reg, None, vec![a], None).expect("create");
        let p2 = create_in(&mut reg, None, vec![b], None).expect("create");

        assert!(
            !set_membership_in(&mut reg, "s1", "ghost"),
            "no membership to a nonexistent project"
        );
        assert!(set_membership_in(&mut reg, "s1", &p1.id));
        assert!(!set_membership_in(&mut reg, "s1", &p1.id), "same value");
        assert!(set_membership_in(&mut reg, "s1", &p2.id), "re-pointing");
        assert!(set_membership_in(&mut reg, "external:codex:u", &p1.id));
        assert!(set_membership_in(&mut reg, "aaa-first", &p1.id));
        assert_eq!(
            containers_in(&reg, &p1.id),
            vec!["aaa-first".to_string(), "external:codex:u".to_string()],
            "container id order"
        );

        assert!(remove_membership_in(&mut reg, "external:codex:u"));
        assert!(
            !remove_membership_in(&mut reg, "external:codex:u"),
            "idempotent"
        );
        assert!(prune_in(
            &mut reg,
            &[
                "s1".to_string(),
                "aaa-first".to_string(),
                "absent".to_string()
            ]
        ));
        assert!(!prune_in(&mut reg, &["absent".to_string()]), "nothing left");
        assert!(reg.membership.is_empty());
    }

    /// The whole stack through the public API against a redirected
    /// settings file: a create lands on disk, survives a cache drop (the
    /// restart shape), and leaves sibling keys intact. The only test
    /// touching the process-wide statics; it restores them before returning.
    #[test]
    fn pub_api_persists_and_reloads_across_a_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = dir.path().join("settings.toml");
        std::fs::write(&settings, "ui_language = \"zh-CN\"\n\n[host]\nport = 42\n").expect("seed");
        let root = dir.path().join("root");
        std::fs::create_dir(&root).expect("mkdir");

        *TEST_SETTINGS_FILE.lock().expect("lock") = Some(settings.clone());
        *REGISTRY.lock().expect("lock") = None;

        let project = create_project(Some("Steer".into()), vec![root], None).expect("create");
        set_membership("session-1", &project.id);
        assert_eq!(
            membership_of("session-1").as_deref(),
            Some(project.id.as_str())
        );

        // The cache drops (the restart shape); the file alone answers.
        *REGISTRY.lock().expect("lock") = None;
        let loaded = list_projects();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, project.id);
        assert_eq!(loaded[0].roots, project.roots);
        assert_eq!(
            membership_of("session-1").as_deref(),
            Some(project.id.as_str())
        );
        let doc: toml::Value =
            toml::from_str(&std::fs::read_to_string(&settings).expect("read")).expect("parse");
        assert_eq!(
            doc.get("ui_language").and_then(|v| v.as_str()),
            Some("zh-CN")
        );

        delete_project(&project.id);
        assert!(membership_of("session-1").is_none(), "cascade");

        *TEST_SETTINGS_FILE.lock().expect("lock") = None;
        *REGISTRY.lock().expect("lock") = None;
    }
}
