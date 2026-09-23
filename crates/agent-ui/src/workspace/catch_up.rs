//! Attach-time catch-up for the live-only thread state a parked thread drops.
//!
//! `Workspace::subscribe_background_thread` is deliberately minimal: a thread
//! parked in the background only has its settle/bookkeeping events handled, so
//! everything a running thread streams onto the *observation* surfaces — a
//! sub-agent's child transcript and rail rows, a running tool's output, a live
//! retry notice — is dropped while it is parked. Those events are the ones
//! with no display projection (`journal_translate::history_entries_of`), so the
//! rebuilt transcript cannot reproduce them either.
//!
//! The leaf's journal window still holds every one of them: the follow stream
//! delivered the rows regardless of which thread the user was watching. This
//! module replays the live-only rows that window holds onto the freshly
//! attached thread, so a switch back does not lose the part of the run the user
//! did not watch. The scan is whole-window, not a positional tail: the
//! sub-agent observation state it restores was dropped wholesale by the switch
//! (`clear_subagent_observation`), so a completed run's rows have to come back
//! whole. Each shape is still bounded on its own terms — streamed output stops
//! at the call's settle row, the retry notice as soon as the window moves past
//! it.
//!
//! Split from `workspace.rs` — `super` is the workspace module, so the parent's
//! imports and `Workspace`'s private fields resolve unchanged; the event
//! handlers the parent's live subscription also calls are `pub(super)`.

use super::*;
use manox_agent::thread::SubagentChildEvent;
use manox_protocol::journal::JournalWireEvent;

impl Workspace {
    /// Replay the incoming thread's live-only window rows. Called from the
    /// attach path after the conversation rebuild and the settled sub-agent
    /// rows, so the replayed observation state lands on top of the restored
    /// baseline (a rail row upserts by address, a panel backfill is ordered by
    /// the window's own seq order).
    ///
    /// Only three shapes are replayed, each because nothing else can restore
    /// it: streamed tool output for a call that has not settled, sub-agent
    /// child/progress rows, and the trailing retry notice. Replaying a settled
    /// call's chunks would duplicate the output the display fold already
    /// carries, so the call's `ToolResult` row in the window is the gate.
    pub(super) fn catch_up_live_only_state(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let events: Vec<ThreadEvent> = store.read_with(cx, |handle, _| {
            let window = &handle.store.window;
            let settled: HashSet<&str> = window
                .iter()
                .filter_map(|entry| match &entry.event {
                    JournalWireEvent::ToolResult { call_id, .. } => Some(call_id.as_str()),
                    _ => None,
                })
                .collect();
            let mut last_retry = None;
            let mut out = Vec::new();
            for entry in window {
                match &entry.event {
                    JournalWireEvent::ToolOutputChunk { call_id, .. }
                        if !settled.contains(call_id.as_str()) =>
                    {
                        out.push(crate::journal_translate::thread_event_of(entry));
                    }
                    JournalWireEvent::SubagentChild { .. }
                    | JournalWireEvent::SubagentProgress { .. } => {
                        out.push(crate::journal_translate::thread_event_of(entry));
                    }
                    // A trailing retry badge is live state only while the
                    // window has not moved past it: the live fold pops it on
                    // the first real content or terminal error
                    // (`ConversationState::apply`'s popper set), and a turn
                    // boundary retires it with the turn that scheduled it.
                    // Replaying past either would re-surface a badge the live
                    // path had already popped.
                    JournalWireEvent::Retry { .. } => last_retry = Some(entry),
                    JournalWireEvent::AgentTextDelta { .. }
                    | JournalWireEvent::AgentThinkingDelta { .. }
                    | JournalWireEvent::ToolCall { .. }
                    | JournalWireEvent::Error { .. }
                    | JournalWireEvent::Compaction { .. }
                    | JournalWireEvent::TurnStart
                    | JournalWireEvent::TurnFinish { .. } => last_retry = None,
                    _ => {}
                }
            }
            if handle.store.running
                && let Some(entry) = last_retry
            {
                out.push(crate::journal_translate::thread_event_of(entry));
            }
            out.into_iter().flatten().collect()
        });
        for ev in events {
            match &ev {
                ThreadEvent::SubagentProgress {
                    id,
                    subagent_type,
                    latest_activity,
                    status,
                    health,
                    ..
                } => self.apply_subagent_progress(
                    id,
                    subagent_type,
                    latest_activity.as_deref(),
                    *status,
                    health.as_deref(),
                    cx,
                ),
                ThreadEvent::SubagentChild { id, child } => {
                    self.apply_subagent_child(id, child, cx)
                }
                _ => self.apply_to_conversation(&ev, cx),
            }
        }
    }

    /// The rail + panel half of a sub-agent progress tick. Shared by the live
    /// foreground subscription and the attach catch-up.
    pub(super) fn apply_subagent_progress(
        &mut self,
        id: &str,
        subagent_type: &str,
        latest_activity: Option<&str>,
        status: manox_agent::ToolCallStatus,
        health: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.context_rail.update(cx, |r, cx| {
            r.apply_subagent_progress(id, subagent_type, latest_activity, status, health, cx);
        });
        // Record the completion text so a panel opened later (after the Agent
        // tool-result is gone) can show it.
        if matches!(
            status,
            manox_agent::ToolCallStatus::Success
                | manox_agent::ToolCallStatus::Error
                | manox_agent::ToolCallStatus::Denied
        ) && let Some(text) = latest_activity
        {
            self.subagent_final_text
                .insert(id.to_string(), text.to_string());
        }
        if let Some(panel) = self.subagent_panels.get(id) {
            panel.update(cx, |p, cx| p.set_status(status, cx));
        }
    }

    /// Accumulate one child-session event for the drill-down transcript and
    /// forward it to an already-open panel. Shared by the live foreground
    /// subscription and the attach catch-up.
    pub(super) fn apply_subagent_child(
        &mut self,
        id: &str,
        child: &SubagentChildEvent,
        cx: &mut Context<Self>,
    ) {
        self.subagent_transcripts
            .entry(id.to_string())
            .or_default()
            .push(child.clone());
        if let Some(panel) = self.subagent_panels.get(id) {
            panel.update(cx, |p, cx| p.push(child, cx));
        }
    }

    /// Route one `ThreadEvent` into the conversation with the foreground's
    /// current role/usage/cwd context, then reconcile the list. Shared by the
    /// live foreground subscription and the attach catch-up.
    pub(super) fn apply_to_conversation(&mut self, ev: &ThreadEvent, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        let role = self.model_label(cx);
        let usage = self.store.as_ref().and_then(|s| {
            s.read(cx)
                .store
                .last_token_usage
                .as_ref()
                .map(|u| manox_agent::TokenUsage {
                    input_tokens: u.input,
                    output_tokens: u.output,
                    cache_creation_input_tokens: u.cache_creation,
                    cache_read_input_tokens: u.cache_read,
                })
        });
        let cwd = thread_cwd(&self.thread, &self.store, cx);
        let outcome = self.conversation.update(cx, |c, cx| {
            c.apply(
                ev,
                &role,
                usage,
                crate::conversation::ApplyCtx {
                    weak,
                    cwd,
                    fork_source: self.fork_source_session(cx),
                },
                cx,
            )
        });
        self.apply_list_outcome(outcome, cx);
    }
}
