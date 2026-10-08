//! The main-column external sessions — the legacy shell's external-session
//! family on the chrome face. A session is a live PTY terminal (a CLI
//! agent's TUI or a plain shell) the project menu launched. It renders IN
//! PLACE OF the conversation column (`ViewMode::ExternalSession` — the
//! shell wraps the main column, and the main column is where terminal/TUI
//! sessions live); switching away parks the session (its terminal keeps
//! running), closing drops the view and tears the process tree down.
//!
//! In-memory only — the PTY dies with the process, nothing persists. The
//! sidebar rows are projected by [`external_session_rows`] and merged into
//! the assembly's snapshot (the chrome regroups them under the project's
//! header).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use manox_agent_chrome_ui::session_list::{SessionRowKind, SessionStatus};
use manox_agent_chrome_ui::shell::SessionRow;

use super::Workspace;

/// The row-id vocabulary (`ext-NNNN`) — the select hook's routing predicate
/// in the chrome assembly matches this exact prefix.
pub(crate) fn is_external_row(id: &str) -> bool {
    id.starts_with("ext-")
}

fn mint_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    format!("ext-{:04}", SEQ.fetch_add(1, Ordering::Relaxed))
}

/// One live external session: the sidebar row's fields plus the terminal
/// entity that occupies the main column while the session is foreground.
pub(crate) struct ExternalSessionRecord {
    pub id: String,
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

impl Workspace {
    /// Register a spawned terminal and bring it up in the main column.
    /// `view` comes from `tool_tabs::spawn_agent_terminal` /
    /// `spawn_standalone_terminal` — the record owns it, so dropping the
    /// record tears the process tree down.
    pub fn spawn_external_session(
        &mut self,
        label: String,
        svg: &'static str,
        project: Option<PathBuf>,
        view: gpui::Entity<terminal_ui::TerminalView>,
        cx: &mut gpui::Context<Self>,
    ) {
        let id = mint_id();
        self.externals.push(ExternalSessionRecord {
            id: id.clone(),
            label,
            svg,
            project,
            created_at: chrono::Utc::now().timestamp(),
            view,
        });
        self.active_external = Some(id);
        self.view_mode = super::ViewMode::ExternalSession;
        cx.notify();
    }

    /// Bring a parked session back into the main column (the sidebar row's
    /// click). A no-op for an unknown id — a stale row cannot crash.
    pub fn open_external_session(&mut self, id: &str, cx: &mut gpui::Context<Self>) {
        if !self.externals.iter().any(|s| s.id == id) {
            return;
        }
        self.active_external = Some(id.to_string());
        self.view_mode = super::ViewMode::ExternalSession;
        cx.notify();
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

    /// Whether `id` names a still-live external session row. The assembly
    /// layer's active guard needs this beyond the `ext-` prefix check: a
    /// closed foreground external leaves its dead id in the chrome's
    /// `active`, and a prefix-only guard would leave the sidebar
    /// highlight-less until the next click.
    pub(crate) fn external_is_live(&self, id: &str) -> bool {
        self.externals.iter().any(|s| s.id == id)
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
                    title: s.label.clone(),
                    workspace,
                    project,
                    status: SessionStatus::Running,
                    kind: SessionRowKind::External {
                        icon: manox_agent_chrome_ui::theme::icons::IconAsset(s.svg),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The row vocabulary the chrome routes on: minted ids always match,
    /// thread ids never do.
    #[test]
    fn row_predicate_matches_the_minted_vocabulary() {
        assert!(!is_external_row("0197uuid-thread-id"));
        let id = mint_id();
        assert!(is_external_row(&id));
        assert!(mint_id() != id, "ids are unique");
    }
}
