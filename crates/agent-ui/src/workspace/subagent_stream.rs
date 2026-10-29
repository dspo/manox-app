//! The Captain status block's sub-agent lines and the main-column page they
//! open.
//!
//! The live set is AHP state, not client bookkeeping: the host folds each
//! running run into the parent chat's background work
//! (`chat/backgroundWorkSet` / `…Removed`), so the block lists exactly the
//! runs the host still counts as live. A child's own transcript lives in the
//! child's chat (`ahp-chat:/{run-id}`) — the parent's chat carries the run's
//! status, never the child's text — so the click opens that chat as a
//! read-only page in the main column.
//!
//! Following the run is one subscribe, not a loop: the host bridges a running
//! child's journal into its own chat channel, so the subscribe on open
//! delivers the folded history AND the live deltas, and the page re-projects
//! from the fold on its own store observer (see
//! `agent_ui::views::subagent_stream`). No composer: a child run is driven by
//! the parent's dispatch, not by the person watching it.

use super::*;
use steer_agent_chat_ui::ahp_store::SubagentWorkRow;

impl Workspace {
    /// The foreground chat's running sub-agents, in host order.
    pub(crate) fn running_subagents(&self, cx: &App) -> Vec<SubagentWorkRow> {
        self.chat
            .read(cx)
            .store
            .as_ref()
            .map(|(store, sid)| {
                crate::ahp_store::leaf(&store.read(cx).book, sid).running_subagents()
            })
            .unwrap_or_default()
    }

    /// Open (or re-open) one running sub-agent's transcript in the main
    /// column: the child chat's channel is the run's live feed, so one
    /// subscribe arms the page — its own store observer follows the child's
    /// journal from there (see the module note).
    pub(crate) fn open_subagent_stream(&mut self, row: &SubagentWorkRow, cx: &mut Context<Self>) {
        // The FOREGROUND store owns the fold the block was read from; the
        // child's chat rides the same connection.
        let Some((store, _)) = self.chat_store(cx) else {
            return;
        };
        store.update(cx, |s, cx| {
            s.subscribe(crate::ahp_store::chat_uri(&row.id), cx)
        });
        if self
            .subagent_stream
            .as_ref()
            .is_some_and(|page| page.id == row.id)
        {
            // Already on this run: the subscribe above is idempotent and the
            // page's observer is already bound — one projection pass keeps
            // the page current with whatever landed meanwhile.
            if let Some(page) = self.subagent_stream.as_mut() {
                page.view.update(cx, |view, cx| view.refresh(cx));
            }
            self.view_mode = ViewMode::SubagentStream;
            cx.notify();
            return;
        }
        let role = self.model_label(cx);
        let host = self.chat.read(cx).host.clone();
        // The row's `label` is the task topic; the page's recipient and
        // header type are the agent type the spawning call resolved.
        let kind = row
            .kind
            .clone()
            .filter(|kind| !kind.is_empty())
            .unwrap_or_else(|| row.label.clone());
        let topic = row
            .activity
            .clone()
            .filter(|topic| !topic.is_empty())
            .unwrap_or_else(|| row.label.clone());
        let view = crate::views::subagent_stream::SubagentStreamView::new(
            row.id.clone(),
            kind,
            Some(topic),
            row.color
                .as_deref()
                .and_then(steer_agent_chat_ui::views::subagents::agent_color),
            store.clone(),
            host,
            role,
            cx,
        );
        let sub = cx.subscribe(
            &view,
            |this, _, _: &crate::views::subagent_stream::BackToConversation, cx| {
                this.leave_subagent_stream(cx)
            },
        );
        self.subagent_stream = Some(SubagentStreamPage {
            id: row.id.clone(),
            view,
            _sub: sub,
        });
        self.view_mode = ViewMode::SubagentStream;
        cx.notify();
    }

    /// Return the main column to the conversation — the page's back control,
    /// and any thread navigation (the parent thread is still the foreground
    /// one, so no attach is involved). Dropping the page drops its store
    /// observer — a page nobody sees follows nothing.
    pub(crate) fn leave_subagent_stream(&mut self, cx: &mut Context<Self>) {
        if matches!(self.view_mode, ViewMode::SubagentStream) {
            self.view_mode = ViewMode::Workspace;
        }
        if self.subagent_stream.take().is_some() {
            cx.notify();
        }
    }
}
