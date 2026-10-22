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
//! Refresh has two halves, because the host serves a child chat SNAPSHOT-ONLY
//! (the child's journal is folded into the child's own channel and is never
//! bridged to the parent, and the parent's background-work entry is set ONCE
//! per run — its `latestActivity` never ticks):
//!
//! - the FETCH is a bounded poll. While the page owns the main column and its
//!   run is still live, a 2s timer re-subscribes the child's chat; the run
//!   leaving the live set earns one last subscribe (its journal ends when the
//!   run ends, so that snapshot is final) and stops the poll.
//! - the REBUILD is driven by the child chat's own fold version, compared on
//!   the store notify the landed snapshot produces. A snapshot that carried
//!   nothing new neither rebuilds nor churns the page.

use super::*;
use steer_agent_chat_ui::ahp_store::{ChatFoldVersion, SubagentWorkRow, chat_fold_version};

/// How often an open page fetches a fresh child-chat snapshot.
const SUBAGENT_STREAM_POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// One poll pass's verdict.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PollStep {
    /// The run is still live: keep the timer armed.
    Keep,
    /// The page is gone, the mode moved on, or the run settled: drop the poll.
    Stop,
}

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
    /// column: subscribe the child's chat once for the click-time transcript,
    /// then poll (see the module note — the host serves a child chat
    /// snapshot-only, so a re-subscribe is the only way it can advance).
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
            // Already on this run: the re-subscribe above is the refresh. The
            // version is synced to what the rebuild renders, and the poll is
            // re-armed (assigning drops the old task, which cancels it) —
            // defence in depth: the "page exists ⟺ its mode owns the column"
            // invariant already routes a re-open through the create arm.
            let poll = Self::spawn_subagent_stream_poll(cx, row.id.clone());
            let version = child_fold_version(&store, &row.id, cx);
            if let Some(page) = self.subagent_stream.as_mut() {
                page.version = version;
                page._poll = poll;
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
            store.clone(),
            host,
            role,
            cx,
        );
        let version = child_fold_version(&store, &row.id, cx);
        let sub = cx.subscribe(
            &view,
            |this, _, _: &crate::views::subagent_stream::BackToConversation, cx| {
                this.leave_subagent_stream(cx)
            },
        );
        let poll = Self::spawn_subagent_stream_poll(cx, row.id.clone());
        self.subagent_stream = Some(SubagentStreamPage {
            id: row.id.clone(),
            version,
            view,
            _poll: poll,
            _sub: sub,
        });
        self.view_mode = ViewMode::SubagentStream;
        cx.notify();
    }

    /// Return the main column to the conversation — the page's back control,
    /// and any thread navigation (the parent thread is still the foreground
    /// one, so no attach is involved). Dropping the page drops its poll.
    pub(crate) fn leave_subagent_stream(&mut self, cx: &mut Context<Self>) {
        if matches!(self.view_mode, ViewMode::SubagentStream) {
            self.view_mode = ViewMode::Workspace;
        }
        if self.subagent_stream.take().is_some() {
            cx.notify();
        }
    }

    /// Rebuild the open page from its child chat's fold when the fold grew
    /// (the store observe's notify — the same one that drives the
    /// conversation rebuild). Gated on the MODE, not just the field: a page
    /// that no longer owns the main column must not keep fetching.
    pub(crate) fn refresh_subagent_stream(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.view_mode, ViewMode::SubagentStream) {
            return;
        }
        let Some(page_id) = self.subagent_stream.as_ref().map(|page| page.id.clone()) else {
            return;
        };
        let Some((store, _)) = self.chat_store(cx) else {
            return;
        };
        let version = child_fold_version(&store, &page_id, cx);
        let Some(page) = self.subagent_stream.as_mut() else {
            return;
        };
        if page.version == version {
            return;
        }
        page.version = version;
        page.view.update(cx, |view, cx| view.refresh(cx));
    }

    /// The page's refresh poll (see the module note). The task lives on the
    /// page record, so closing the page cancels it.
    fn spawn_subagent_stream_poll(cx: &mut Context<Self>, id: String) -> gpui::Task<()> {
        cx.spawn(async move |weak, cx| {
            loop {
                // `background_executor().timer()` — never `tokio::time` on the
                // gpui foreground (that panics: no current tokio runtime).
                cx.background_executor().timer(SUBAGENT_STREAM_POLL).await;
                let Some(ws) = weak.upgrade() else {
                    return;
                };
                let step = ws.update(cx, |this, cx| {
                    let page_is_live = this
                        .subagent_stream
                        .as_ref()
                        .is_some_and(|page| page.id == id)
                        && matches!(this.view_mode, ViewMode::SubagentStream);
                    if !page_is_live {
                        return PollStep::Stop;
                    }
                    // Fetch: this subscribe is what delivers a fresh child
                    // snapshot (the store notify then rebuilds the page).
                    if let Some((store, _)) = this.chat_store(cx) {
                        store.update(cx, |s, cx| s.subscribe(crate::ahp_store::chat_uri(&id), cx));
                    }
                    let live = this.running_subagents(cx).iter().any(|row| row.id == id);
                    if live { PollStep::Keep } else { PollStep::Stop }
                });
                if step == PollStep::Stop {
                    return;
                }
            }
        })
    }
}

/// The child chat's fold version, if the fold holds it at all.
fn child_fold_version(
    store: &Entity<steer_agent_chat_ui::ahp_store::AhpStore>,
    id: &str,
    cx: &App,
) -> Option<ChatFoldVersion> {
    store.read(cx).book.chats.get(id).map(chat_fold_version)
}
