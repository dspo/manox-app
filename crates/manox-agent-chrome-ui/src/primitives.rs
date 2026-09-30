//! Base controls — icon buttons, kbd chips, badges — matching the
//! agents-window calibration value for value (sizes, radii, colors).

use crate::theme::{
    ACCENT, BADGE_BLUE_BG, BADGE_BLUE_FG, FG_DIM, FG_FAINT, ICON_ON_BG, Icon, SURFACE_ACTIVE,
    SURFACE_TERTIARY, TOOLBAR_HOVER, icon,
};
use gpui::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, Stateful,
    StatefulInteractiveElement, Styled, Window, div, px, rgba,
};

/// The flat-button's three tones. One geometry for all of them — a state
/// flip must never reflow the row, and a disabled control still looks like
/// the control it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconButtonState {
    /// Lit: accent glyph + accent underlay.
    On,
    /// Idle: dim glyph, hover/pressed washes.
    Off,
    /// Inert: faint glyph, no hover/pressed/click — but still swallows
    /// mouse-down (toolbar rows are window-drag zones) and keeps the box.
    Disabled,
}

/// Icon button — the flat-button + inner-underlay pair from the calibration:
/// outer box 6/12 padding (the 47×33 hit points), radius 6; inner (3,4)
/// radius-4 underlay. Geometry is shared by all three tones; the tone
/// ([`IconButtonState`]) decides hover/pressed/click — hover
/// `surface_tertiary` and the accent underlay belong to `On`/`Off` only.
/// The inner glyph carries a `<id>-glyph` debug selector for render tests.
pub fn icon_button(
    id: impl Into<ElementId>,
    glyph: Icon,
    size: f32,
    state: IconButtonState,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<gpui::Div> {
    let id: ElementId = id.into();
    let glyph_selector = format!("{id}-glyph");
    let mut inner = div()
        .id(id.clone())
        .debug_selector(move || glyph_selector.clone())
        .py(px(3.))
        .px(px(4.))
        .rounded(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .child(icon(glyph, size));
    // The outer box is the flat button: 6/12 padding, radius 6. Left-button-
    // down stops propagation because host toolbars commonly use row-level
    // mouse-down as a window-drag zone — interactive controls must not be
    // dragged along.
    let mut outer = div()
        .id(id)
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .py(px(6.))
        .px(px(12.))
        .rounded(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .text_color(FG_DIM);
    match state {
        IconButtonState::On => {
            inner = inner
                .text_color(ACCENT)
                .bg(ICON_ON_BG)
                .hover(|style| style.bg(rgba(0x0069CC40)));
            outer = outer
                .on_click(on_click)
                .hover(|style| style.bg(SURFACE_TERTIARY))
                .active(|style| style.bg(SURFACE_ACTIVE));
        }
        IconButtonState::Off => {
            inner = inner.text_color(FG_DIM);
            outer = outer
                .on_click(on_click)
                .hover(|style| style.bg(SURFACE_TERTIARY))
                .active(|style| style.bg(SURFACE_ACTIVE));
        }
        IconButtonState::Disabled => {
            inner = inner.text_color(FG_FAINT);
        }
    }
    outer.child(inner)
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
