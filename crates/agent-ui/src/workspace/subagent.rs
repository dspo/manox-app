//! The sub-agent observation face: the per-thread child-transcript state the
//! panels render from, plus the right-pane tab that surfaces one panel per
//! sub-agent address. The tab instance is built here and handed to the live
//! chrome pane through the assembly's opener — the shell owns tab placement,
//! the workspace owns the data a panel displays.

use super::*;

impl Workspace {
    /// Sub-agent drill-down tabs are per-thread ephemeral state: close the
    /// outgoing thread's panels before rebinding (the transcript itself is
    /// the child session's own fold now — there is nothing local to clear).
    pub(super) fn close_subagent_tabs(&mut self, cx: &mut Context<Self>) {
        crate::chrome_assembly::close_tool_tabs_of_kind("subagent", cx);
    }

    /// Open (or focus) a sub-agent drill-down in the right pane. The run id
    /// names the child session (the swarm invariant): the multiplexer
    /// attaches it and the panel observes the child's own chat fold — the
    /// journal replays through the child's event bucket and the panel fills
    /// live, from settling through active to its settled end state.
    pub(crate) fn open_subagent_tab(
        &mut self,
        id: &str,
        subagent_type: &str,
        topic: &str,
        status: manox_agent::ToolCallStatus,
        cx: &mut Context<Self>,
    ) {
        self.multiplexer
            .update(cx, |m, cx| m.open_or_create(id, true, cx));
        let source = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|(store, _)| (store.clone(), id.to_string()));
        let banner = if topic.is_empty() {
            id.to_string()
        } else {
            topic.to_string()
        };
        let panel = crate::views::subagent_panel::SubagentPanel::new(
            banner.clone(),
            subagent_type.to_string(),
            self.model_label(cx),
            status,
            self.chat.read(cx).host.clone(),
            source,
            cx,
        );
        // The tab's identity is the run id (one panel per run — open_tab
        // dedups by id, so topic-colliding runs must not share a tab);
        // the label is the task topic, not the bare run id — a row of hex
        // tails cannot be told apart.
        let tab = crate::tool_tabs::subagent_tab(id, &banner, panel);
        let address = id.to_string();
        cx.defer(move |cx| {
            if !crate::chrome_assembly::open_tool_tab(tab, cx) {
                tracing::debug!(
                    subagent = %address,
                    "subagent panel requested with no live right pane"
                );
            }
        });
    }

    /// Rebuild the per-thread sub-agent observation rows (the rail's crew
    /// rows) from the persistent child-run roster — the work-channel
    /// registry, settled runs included. This is the restore source (plan
    /// PR-A4): a restart or a switch-back rebuilds the rail from the
    /// roster, where the retired transcript-scan path fed an empty message
    /// set (the roster rides the x-manox channels, which no message scan
    /// can see) and rebuilt nothing. Idempotent: rows upsert by address.
    /// Takes precomputed rows (read inside the store-read closure at the
    /// attach) so the rail update is the only work done here.
    pub(super) fn apply_subagent_rows(
        &mut self,
        rows: Vec<steer_agent_chat_ui::ahp_store::ChildRunRow>,
        cx: &mut Context<Self>,
    ) {
        for run in rows {
            let status = run.display_status();
            let (address, kind, label) = (run.run_id, run.kind, run.label);
            self.chat_rail(cx).update(cx, |r, cx| {
                r.apply_subagent_progress(
                    &address,
                    &kind,
                    (!label.is_empty()).then_some(label.as_str()),
                    status,
                    None,
                    cx,
                );
            });
        }
    }
}
