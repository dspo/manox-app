//! Client-side external sessions: the CLI agents and plain terminals
//! launched from the project menu, surfaced as SIDEBAR rows — the legacy
//! shell's external-session semantics on the chrome face.
//!
//! AHP has no external-session channel (the retired v2 wire carried one),
//! so the registry is client state: each launch mints a row that merges
//! into the sidebar snapshot under its project's group (Running while it
//! lives), clicking the row re-focuses its right-pane tab, and closing the
//! tab reaps the row. The registry holds the live terminal ENTITY; the tab
//! wrapping it is minted on demand with the row's id, so a re-focus lands
//! on the same store slot (same id, same view) wherever the pane currently
//! lives. Nothing persists — the launched PTYs live in the shell's pane and
//! die with the window, so [`clear_all`] runs on every window mount (the
//! accepted window-scoped lifetime the pane stashes already follow).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use manox_agent_chrome_ui::session_list::SessionStatus;
use manox_agent_chrome_ui::shell::SessionRow;

/// One launched external session: the sidebar row's fields plus the live
/// terminal entity the tab renders.
pub(crate) struct LaunchedSession {
    /// `ext-NNNN` — the sidebar row id AND the tab id (one identity, so
    /// open/focus/close route by the same string the shell hands around).
    pub id: String,
    /// Row title / tab label (the agent's display name, or the localized
    /// terminal name).
    pub label: String,
    pub svg: &'static str,
    /// The project directory the session was launched in — the sidebar
    /// group it merges into. `None` → the loose Chats bucket.
    pub project: Option<String>,
    pub created_at: i64,
    view: gpui::Entity<terminal_ui::TerminalView>,
}

static LAUNCHED: Mutex<Vec<LaunchedSession>> = Mutex::new(Vec::new());
static SEQ: AtomicU64 = AtomicU64::new(1);

fn lock() -> std::sync::MutexGuard<'static, Vec<LaunchedSession>> {
    LAUNCHED.lock().expect("external-session registry lock")
}

/// Register a launched terminal and put its row on the sidebar. The tab is
/// opened on demand by [`focus`] (and by the launcher's own immediate
/// focus), never stored.
pub(crate) fn launch(
    label: String,
    svg: &'static str,
    project: Option<String>,
    view: gpui::Entity<terminal_ui::TerminalView>,
    cx: &mut gpui::App,
) -> String {
    let id = mint_id();
    lock().push(LaunchedSession {
        id: id.clone(),
        label,
        svg,
        project,
        created_at: chrono::Utc::now().timestamp(),
        view,
    });
    focus(&id, cx);
    notify_workspace(cx);
    id
}

/// Every live session, in launch order.
pub(crate) fn all() -> Vec<LaunchedSessionSnapshot> {
    lock()
        .iter()
        .map(|s| LaunchedSessionSnapshot {
            id: s.id.clone(),
            label: s.label.clone(),
            project: s.project.clone(),
            created_at: s.created_at,
        })
        .collect()
}

/// The projection-side face of a session (the registry holds a gpui entity;
/// the snapshot does not).
pub(crate) struct LaunchedSessionSnapshot {
    pub id: String,
    pub label: String,
    pub project: Option<String>,
    pub created_at: i64,
}

/// Whether a sidebar row id belongs to an external session (the select
/// hook's routing predicate).
pub(crate) fn is_external(id: &str) -> bool {
    id.starts_with("ext-")
}

/// Focus a session's tab in the live pane: mint the tab for the row's id
/// (the pane dedupes by id and re-activates an already-open one), open it,
/// and surface spawn/launch failures as a notification. `false` when the id
/// is unknown or no shell/window is live.
pub(crate) fn focus(id: &str, cx: &mut gpui::App) -> bool {
    let found = {
        let sessions = lock();
        sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| (s.label.clone(), s.svg, s.view.clone()))
    };
    let Some((label, svg, view)) = found else {
        return false;
    };
    let tab = crate::tool_tabs::external_session_tab(id.to_string(), label, svg, view, {
        let id = id.to_string();
        Box::new(move |cx| terminate(&id, cx))
    });
    crate::chrome_assembly::open_tool_tab(tab, cx)
}

/// Reap a session (its tab was closed — dropping the view tears the PTY
/// down) and refresh the sidebar.
pub(crate) fn terminate(id: &str, cx: &mut gpui::App) {
    let before = lock().len();
    lock().retain(|s| s.id != id);
    if before != lock().len() {
        notify_workspace(cx);
    }
}

/// Drop every session — window mount time: the registry rides the shell, so
/// a fresh window starts with no external rows (the processes behind them
/// died with the previous window's pane).
pub(crate) fn clear_all() {
    lock().clear();
}

/// The sidebar row shape: merged into the assembly's snapshot, it lands in
/// its project's group (the shell regroups by display name) as a Running
/// session.
pub(crate) fn row_of(s: &LaunchedSessionSnapshot) -> SessionRow {
    SessionRow {
        id: s.id.clone(),
        title: s.label.clone(),
        workspace: crate::sidebar_projection::project_label(s.project.as_deref().unwrap_or("")),
        project: s.project.clone(),
        status: SessionStatus::Running,
        updated_at: s.created_at,
        sort_stamp: s.created_at,
        pinned: false,
        archived: false,
        tag: None,
        team_leader: false,
    }
}

fn notify_workspace(cx: &mut gpui::App) {
    if let Some(ws) = crate::dispatch::workspace_global() {
        ws.update(cx, |_, cx| cx.notify());
    }
}

pub(crate) fn mint_id() -> String {
    format!("ext-{:04}", SEQ.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(id: &str, project: Option<&str>) -> LaunchedSessionSnapshot {
        LaunchedSessionSnapshot {
            id: id.into(),
            label: "Claude Code".into(),
            project: project.map(str::to_string),
            created_at: 100,
        }
    }

    /// The row routes into its project's group (the shell regroups by
    /// display name) as a Running session with the shared identity.
    #[test]
    fn row_lands_in_the_project_group_as_running() {
        let row = row_of(&snapshot("ext-0001", Some("/home/u/projs/manox")));
        assert_eq!(row.id, "ext-0001");
        assert_eq!(row.workspace, "manox");
        assert_eq!(row.project.as_deref(), Some("/home/u/projs/manox"));
        assert_eq!(row.status, SessionStatus::Running);
        // No project → the loose bucket.
        assert_eq!(row_of(&snapshot("ext-0002", None)).workspace, "Chats");
    }

    /// The routing predicate matches exactly the minted id vocabulary, and
    /// minted ids are unique.
    #[test]
    fn routing_predicate_matches_the_minted_ids() {
        assert!(!is_external("0197uuid-thread-id"));
        let id = mint_id();
        assert!(is_external(&id));
        assert!(mint_id() != id, "ids are unique");
    }
}
