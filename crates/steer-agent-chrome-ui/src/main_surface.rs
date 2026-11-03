//! The main-column surface contract.
//!
//! Chat is the first intended implementation (steer-agent-chat-ui); the
//! settings overlay is the second. A full-window terminal or an external
//! CLI-agent session is also a whole-surface view: the assembly swaps the
//! main column for it (`ViewMode::ExternalSession`) — there is no dock
//! maximize route; the dock is a bottom panel, never a column swap.

use std::sync::Arc;

use gpui::{AnyView, App, SharedString};

pub type MainSurfaceHandle = Arc<dyn MainSurface>;

/// The main column's content. The shell mounts `view` inside the main card
/// and labels the titlebar session-picker trigger with `title`.
pub trait MainSurface: 'static {
    fn view(&self) -> AnyView;
    /// Label for the titlebar picker trigger when no session is active.
    fn title(&self, cx: &App) -> SharedString;
}
