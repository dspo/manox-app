//! Re-export shim: the ChatColumn state moved to steer-agent-chat-ui's
//! `column` module (Phase 2 tail). The workspace keeps this path so
//! `super::chat_column::…` references resolve.

pub use steer_agent_chat_ui::column::ChatColumn;
