//! View rendering layer.
//!
//! The conversation column's view modules moved to steer-agent-chat-ui
//! (Phase 2); the re-exports below keep every `crate::views::…` path
//! resolving. The shell-side views stay here.

// Compiled out on Linux/FreeBSD together with the rest of the integrated
// browser (see `lib.rs`).
#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
pub mod browser_view;
pub mod composer_menu;
pub mod history_loading;
pub mod management_shell;
pub mod model_cascade;
pub mod plugin_manager;
pub mod settings;
pub mod settings_nav;
pub mod subagent_panel;
pub mod subagent_stream;

pub use steer_agent_chat_ui::views::{
    CardWidth, MessageListWidthInvalidator, centered, completion, context_rail, message,
    popup_menu, subagents, turn_navigator, turn_rail,
};
