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
