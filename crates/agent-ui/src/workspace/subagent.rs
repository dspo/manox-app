//! The sub-agent observation face: the per-thread child-transcript state the
//! panels render from, plus the right-pane tab that surfaces one panel per
//! sub-agent address. The tab instance is built here and handed to the live
//! chrome pane through the assembly's opener — the shell owns tab placement,
//! the workspace owns the data a panel displays.

use super::*;

impl Workspace {
    /// Open (or focus) a sub-agent drill-down in the right pane. The run id
    /// names the child session (the swarm invariant): the multiplexer
    /// attaches it and the panel observes the child's own chat fold — the
    /// journal replays through the child's event bucket and the panel fills
    /// live, from settling through active to its settled end state.
    pub(crate) fn open_subagent_tab(
        &mut self,
        id: &str,
        _subagent_type: &str,
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
            self.model_label(cx),
            status,
            self.chat.read(cx).host.clone(),
            source,
            cx,
        );
        // The tab label is the task topic, not the bare run id — a row of
        // hex tails cannot be told apart.
        let tab = crate::tool_tabs::subagent_tab(&banner, panel);
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

    /// Drop the per-thread sub-agent observation state: the accumulated child
    /// transcripts, and every observation panel mounted in the live pane.
    ///
    /// The panels must go with it. A panel renders the transcript accumulated
    /// up to its open, and the shell's per-thread stash would carry it across
    /// the switch — it would then show the thread's history only as of the
    /// moment it was left, while the catch-up refills `subagent_transcripts`
    /// behind it. Retiring them here keeps "a panel shows the transcript as of
    /// its open" true for the reopened one (the rail row reopens from the
    /// fresh transcript).
    pub(super) fn clear_subagent_observation(&mut self, cx: &mut Context<Self>) {
        self.subagent_transcripts.clear();
        crate::chrome_assembly::close_tool_tabs_of_kind("subagent", cx);
    }

    /// Rebuild the per-thread sub-agent observation state (rail rows + panel
    /// prompt/final-text) from the restored transcript. Live rows are fed by
    /// `SubagentProgress` events, which die with the process; this recovers
    /// the settled rows after a restart or a thread switch-back so the rail
    /// and panels are not left empty. Idempotent: rows upsert by address.
    /// Takes precomputed rows (derived inside the store-read closure) so the
    /// callers never clone the full transcript just for this scan.
    pub(super) fn apply_subagent_rows(
        &mut self,
        rows: Vec<manox_agent::subagent_restore::RestoredSubagent>,
        cx: &mut Context<Self>,
    ) {
        for row in rows {
            let first_line = crate::conversation::first_line(&row.prompt);
            self.chat_rail(cx).update(cx, |r, cx| {
                r.apply_subagent_progress(
                    &row.address,
                    &row.subagent_type,
                    first_line.as_deref(),
                    row.status,
                    None,
                    cx,
                );
            });
        }
    }
}
