//! The main-column external sessions — the legacy shell's external-session
//! family on the chrome face. A session is a live PTY terminal (a CLI
//! agent's TUI or a plain shell) the project menu launched. It renders IN
//! PLACE OF the conversation column (`ViewMode::ExternalSession` — the
//! shell wraps the main column, and the main column is where terminal/TUI
//! sessions live); switching away parks the session (its terminal keeps
//! running), closing drops the view and tears the process tree down.
//!
//! Identity (the legacy model): each session mints a manox-app-style row id
//! `external:{agent}:{uuid}` — namespaced so it never collides with a manox
//! thread UUID in the sidebar's selection namespace — and records the REAL
//! external session id behind it (`cx_session_id`, derived from the cx
//! session socket's `<id>.sock` filename; empty for plain terminals and
//! when the IPC bind failed). The row id is what the sidebar shows and
//! routes on; the cx id is the traceable link to `~/.manox/sessions/`.
//!
//! In-memory only — the PTY dies with the process, nothing persists. The
//! sidebar rows are projected by [`Workspace::external_session_rows`] and
//! merged into the assembly's snapshot (the chrome regroups them under the
//! project's header).

use std::path::PathBuf;

use steer_agent_chrome_ui::session_list::{SessionRowKind, SessionStatus};
use steer_agent_chrome_ui::shell::SessionRow;

use super::Workspace;

/// One live external session: the identity pair, the sidebar row's fields,
/// and the terminal entity that occupies the main column while the session
/// is foreground.
pub(crate) struct ExternalSessionRecord {
    /// `external:{agent_id}:{uuid}` — the manox-app-style row id. It is the
    /// sidebar selection key AND the routing key; the `external:` namespace
    /// keeps it clear of manox thread UUIDs.
    pub id: String,
    /// The REAL external session id (the cx session naming
    /// `~/.manox/sessions/<id>.sock`), derived from the session handle's
    /// socket path at spawn. Empty for plain terminals and when the IPC
    /// bind failed — the association the row id resolves to.
    pub cx_session_id: String,
    /// Row title / main-column heading (the agent's display name, or the
    /// localized terminal name).
    pub label: String,
    pub svg: &'static str,
    /// The project directory the session was launched in — the sidebar
    /// group it merges into; `None` → the loose Chats bucket.
    pub project: Option<PathBuf>,
    pub created_at: i64,
    pub view: gpui::Entity<terminal_ui::TerminalView>,
}

fn mint_row_id(agent_id: &str) -> String {
    format!("external:{}:{}", agent_id, uuid::Uuid::new_v4())
}

/// One launch's session payload (bundled — the spawn would otherwise arc
/// past clippy's argument ceiling).
pub struct ExternalSessionLaunch<'a> {
    /// Names the row-id namespace (`external:{agent}:{uuid}`).
    pub agent_id: &'a str,
    /// The REAL external session id (see [`ExternalSessionRecord`]).
    pub cx_session_id: String,
    pub label: String,
    pub svg: &'static str,
    pub project: Option<PathBuf>,
}

impl Workspace {
    /// Register a spawned terminal and bring it up in the main column —
    /// focused, so the TUI takes keyboard input immediately.
    pub fn spawn_external_session(
        &mut self,
        launch: ExternalSessionLaunch<'_>,
        view: gpui::Entity<terminal_ui::TerminalView>,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let id = mint_row_id(launch.agent_id);
        self.externals.push(ExternalSessionRecord {
            id: id.clone(),
            cx_session_id: launch.cx_session_id,
            label: launch.label,
            svg: launch.svg,
            project: launch.project,
            created_at: chrono::Utc::now().timestamp(),
            view: view.clone(),
        });
        // The identity link, read from the record: a row id traces back to
        // the cx session's socket on disk.
        let record = self.externals.last().expect("just pushed");
        tracing::info!(
            row_id = %record.id,
            cx_session_id = %record.cx_session_id,
            agent = launch.agent_id,
            "external session registered"
        );
        self.active_external = Some(id);
        self.view_mode = super::ViewMode::ExternalSession;
        focus_terminal(&view, window, cx);
        cx.notify();
    }

