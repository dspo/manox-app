//! The chat column's port to its host (the dependency-inversion face that
//! lets chat views live outside the shell).
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

use crate::ask_card::AskCardReply;
use crate::conversation::ConversationState;

pub type ChatHostHandle = Arc<dyn ChatHost>;

/// Host-side actions the chat column mirrors to. Implemented by the shell
/// assembly (agent-ui's workspace).
pub trait ChatHost: Send + Sync + 'static {
    /// The conversation entity the message items act on (`None` when the
    /// shell is gone — callers treat it as a no-op, matching the old
    /// weak-upgrade failure path).
    fn conversation(&self, cx: &App) -> Option<Entity<ConversationState>>;
    /// Dismiss the pending ask card (its header's close control).
    fn dismiss_ask(&self, cx: &mut App);
    /// Settle the pending ask with the card's answers, in question order. The
    /// host folds them onto the wire (option labels, untouched questions as
    /// explicit skips) — the card reports indices and note text only.
    fn submit_ask(&self, replies: Vec<AskCardReply>, cx: &mut App);
    /// Submit the composer's current input.
    fn submit_input(&self, window: &mut Window, cx: &mut App);
    /// Settle the tool confirmation parked under `auth_id` — the unified
    /// confirmation card's button verdict. `approved` is the verdict bool;
    /// `option_id` names the fold option the user clicked (it rides the
    /// wire verdict so the host learns which option's semantics apply);
    /// the auth id rides the verdict's `_meta` stamp, the identity the
    /// host settles by.
    fn resolve_tool_confirmation(
        &self,
        auth_id: &str,
        option_id: &str,
        approved: bool,
        cx: &mut App,
    );
    /// The ask card body's tracked scroll handle for step `qi` — `None`
    /// before the ask scratch is allocated (the render path allocates it), in
    /// which case the card keeps gpui's untracked element-state scroll and
    /// takes no wheel guard.
    fn ask_body_scroll(&self, qi: usize, cx: &App) -> Option<gpui::ScrollHandle>;
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
    fn ask_body_scroll(&self, _qi: usize, _cx: &App) -> Option<gpui::ScrollHandle> {
        None
    }
    fn dismiss_ask(&self, _cx: &mut App) {}
    fn submit_ask(&self, _replies: Vec<AskCardReply>, _cx: &mut App) {}
    fn submit_input(&self, _window: &mut Window, _cx: &mut App) {}
    fn resolve_tool_confirmation(
        &self,
        _auth_id: &str,
        _option_id: &str,
        _approved: bool,
        _cx: &mut App,
    ) {
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
