//! The agent-ui side of the [`ChatHost`] port: a thin weak-entity wrapper
//! routing the chat crate's calls back onto `Workspace`. Every method is a
//! no-op once the shell is gone — the same semantics the weak-handle call
//! sites inside the moved views had before the split.

use gpui::{App, Entity, WeakEntity, Window};
use steer_agent_chat_ui::conversation::ConversationState;
use steer_agent_chat_ui::host::ChatHost;

use crate::Workspace;

pub struct WorkspaceChatHost(WeakEntity<Workspace>);

impl WorkspaceChatHost {
    pub fn new(weak: WeakEntity<Workspace>) -> Self {
        Self(weak)
    }
}

fn up(host: &WorkspaceChatHost, _cx: &mut App) -> Option<Entity<Workspace>> {
    host.0.upgrade()
}

impl ChatHost for WorkspaceChatHost {
    fn conversation(&self, cx: &App) -> Option<Entity<ConversationState>> {
        self.0
            .upgrade()
            .map(|ws| ws.read(cx).chat.read(cx).conversation.clone())
    }

    fn dismiss_ask(&self, cx: &mut App) {
        if let Some(ws) = up(self, cx) {
            ws.update(cx, |w, cx| w.dismiss_ask(cx));
        }
    }

    fn submit_ask(&self, replies: Vec<steer_agent_chat_ui::ask_card::AskCardReply>, cx: &mut App) {
        if let Some(ws) = up(self, cx) {
            ws.update(cx, |w, cx| w.reply_ask(replies, cx));
        }
    }

    fn submit_input(&self, window: &mut Window, cx: &mut App) {
        if let Some(ws) = up(self, cx) {
            ws.update(cx, |w, cx| w.submit_input(window, cx));
        }
    }

    fn resolve_tool_confirmation(
        &self,
        auth_id: &str,
        option_id: &str,
        approved: bool,
        cx: &mut App,
    ) {
        // The auth id is the GUARD (a stale button must not settle a
        // different park); the option id rides the verdict so the host
        // learns which option's semantics the user chose.
        if let Some(ws) = up(self, cx) {
            let decision = if approved {
                manox_agent::PermissionDecision::AllowOnce
            } else {
                manox_agent::PermissionDecision::Deny
            };
            ws.update(cx, |w, cx| {
                w.resolve_auth(auth_id, Some(option_id.to_string()), decision, cx);
            });
        }
    }

    fn fork_session_at(&self, through_entry_id: &str, cx: &mut App) {
        if let Some(ws) = up(self, cx) {
            ws.update(cx, |w, cx| w.fork_session_at(through_entry_id, cx));
        }
    }

    fn open_subagent_tab(
        &self,
        id: &str,
        subagent_type: &str,
        topic: &str,
        status: manox_agent::ToolCallStatus,
        cx: &mut App,
    ) {
        if let Some(ws) = up(self, cx) {
            ws.update(cx, |w, cx| {
                w.open_subagent_tab(id, subagent_type, topic, status, cx)
            });
        }
    }
}
