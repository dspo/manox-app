//! The chat column's port to its host (PLAN-CHROME-CHAT-SPLIT §3.2 — the
//! dependency-inversion face that lets chat views live outside the shell).
//!
//! The chat crate's views and state must never hold a shell entity. Instead
//! they hold a [`ChatHost`] and call through it; the host side (agent-ui's
//! `WorkspaceChatHost`) wraps a weak shell entity and performs the update
//! internally. Every method is a no-op on a dead shell — the same semantics
//! the weak-handle call sites had before the split.
//!
//! The surface is deliberately narrow: the ask-drawer verdicts, composer
//! submit, session fork, the one right-pane action the rail triggers, and a
//! conversation-handle accessor for the message items' click paths.

use std::sync::Arc;

use gpui::{App, Entity, Window};

use crate::conversation::ConversationState;

pub type ChatHostHandle = Arc<dyn ChatHost>;

/// Host-side actions the chat column mirrors to. Implemented by the shell
/// assembly (today agent-ui's Workspace; at Phase 4 the new state layer).
pub trait ChatHost: Send + Sync + 'static {
    /// The conversation entity the message items act on (`None` when the
    /// shell is gone — callers treat it as a no-op, matching the old
    /// weak-upgrade failure path).
    fn conversation(&self, cx: &App) -> Option<Entity<ConversationState>>;
    /// Dismiss the pending ask card (its × affordance).
    fn dismiss_ask(&self, cx: &mut App);
    /// Toggle option `oi` of question `qi` on the pending ask card.
    fn decide_ask_option(&self, qi: usize, oi: usize, cx: &mut App);
    /// Toggle option `oi` of the ask card's step `step` (the option row's
    /// click).
    fn toggle_ask_option(&self, step: usize, oi: usize, cx: &mut App);
    /// Step to the previous ask question.
    fn ask_prev(&self, cx: &mut App);
    /// Step to the next ask question.
    fn ask_next(&self, cx: &mut App);
    /// Mark question `qi` explicitly skipped.
    fn skip_ask_question(&self, qi: usize, window: &mut Window, cx: &mut App);
    /// Submit the composer's current input.
    fn submit_input(&self, window: &mut Window, cx: &mut App);
    /// The step-`qi` custom-answer input entity of the pending ask card
    /// (allocated on the render path by the shell).
    fn ask_custom_state(
        &self,
        qi: usize,
        cx: &App,
    ) -> Option<Entity<gpui_component::input::InputState>>;
    /// Fork the session at `through_entry_id`.
    fn fork_session_at(&self, through_entry_id: &str, cx: &mut App);
    /// Open the right-pane observation tab for a sub-agent (the rail's row
    /// click).
    fn open_subagent_tab(
        &self,
        id: &str,
        subagent_type: &str,
        topic: &str,
        status: manox_agent::ToolCallStatus,
        cx: &mut App,
    );
}

/// An all-no-op host for tests: every action swallows, every accessor is
/// `None` — exactly the dead-shell semantics, without needing a shell.
pub struct NoopHost;

impl ChatHost for NoopHost {
    fn conversation(&self, _cx: &App) -> Option<Entity<ConversationState>> {
        None
    }
    fn dismiss_ask(&self, _cx: &mut App) {}
    fn decide_ask_option(&self, _qi: usize, _oi: usize, _cx: &mut App) {}
    fn toggle_ask_option(&self, _step: usize, _oi: usize, _cx: &mut App) {}
    fn ask_prev(&self, _cx: &mut App) {}
    fn ask_next(&self, _cx: &mut App) {}
    fn skip_ask_question(&self, _qi: usize, _window: &mut Window, _cx: &mut App) {}
    fn submit_input(&self, _window: &mut Window, _cx: &mut App) {}
    fn ask_custom_state(
        &self,
        _qi: usize,
        _cx: &App,
    ) -> Option<Entity<gpui_component::input::InputState>> {
        None
    }
    fn fork_session_at(&self, _through_entry_id: &str, _cx: &mut App) {}
    fn open_subagent_tab(
        &self,
        _id: &str,
        _subagent_type: &str,
        _topic: &str,
        _status: manox_agent::ToolCallStatus,
        _cx: &mut App,
    ) {
    }
}

/// A shared [`NoopHost`] handle for tests.
pub fn noop_host() -> ChatHostHandle {
    Arc::new(NoopHost)
}
