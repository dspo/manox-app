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
    /// column. The host serves a child chat as a snapshot (the child's
    /// journal is folded into the child's own channel and is not streamed to
    /// the parent), so opening subscribes that channel — and a run that is
    /// still going keeps advancing through [`Self::refresh_subagent_stream`],
    /// which re-subscribes on the parent's own activity ticks.
    pub(crate) fn open_subagent_stream(&mut self, row: &SubagentWorkRow, cx: &mut Context<Self>) {
        // The FOREGROUND store owns the fold the block was read from; the
        // child's chat rides the same connection.
        let Some((store, _)) = self.chat_store(cx) else {
            return;
        };
        store.update(cx, |s, cx| {
            s.subscribe(crate::ahp_store::chat_uri(&row.id), cx)
        });
        if let Some(page) = self.subagent_stream.as_mut()
            && page.id == row.id
        {
            // Already on this run: the re-subscribe above is the refresh.
            page.signature = activity_signature(Some(row));
            page.view.update(cx, |view, cx| view.refresh(cx));
            self.view_mode = ViewMode::SubagentStream;
            cx.notify();
            return;
        }
        let role = self.model_label(cx);
        let host = self.chat.read(cx).host.clone();
        let view = crate::views::subagent_stream::SubagentStreamView::new(
            row.id.clone(),
            row.label.clone(),
            row.activity.clone(),
            store,
            host,
            role,
            cx,
        );
        let sub = cx.subscribe(
            &view,
            |this, _, _: &crate::views::subagent_stream::BackToConversation, cx| {
                this.leave_subagent_stream(cx);
            },
        );
        self.subagent_stream = Some(SubagentStreamPage {
            id: row.id.clone(),
            signature: activity_signature(Some(row)),
            view,
            _sub: sub,
        });
        self.view_mode = ViewMode::SubagentStream;
        cx.notify();
    }

    /// Return the main column to the conversation — the page's back control,
    /// and any thread navigation (the parent thread is still the foreground
    /// one, so no attach is involved).
    pub(crate) fn leave_subagent_stream(&mut self, cx: &mut Context<Self>) {
        if matches!(self.view_mode, ViewMode::SubagentStream) {
            self.view_mode = ViewMode::Workspace;
        }
        if self.subagent_stream.take().is_some() {
            cx.notify();
        }
    }

    /// Re-subscribe the open page when the parent's run reports new activity
    /// (the store observe's notify — the same one that drives the
    /// conversation rebuild). A child chat has no live bridge, so a fresh
    /// subscribe is how a running child's transcript advances; the page then
    /// rebuilds only if the fold actually grew.
    pub(crate) fn refresh_subagent_stream(&mut self, cx: &mut Context<Self>) {
        let Some(page_id) = self.subagent_stream.as_ref().map(|page| page.id.clone()) else {
            return;
        };
        let row = self
            .running_subagents(cx)
            .into_iter()
            .find(|row| row.id == page_id);
        let signature = activity_signature(row.as_ref());
        let Some(page) = self.subagent_stream.as_mut() else {
            return;
        };
        if page.signature == signature {
            return;
        }
        page.signature = signature;
        let view = page.view.clone();
        let Some((store, _)) = self.chat_store(cx) else {
            return;
        };
        store.update(cx, |s, cx| {
            s.subscribe(crate::ahp_store::chat_uri(&page_id), cx)
        });
        view.update(cx, |view, cx| {
            view.refresh(cx);
        });
    }
}

/// The parent-reported activity of one run, as a refresh signature: the
/// host's `latestActivity` (its ticks carry the run's current task label).
/// `None` — the run left the live set, its transcript is final — differs
/// from every `Some(_)`, so settling refreshes once and then stops.
fn activity_signature(row: Option<&SubagentWorkRow>) -> Option<String> {
    row.map(|row| row.activity.clone().unwrap_or_default())
}
