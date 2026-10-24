//! Panel resize handles, riding upstream `gpui_base::resize_handle` bands:
//! - the band owns the hit strip, the col-resize cursor, the
//!   Idle→Hovered→Pressed→Dragging progression and the drag channel; a
//!   handle only **initiates** the drag (the `on_drag` payload) and the
//!   double-click reset;
//! - `on_drag_move` lives on the **shell root** (see `shell.rs`), which does
//!   the absolute-coordinate math against the root (whole-window) bounds —
//!   mounted on the band itself, `ev.bounds` would be the handle's
//!   bounds and the width would jump to the clamp edge and then track the
//!   moving strip, which is unusable;
//! - the payload types are one per side (gpui dispatches by `TypeId`; a
//!   shared type makes both handles fire on one drag).
//!
//! The bands paint nothing (`invisible_appearance`): the 2026-Light seams
//! are the card gaps themselves, so base's hairline is suppressed rather
//! than restyled.

use std::rc::Rc;

use gpui::{
    AppContext as _, Axis, ClickEvent, InteractiveElement, IntoElement, MouseButton,
    ParentElement as _, Render, Styled, div, px,
};
use gpui_base::{InteractiveElementExt as _, ResizeHandleRenderer, resize_handle};

use crate::shell::Shell;

pub(crate) const HANDLE_WIDTH: f32 = 6.;
pub(crate) const SIDEBAR_DEFAULT: f32 = 224.;
pub(crate) const SIDEBAR_MIN: f32 = 180.;
pub(crate) const SIDEBAR_MAX: f32 = 460.;
pub(crate) const RIGHT_DEFAULT: f32 = 460.;
pub(crate) const RIGHT_MIN: f32 = 320.;
pub(crate) const RIGHT_MAX: f32 = 900.;

/// Sidebar|main drag payload (doubles as the invisible ghost).
#[derive(Clone, Copy, PartialEq)]
pub struct DraggedSidebarDivider;

impl Render for DraggedSidebarDivider {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        gpui::Empty
    }
}

/// Main-card|right-pane drag payload (doubles as the invisible ghost).
#[derive(Clone, Copy, PartialEq)]
pub struct DraggedRightDivider;

impl Render for DraggedRightDivider {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        gpui::Empty
    }
}

/// The chrome's handle appearance: nothing. A divider paints no stroke — the
/// renderer yields an empty element so base's built-in hairline never shows.
fn invisible_appearance() -> ResizeHandleRenderer {
    Rc::new(|_, _, _| Some(gpui::Empty.into_any_element()))
}

/// The invisible sidebar|main handle: the band straddles the seam, hanging
/// off a zero-width anchor that rides the sidebar's right edge (`left =
/// sidebar_width`). Width math lives on the shell root (see the module docs).
pub(crate) fn sidebar_handle(shell: &Shell, cx: &mut gpui::Context<Shell>) -> impl IntoElement {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left(px(f32::from(shell.sidebar_width)))
        .w(px(0.))
        .child(
            resize_handle::<DraggedSidebarDivider, DraggedSidebarDivider>(
                "sidebar-resize-handle",
                Axis::Horizontal,
            )
            .with_appearance(invisible_appearance())
            .on_drag(DraggedSidebarDivider, |_, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| DraggedSidebarDivider)
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_double_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.set_sidebar_width(SIDEBAR_DEFAULT);
                cx.notify();
            })),
        )
}

/// The main-card|right-pane sash: the 6px strip stays the seam itself (an
/// in-layout flex child, invisible), and the band rides it absolutely so the
/// drag keeps the strip's geometry.
pub(crate) fn chat_right_sash(cx: &mut gpui::Context<Shell>) -> impl IntoElement {
    div()
        .w(px(HANDLE_WIDTH))
        .h_full()
        .flex_shrink_0()
        .relative()
        .child(
            resize_handle::<DraggedRightDivider, DraggedRightDivider>(
                "right-resize-sash",
                Axis::Horizontal,
            )
            .with_appearance(invisible_appearance())
            .on_drag(DraggedRightDivider, |_, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| DraggedRightDivider)
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_double_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.right
                    .update(cx, |pane, _cx| pane.set_width(RIGHT_DEFAULT));
                cx.notify();
            })),
        )
}
