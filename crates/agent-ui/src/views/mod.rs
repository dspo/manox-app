//! View rendering layer.
//!
//! The conversation column's view modules moved to steer-agent-chat-ui
//! (Phase 2); the re-exports below keep every `crate::views::…` path
//! resolving. The shell-side views stay here.

pub mod browser_view;
pub mod composer_menu;
pub mod history_loading;
pub mod management_shell;
pub mod model_cascade;
pub mod plugin_manager;
pub mod settings;
pub mod subagent_panel;

pub use steer_agent_chat_ui::views::{
    CardWidth, MessageListWidthInvalidator, centered, completion, context_rail, message,
    popup_menu, subagents, turn_navigator, turn_rail,
};
