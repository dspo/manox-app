//! Accessors over the conversation's state entity, shared by the app frame and
//! the column that will render itself.
//!
//! The column split gives one piece of state two readers: `Workspace` (which
//! frames the column, and still owns the surroundings) and `ChatColumnView`
//! (which renders the column). The wrong answers are to duplicate the accessor
//! on both entities or to route it through the env port — the first forks the
//! definition, the second lets session state cross the effect boundary.
//!
//! These are free functions over the state entity instead: the dependency runs
//! **entity → conversation state**, never entity → other entity, and each side
//! keeps a one-line method for call-site compatibility.

use gpui::{App, Entity};
use gpui_component::input::TextareaState;

use crate::views::context_rail::ContextRail;
use steer_agent_chat_ui::column::ChatColumn;
use steer_agent_chat_ui::conversation::ConversationState;

/// The column's composer input state.
pub(crate) fn input(chat: &Entity<ChatColumn>, cx: &App) -> Entity<TextareaState> {
    chat.read(cx).input_state.clone()
}

/// The column's context rail (the right-edge usage/observations rail).
pub(crate) fn rail(chat: &Entity<ChatColumn>, cx: &App) -> Entity<ContextRail> {
    chat.read(cx).context_rail.clone()
}

/// The column's conversation projection (the message list's source).
pub(crate) fn conversation(chat: &Entity<ChatColumn>, cx: &App) -> Entity<ConversationState> {
    chat.read(cx).conversation.clone()
}
