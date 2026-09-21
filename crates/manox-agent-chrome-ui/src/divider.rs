//! Panel resize handles:
//! - a handle only **initiates** the drag (the `on_drag` payload, `<|>`
//!   cursor) and the double-click reset;
//! - `on_drag_move` lives on the **shell root** (see `shell.rs`), which does
//!   the absolute-coordinate math against the root (whole-window) bounds —
//!   mounted on the 6px handle itself, `ev.bounds` would be the handle's
//!   bounds and the width would jump to the clamp edge and then track the
//!   moving strip, which is unusable;
//! - the payload types are one per side (gpui dispatches by `TypeId`; a
//!   shared type makes both handles fire on one drag).

use gpui::{
    AppContext as _, CursorStyle, InteractiveElement, IntoElement, MouseButton, Render,
    StatefulInteractiveElement, Styled, div, px,
};

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

/// The invisible sidebar|main handle: absolutely positioned, centered on the
/// card seam (`left = sidebar_width`). Width math lives on the shell root
/// (see the module docs).
pub(crate) fn sidebar_handle(shell: &Shell, cx: &mut gpui::Context<Shell>) -> impl IntoElement {
    div()
        .id("sidebar-resize-handle")
        .absolute()
        .top_0()
        .bottom_0()
        .left(px(f32::from(shell.sidebar_width)))
        .w(px(HANDLE_WIDTH))
        .cursor(CursorStyle::ResizeLeftRight)
        .on_drag(DraggedSidebarDivider, |_, _, _, cx| {
            cx.stop_propagation();
            cx.new(|_| DraggedSidebarDivider)
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, e: &gpui::MouseUpEvent, _w, cx| {
                if e.click_count >= 2 {
                    this.set_sidebar_width(SIDEBAR_DEFAULT);
                    cx.notify();
                }
            }),
        )
}

/// The main-card|right-pane sash — an **in-layout flex child**: its 6px width
/// is the card seam itself, invisible (no divider stroke drawn).
pub(crate) fn chat_right_sash(cx: &mut gpui::Context<Shell>) -> impl IntoElement {
    div()
        .id("right-resize-sash")
        .w(px(HANDLE_WIDTH))
        .h_full()
        .flex_shrink_0()
        .cursor(CursorStyle::ResizeLeftRight)
        .on_drag(DraggedRightDivider, |_, _, _, cx| {
            cx.stop_propagation();
            cx.new(|_| DraggedRightDivider)
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, e: &gpui::MouseUpEvent, _w, cx| {
                if e.click_count >= 2 {
                    this.right
                        .update(cx, |pane, _cx| pane.set_width(RIGHT_DEFAULT));
                    cx.notify();
                }
            }),
        )
}
