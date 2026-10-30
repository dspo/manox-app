//! The main-column external sessions — the legacy shell's external-session
//! family on the chrome face. A session is a live PTY terminal (a CLI
//! agent's TUI or a plain shell) the project menu launched. It renders IN
//! PLACE OF the conversation column (`ViewMode::ExternalSession` — the
//! shell wraps the main column, and the main column is where terminal/TUI
//! sessions live); switching away parks the session (its terminal keeps
//! running), closing drops the view and tears the process tree down.
//!
//! Identity (the container model, D2): each session's id mints in the
//! manox-app-style row-id namespace `external:{agent}:{uuid}`
//! (`container::ContainerId::external`) — namespaced so it never collides
//! with a manox thread UUID in the sidebar's selection namespace — and the
//! record keeps the REAL external session id behind it (`cx_session_id`,
//! derived from the cx session socket's `<id>.sock` filename; empty for
//! plain terminals and when the IPC bind failed). The row id is what the
//! sidebar shows and routes on; the cx id is the traceable link to
//! `~/.manox/sessions/`.
//!
//! In-memory only — the PTY dies with the process, nothing persists. The
//! sidebar rows are projected by [`Workspace::external_session_rows`] and
//! merged into the assembly's snapshot (the chrome regroups them under the
//! project's header).

use std::path::PathBuf;

use steer_agent_chrome_ui::session_list::{SessionRowKind, SessionStatus};
use steer_agent_chrome_ui::shell::{MAX_EXTERNAL_TITLE_CHARS, SessionRow};

use super::Workspace;
use super::container;

/// One live external container (the SessionContainer model's external home —
/// this record IS the container): the identity pair, the sidebar row's
/// fields, and the terminal entity that occupies the main column while the
/// session is foreground. The container face (kind, attrs snapshot, alive
/// bit) lives beside the row fields; native containers are views over
/// threads and have no record here (see `container`).
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
    /// localized terminal name — until the program inside claims its own
    /// title).
    pub label: String,
    /// The launch-time label — what a terminal title reset (`Title(None)`)
    /// restores; the OSC stream's default, never the rename editor's concern.
    pub default_label: String,
    pub svg: &'static str,
    /// The project directory the session was launched in — the sidebar
    /// group it merges into; `None` → the loose Chats bucket. The
    /// container's project association for this home (app memory; the live
    /// read rides `Workspace::container_project`).
    pub project: Option<PathBuf>,
    pub created_at: i64,
    pub view: gpui::Entity<terminal_ui::TerminalView>,
    /// The terminal's `Title` events → [`Self::label`]. Lives (and dies)
    /// with the record: the session's PTY outlives nothing else.
    pub(super) _title_sub: gpui::Subscription,
    // ── the container face (the A1 upgrade) ──
    /// The kind the 0 号 decides: the launched CLI agent. `None` for a plain
    /// terminal — its 0 号 is a bare shell, which the kind vocabulary has no
    /// entry for.
    pub kind: Option<container::ContainerKind>,
    /// The creation-time attribute snapshot (D10): the spawn cwd + the
    /// environment at the creation call, frozen — resources spawned later
    /// (A2) inherit this copy.
    pub attrs: container::ContainerAttrs,
    /// The death predicate's external half (D8): true until the PTY's
    /// child-exit event lands. Flipping it raises
    /// `container::ContainerEvent::Died`; the record itself stays until the
    /// lifecycle PR wires the teardown.
    pub alive: bool,
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
    /// focused, so the TUI takes keyboard input immediately. The record
    /// itself (identity, attrs snapshot, the PTY subscriptions) lands
    /// through the unified container entry; what remains here is the
    /// foreground hand-off.
    pub fn spawn_external_session(
        &mut self,
        launch: ExternalSessionLaunch<'_>,
        view: gpui::Entity<terminal_ui::TerminalView>,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let id = self.create_container(
            container::ContainerSeed::External(container::ExternalContainerSeed {
                agent_id: launch.agent_id,
                cx_session_id: launch.cx_session_id,
                label: launch.label,
                svg: launch.svg,
                project: launch.project,
                view: view.clone(),
            }),
            cx,
        );
        // The main column changes hands. The sub-agent page obeys the same
        // invariant its own fields do — it exists only while its mode owns the
        // column — so the hand-off drops it (and its refresh poll with it);
        // `open_subagent_stream` is the only way back.
        self.subagent_stream = None;
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
        // Same hand-off as the spawn path: the sub-agent page never outlives
        // the mode that owns the column.
        self.subagent_stream = None;
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

    /// The external row menu's 重命名 write-back (the shell's host hook):
    /// retitle the record — the sidebar row and the main-column titlebar
    /// both read it. The next terminal `Title` event may claim the title
    /// back; last writer wins.
    pub fn set_external_title(&mut self, id: &str, title: String, cx: &mut gpui::Context<Self>) {
        let Some(record) = self.externals.iter_mut().find(|s| s.id == id) else {
            return;
        };
        if record.label == title {
            return;
        }
        record.label = title;
        cx.notify();
    }

    /// The terminal's own title claim (the PTY's OSC 0/2 stream): retitles
    /// the row; `None` (a program's title reset) restores the launch label.
    /// No-op on an unchanged or empty title — TUIs re-emit constantly and
    /// each emission must not churn the sidebar.
    pub(super) fn apply_terminal_title(
        &mut self,
        id: &str,
        title: Option<String>,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(record) = self.externals.iter_mut().find(|s| s.id == id) else {
            return;
        };
        let next = match title {
            Some(t) => clamp_external_title(&t),
            None => record.default_label.clone(),
        };
        if next.is_empty() || record.label == next {
            return;
        }
        record.label = next;
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

    /// Whether the Settings split layout owns the main column. The
    /// regression read for the attach tail's mode guard: a stale attach
    /// must not flip past Settings (the nav's injected surface is mounted
    /// exactly while this is true).
    pub fn settings_foreground(&self) -> bool {
        matches!(self.view_mode, super::ViewMode::Settings)
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
                    rank: i64::MAX,
                    pinned: false,
                    archived: false,
                    tag: None,
                    team_leader: false,
                }
            })
            .collect()
    }
}

/// Clamp a terminal's raw title to the chrome's title budget (chars, not
/// bytes — the OSC stream is arbitrary program output) and trim it; an
/// empty result means "not a title", the caller keeps the old label.
pub(super) fn clamp_external_title(raw: &str) -> String {
    raw.trim().chars().take(MAX_EXTERNAL_TITLE_CHARS).collect()
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

    /// The OSC stream is arbitrary program output: the clamp trims and caps
    /// it at the chrome's title budget (char-counted, emoji included).
    #[test]
    fn titles_are_trimmed_and_capped() {
        assert_eq!(clamp_external_title("  ✳ my task  "), "✳ my task");
        let long = "x".repeat(MAX_EXTERNAL_TITLE_CHARS + 40);
        assert_eq!(
            clamp_external_title(&long).chars().count(),
            MAX_EXTERNAL_TITLE_CHARS
        );
        assert_eq!(clamp_external_title("   "), "", "blank is not a title");
    }
}
