//! The agent-ui side of the [`ChatHost`] port: a thin weak-entity wrapper
//! routing the chat crate's calls back onto `Workspace`. Every method is a
//! no-op once the shell is gone — the same semantics the weak-handle call
//! sites inside the moved views had before the split.

use gpui::{App, Entity, WeakEntity, Window};
use manox_agent_chat_ui::conversation::ConversationState;
use manox_agent_chat_ui::host::ChatHost;

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

    fn decide_ask_option(&self, qi: usize, oi: usize, cx: &mut App) {
        if let Some(ws) = up(self, cx) {
            ws.update(cx, |w, cx| w.decide_ask_option(qi, oi, cx));
        }
    }

    fn toggle_ask_option(&self, step: usize, oi: usize, cx: &mut App) {
        if let Some(ws) = up(self, cx) {
            ws.update(cx, |w, cx| w.toggle_ask_option(step, oi, cx));
        }
    }

    fn ask_prev(&self, cx: &mut App) {
        if let Some(ws) = up(self, cx) {
            ws.update(cx, |w, cx| w.ask_prev(cx));
        }
    }

    fn ask_next(&self, cx: &mut App) {
        if let Some(ws) = up(self, cx) {
            ws.update(cx, |w, cx| w.ask_next(cx));
        }
    }

    fn skip_ask_question(&self, qi: usize, window: &mut Window, cx: &mut App) {
        if let Some(ws) = up(self, cx) {
            ws.update(cx, |w, cx| w.skip_ask_question(qi, window, cx));
        }
    }

    fn submit_input(&self, window: &mut Window, cx: &mut App) {
        if let Some(ws) = up(self, cx) {
            ws.update(cx, |w, cx| w.submit_input(window, cx));
        }
    }

    fn ask_custom_state(
        &self,
        qi: usize,
        cx: &App,
    ) -> Option<Entity<gpui_component::input::InputState>> {
        self.0.upgrade().and_then(|ws| {
            let ws = ws.read(cx);
            ws.chat
                .read(cx)
                .ask_custom_inputs
                .get(qi)
                .and_then(|slot| slot.clone())
        })
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