    /// Bring a parked session back into the main column (the sidebar row's
    /// click) — re-focused. A no-op for an unknown id — a stale row cannot
    /// crash.
    pub fn open_external_session(
        &mut self,
        id: &str,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(record) = self.externals.iter().find(|s| s.id == id) else {
            return;
        };
        let view = record.view.clone();
        self.active_external = Some(id.to_string());
        self.view_mode = super::ViewMode::ExternalSession;
        focus_terminal(&view, window, cx);
        cx.notify();
    }

    /// Whether `id` names a live external session (the select hook's routing
    /// predicate — an external row opens its session, anything else takes
    /// the thread switch path).
    pub fn is_external_session(&self, id: &str) -> bool {
        self.externals.iter().any(|s| s.id == id)
    }

    /// Close a session (the sidebar row's 关闭会话): drop the record — the
    /// dropped `TerminalView` tears the process tree down — and fall back
    /// to the conversation when it was the one on screen.
    pub fn close_external_session(&mut self, id: &str, cx: &mut gpui::Context<Self>) {
        let was_active = self.active_external.as_deref() == Some(id);
        self.externals.retain(|s| s.id != id);
        if was_active {
            self.active_external = None;
            self.view_mode = super::ViewMode::Workspace;
        }
        cx.notify();
    }

    /// Leave the external-session mode, if on (any thread navigation is an
    /// implicit park — the session keeps running).
    pub fn leave_external_session(&mut self) {
        if matches!(self.view_mode, super::ViewMode::ExternalSession) {
            self.active_external = None;
            self.view_mode = super::ViewMode::Workspace;
        }
    }

    /// Whether an external session currently owns the main column (the
    /// hosts'/tests' read face; the flip is the row click's contract).
    pub fn external_session_foreground(&self) -> bool {
        matches!(self.view_mode, super::ViewMode::ExternalSession)
    }

    /// The foreground session's label — the main surface's title face.
    pub(crate) fn active_external_label(&self) -> Option<String> {
        let id = self.active_external.as_deref()?;
        Some(self.externals.iter().find(|s| s.id == id)?.label.clone())
    }

    /// The sidebar rows: one per live session, stamped with the external
    /// kind (brand-mark leading slot, close-session menu on the chrome
    /// side) and the project grouping key.
    pub(crate) fn external_session_rows(&self) -> Vec<SessionRow> {
        self.externals
            .iter()
            .map(|s| {
                let project = s.project.as_ref().map(|p| p.to_string_lossy().to_string());
                let workspace =
                    crate::sidebar_projection::project_label(project.as_deref().unwrap_or(""));
                SessionRow {
                    id: s.id.clone(),
                    // The chip shows the uuid SEGMENT (the row id's
                    // `external:{agent}:` namespace stays out of the chip) —
                    // the same 8-char shape a thread's uuid prefix shows.
                    short_id: s
                        .id
                        .rsplit(':')
                        .next()
                        .unwrap_or(&s.id)
                        .chars()
                        .take(8)
                        .collect(),
                    title: s.label.clone(),
                    workspace,
                    project,
                    status: SessionStatus::Running,
                    kind: SessionRowKind::External {
                        icon: steer_agent_chrome_ui::theme::icons::IconAsset(s.svg),
                    },
                    updated_at: s.created_at,
                    sort_stamp: s.created_at,
                    pinned: false,
                    archived: false,
                    tag: None,
                    team_leader: false,
                }
            })
            .collect()
    }
}

/// Focus a session's terminal (the event-path face — open/spawn call this
/// so the TUI takes keyboard input immediately; `render` may not focus).
fn focus_terminal(
    view: &gpui::Entity<terminal_ui::TerminalView>,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) {
    let handle = view.read(cx).focus_handle();
    window.focus(&handle, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The row-id vocabulary: namespaced `external:{agent}:{uuid}`, unique,
    /// and never colliding with a bare thread UUID.
    #[test]
    fn row_ids_are_namespaced_and_unique() {
        let id = mint_row_id("claude");
        assert!(id.starts_with("external:claude:"));
        assert!(mint_row_id("claude") != id, "ids are unique");
        assert!(!id.starts_with("0197"), "never a bare thread-uuid shape");
    }
}
