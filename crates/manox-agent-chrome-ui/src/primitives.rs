//! Base controls — icon buttons, kbd chips, badges — matching the
//! agents-window calibration value for value (sizes, radii, colors).

use crate::theme::{
    ACCENT, BADGE_BLUE_BG, BADGE_BLUE_FG, FG_DIM, ICON_ON_BG, Icon, SURFACE_ACTIVE,
    SURFACE_TERTIARY, TOOLBAR_HOVER, icon,
};
use gpui::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, Stateful,
    StatefulInteractiveElement, Styled, Window, div, px, rgba,
};

/// Icon button — the flat-button + inner-underlay pair from the calibration:
/// outer box 6/12 padding (the 47×33 hit points), whole-box hover
/// `surface_tertiary #F0F0F2`, radius 6; inner (3,4) radius-4 underlay carries
/// the active state (accent glyph + accent 10% background).
pub fn icon_button(
    id: impl Into<ElementId> + Clone,
    glyph: Icon,
    size: f32,
    on: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<gpui::Div> {
    let mut inner = div()
        .id(id.clone())
        .py(px(3.))
        .px(px(4.))
        .rounded(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .child(icon(glyph, size));
    if on {
        inner = inner
            .text_color(ACCENT)
            .bg(ICON_ON_BG)
            .hover(|style| style.bg(rgba(0x0069CC40)));
    } else {
        inner = inner.text_color(FG_DIM);
    }
    // The outer box is the flat button: 6/12 padding, radius 6, hover
    // surface_tertiary, pressed surface_active. Left-button-down stops
    // propagation because host toolbars commonly use row-level mouse-down as
    // a window-drag zone — interactive controls must not be dragged along.
    div()
        .id(id)
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(on_click)
        .py(px(6.))
        .px(px(12.))
        .rounded(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .text_color(FG_DIM)
        .hover(|style| style.bg(SURFACE_TERTIARY))
        .active(|style| style.bg(SURFACE_ACTIVE))
        .child(inner)
}

/// Small icon action (tab close, inline pin/archive): padding 2 + radius 3,
/// hover wash. The caller sets colors through `.text_color` on an ancestor.
pub fn small_icon_button(
    id: impl Into<ElementId>,
    glyph: Icon,
    size: f32,
    color: gpui::Rgba,
    hover_color: gpui::Rgba,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .on_click(move |e, window, cx| {
            cx.stop_propagation();
            on_click(e, window, cx);
        })
        .p(px(2.))
        .rounded(px(3.))
        .flex()
        .flex_shrink_0()
        .text_color(color)
        .hover(|style| style.bg(TOOLBAR_HOVER).text_color(hover_color))
        .child(icon(glyph, size))
}

/// kbd chip (e.g. ⌘N): a 10px gray mini-pill.
pub fn kbd_chip(label: &str) -> impl IntoElement {
    div()
        .px(px(3.))
        .text_size(px(10.))
        .text_color(FG_DIM)
        .child(label.to_string())
}

/// Small badge (e.g. NEW): a blue-on-white mini-pill.
pub fn badge(label: &str) -> impl IntoElement {
    div()
        .py(px(1.))
        .px(px(4.))
        .rounded(px(4.))
        .bg(BADGE_BLUE_BG)
        .text_color(BADGE_BLUE_FG)
        .text_size(px(9.))
        .flex_shrink_0()
        .child(label.to_string())
}
