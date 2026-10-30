//! The per-container resource owner registry (plan §3.2 A2): everything a
//! container's life holds OUTSIDE the session itself — the dock terminal
//! view, the right-pane session, and the CLI tabs' kill handles — keyed by
//! the container id (D2: a native thread id verbatim, an
//! `external:{agent}:{uuid}` row id). One container's death reclaims
//! everything under its key; other containers' entries are untouched.
//!
//! The registry lives ON the [`Workspace`]: gpui views are main-thread
//! values (a `RightPaneSession`/`AnyView` cannot sit in a global), and the
//! workspace is the process-lifetime singleton the resources' owner
//! outlives — the legacy stash lifetime, restored.
//!
//! Keying rule: the entries follow the FOREGROUND container at the time the
//! resource was created or the pane was last switched — an external container
//! foreground in the main column owns the live dock/right session under its
//! `external:` id exactly as a native thread does under its session id.

use std::collections::HashMap;
use std::sync::Arc;

use steer_ext_agents::SessionHandle;

use crate::workspace::Workspace;

/// One container's pane-side holdings: the stashed dock view (its drop tears
/// the PTY down) and the stashed right-pane session (the tab set + store).
#[derive(Default)]
pub(crate) struct PaneEntry {
    pub(crate) dock: Option<gpui::AnyView>,
    pub(crate) right: Option<steer_agent_chrome_ui::right_pane::RightPaneSession>,
}

/// A registered CLI-tab kill handle: the tab instance id → (owning
/// container, handle). Registered once at spawn (under the then-foreground
/// container) and removed when killed — by the tab's own close or by the
/// container teardown.
pub(crate) type KillHandles = HashMap<String, (String, Arc<SessionHandle>)>;

impl Workspace {
    // ── dock views ────────────────────────────────────────────────────

    /// Park the outgoing live dock view under its container (the switch-out
    /// half of the dock's per-container stash).
    pub(crate) fn stash_dock(&mut self, container: &str, view: gpui::AnyView) {
        self.pane_stash
            .entry(container.to_string())
            .or_default()
            .dock = Some(view);
    }

    /// Take a container's stashed dock view back (the switch-in half).
    /// `None` when the container never opened a dock — the slot stays empty
    /// and the next expand spawns at that container's attrs cwd.
    pub(crate) fn take_dock(&mut self, container: &str) -> Option<gpui::AnyView> {
        self.pane_stash
            .get_mut(container)
            .and_then(|entry| entry.dock.take())
    }

    /// Which container's dock view is live in the shell right now.
    pub(crate) fn dock_owner(&self) -> Option<String> {
        self.live_dock_owner.clone()
    }

    pub(crate) fn set_dock_owner(&mut self, container: Option<String>) {
        self.live_dock_owner = container;
    }

    // ── right-pane sessions ───────────────────────────────────────────

    /// Park the outgoing live right-pane session under its container.
    pub(crate) fn stash_right(
        &mut self,
        container: &str,
        session: steer_agent_chrome_ui::right_pane::RightPaneSession,
    ) {
        self.pane_stash
            .entry(container.to_string())
            .or_default()
            .right = Some(session);
    }

    /// Take a container's stashed right-pane session back. `None` → the
    /// caller falls back to the container's persisted snapshot.
    pub(crate) fn take_right(
        &mut self,
        container: &str,
    ) -> Option<steer_agent_chrome_ui::right_pane::RightPaneSession> {
        self.pane_stash
            .get_mut(container)
            .and_then(|entry| entry.right.take())
    }

    /// Which container's right-pane session is live in the shell right now.
    pub(crate) fn right_owner(&self) -> Option<String> {
        self.live_right_owner.clone()
    }

    pub(crate) fn set_right_owner(&mut self, container: Option<String>) {
        self.live_right_owner = container;
    }

    // ── CLI-tab kill handles ──────────────────────────────────────────

    /// Register a CLI tab's child handle under `(container, tab instance
    /// id)` — the spawn path's write face. The registry (not the picker
    /// entity alone) owns the kill, so both the tab's close and the
    /// container teardown reach it.
    pub(crate) fn register_kill_handle(
        &mut self,
        container: &str,
        tab_id: &str,
        handle: Arc<SessionHandle>,
    ) {
        self.kill_handles
            .insert(tab_id.to_string(), (container.to_string(), handle));
    }

    /// Kill one tab's child and retire its handle (the tab's own close).
    /// `false` when no handle was registered — a close of a tab whose picker
    /// never launched.
    pub(crate) fn kill_handle_for_tab(&mut self, tab_id: &str) -> bool {
        match self.kill_handles.remove(tab_id) {
            Some((_, handle)) => {
                let _ = handle.kill();
                true
            }
            None => false,
        }
    }

    /// Kill every CLI-tab child a container owns and retire the handles (the
    /// container teardown's kill leg). Returns how many handles were killed.
    pub(crate) fn kill_handles_of_container(&mut self, container: &str) -> usize {
        let owned: Vec<String> = self
            .kill_handles
            .iter()
            .filter(|(_, (owner, _))| owner == container)
            .map(|(tab, _)| tab.clone())
            .collect();
        for tab in &owned {
            if let Some((_, handle)) = self.kill_handles.remove(tab) {
                let _ = handle.kill();
            }
        }
        owned.len()
    }

    // ── teardown ──────────────────────────────────────────────────────

    /// Reclaim a container's registry entries: the stashed dock view and the
    /// stashed right-pane session (the PTYs die with the dropped views). The
    /// container-external half — the LIVE dock/pane — is
    /// `chrome_assembly::reclaim_container_panes`, which must run outside
    /// the workspace lease.
    pub(crate) fn reclaim_stash(&mut self, container: &str) {
        let _ = self.take_dock(container);
        let _ = self.take_right(container);
        // An emptied entry is dead weight (the container is going away).
        if let Some(entry) = self.pane_stash.get(container)
            && entry.dock.is_none()
            && entry.right.is_none()
        {
            self.pane_stash.remove(container);
        }
    }

    // ── diagnostics (test-support only) ───────────────────────────────

    /// The stashed dock keys and whether each holds a view — the
    /// per-container keying regression read.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_dock_stash(&self) -> Vec<(String, bool)> {
        let mut rows: Vec<(String, bool)> = self
            .pane_stash
            .iter()
            .map(|(container, entry)| (container.clone(), entry.dock.is_some()))
            .collect();
        rows.sort();
        rows
    }

    /// Whether a container holds a stashed right-pane session.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_right_stash_contains(&self, container: &str) -> bool {
        self.pane_stash
            .get(container)
            .is_some_and(|entry| entry.right.is_some())
    }

    /// The (container, tab) pairs of the registered kill handles — the
    /// ownership regression read.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_kill_handles(&self) -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = self
            .kill_handles
            .iter()
            .map(|(tab, (owner, _))| (owner.clone(), tab.clone()))
            .collect();
        pairs.sort();
        pairs
    }

    /// The live dock/right owner slots — the switch-tracking regression
    /// read.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_live_owners(&self) -> (Option<String>, Option<String>) {
        (self.dock_owner(), self.right_owner())
    }

    /// Diagnostic: register a CLI tab's kill handle under `(container, tab)`
    /// — the teardown regression plants a REAL child handle here (the
    /// production path spawns through the model picker, which needs a live
    /// CLI registration); the death/kill assertions observe the child.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_register_kill_handle(
        &mut self,
        container: &str,
        tab: &str,
        handle: Arc<SessionHandle>,
    ) {
        self.register_kill_handle(container, tab, handle);
    }
}
