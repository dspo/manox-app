//! View rendering layer.
//!
//! The conversation column's view modules moved to manox-agent-chat-ui
//! (Phase 2); the re-exports below keep every `crate::views::…` path
//! resolving. The shell-side views stay here.

pub mod browser_view;
pub mod composer_menu;
pub mod launcher;
pub mod management_shell;
pub mod model_cascade;
pub mod plugin_manager;
pub mod settings;
pub mod sidebar;
pub mod subagent_panel;
pub mod title_menu;

pub use manox_agent_chat_ui::views::{
    MessageListWidthInvalidator, centered, completion, context_rail, message, popup_menu,
    subagents, turn_navigator,
};
