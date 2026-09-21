//! The bottom dock — a generic container whose content is injected via
//! [`PanelSurface`]. The chrome owns the card, the tab strip and the
//! show/hide lifecycle; what renders inside (a terminal in manox-app's
//! assembly, anything else elsewhere) is the host's choice.
//!
//! Lifecycle contract: `open` runs when the panel expands (spawn side
//! effects — mount-equals-launch), `close` when it collapses (the view is
//! dropped and resources tear down with it — unmount-equals-teardown, e.g.
//! a PTY's process tree).

use std::sync::Arc;

use gpui::{AnyView, App, SharedString, Window};

use crate::theme::Icon;

/// Bottom-dock content. The chrome never interprets the view — it mounts it
/// full-bleed under the tab strip.
pub trait PanelSurface: 'static {
    fn title(&self) -> SharedString;
    fn icon(&self) -> Icon;
    /// Expand: create the view (side effects allowed — this is the event
    /// path). An `Err` renders the shell's error body with a retry.
    fn open(&self, window: &mut Window, cx: &mut App) -> Result<AnyView, String>;
    /// Collapse: teardown hook; the default just drops the view handle.
    fn close(&self, _view: AnyView, _cx: &mut App) {}
}

/// One live panel slot: the surface plus its opened view (or the failure
/// text from `open`).
pub(crate) struct PanelSlot {
    pub surface: Arc<dyn PanelSurface>,
    pub view: Option<AnyView>,
    pub error: Option<String>,
}
