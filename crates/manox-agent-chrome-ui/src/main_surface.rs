//! The main-column surface contract.
//!
//! Chat is the first intended implementation (manox-agent-chat-ui); the
//! settings overlay is the second. Full-window terminal / external-session
//! needs go through **dock maximize**, never through this slot — the main
//! surface only ever swaps for whole-surface views, not for tools.

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
