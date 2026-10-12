//! Layout + control primitives shared by the large element panels (Models,
//! ChatGPT.app, VS Code) that still draw their own interiors.
//!
//! Visual style: a vertical stack of section
//! cards. Each card is a rounded rectangle with a subtle secondary background
//! and a single-column list of rows. A row is title (left) + control (right),
//! with an optional muted description line below the title. Rows are separated
//! by a single hairline border so the card reads as a list rather than a
//! grid of unrelated items.

use std::sync::Arc;

use gpui::{
    AnyElement, AnyWindowHandle, App, Context, Entity, Focusable, Hsla, InteractiveElement,
    IntoElement, MouseDownEvent, ParentElement as _, SharedString, StatefulInteractiveElement,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::input::InputState;
use gpui_component::theme::Theme;
use gpui_component::{h_flex, v_flex};

use crate::i18n;

use super::SettingsView;

// --- Layout helpers -------------------------------------------------------

/// Wraps the panel content in a vertical scroll container that fills the
/// right pane. The scrollbar appears automatically; we don't need to track
/// the scroll position across re-renders for a settings overlay.
pub(super) fn panel_scroll(content: impl IntoElement) -> AnyElement {
    v_flex()
        .flex_1()
        .h_full()
        .min_h_0()
        .min_w_0()
        .id("settings-right-pane")
        .overflow_y_scroll()
        .p_4()
        .gap_4()
        .child(content)
        .into_any_element()
}

/// A single settings row: title (left) + control (right), with an optional
/// muted description. The description slot is an `AnyElement` so callers can
/// include link chips or other inline widgets without losing styling.
pub(super) fn row_with_control(
    title: SharedString,
    description: Option<AnyElement>,
    control: AnyElement,
) -> AnyElement {
    h_flex()
        .w_full()
        .items_start()
        .justify_between()
        .gap_3()
        .px_3()
        .py_3()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .truncate()
                        .child(title),
                )
                .when_some(description, |this, desc| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(gpui::transparent_black())
                            .child(desc),
                    )
                }),
        )
        .child(control)
        .into_any_element()
}

/// A row used as a section header: title only, no control, smaller type.
pub(super) fn section_header(label: &'static str) -> AnyElement {
    div()
        .px_3()
        .pt_3()
        .pb_1()
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(gpui::transparent_black())
        .child(i18n::t(label))
        .into_any_element()
}

pub(super) fn hairline(divider: Hsla) -> AnyElement {
    div()
        .h(px(1.))
        .w_full()
        .bg(divider)
        .mx_3()
        .into_any_element()
}

/// Section card: rounded rectangle with a secondary background that hosts a
/// list of pre-built rows. A single hairline divider is injected between
/// adjacent rows (not after the last one) so the card reads as a list.
pub(super) fn section_card(theme: &Theme, children: Vec<AnyElement>) -> AnyElement {
    let bg = theme.secondary.opacity(0.45);
    let divider = theme.border.opacity(0.6);
    let muted = theme.muted_foreground;
    let mut col = v_flex().w_full().p_2().gap_0().rounded(px(10.)).bg(bg);
    let last_ix = children.len().saturating_sub(1);
    for (ix, child) in children.into_iter().enumerate() {
        if ix == 0 {
            // First child is the section header — paint muted-foreground text.
            col = col.child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .child(child),
            );
        } else {
            col = col.child(child);
        }
        if ix < last_ix {
            col = col.child(div().h(px(1.)).w_full().bg(divider).mx_3());
        }
    }
    col.into_any_element()
}

pub(super) fn muted_text(label: SharedString, muted: Hsla) -> AnyElement {
    div()
        .text_xs()
        .font_weight(gpui::FontWeight::NORMAL)
        .text_color(muted)
        .child(label)
        .into_any_element()
}

// --- Common controls ------------------------------------------------------

/// Re-enter the view entity through the window handle at the end of the
/// current effect cycle. Blur-flush subscriptions need this: their dispatch
/// leases the view for the whole callback, while the flush needs `&mut
/// Window`, which only the handle re-entry provides. A synchronous
/// `entity.update` inside such a subscription would lease the entity a second
/// time and panic; deferring runs the update after the dispatch lease is
/// released (same re-entry principle as the debounced autosave task).
pub(super) fn defer_blur_flush<T: 'static>(
    cx: &mut Context<T>,
    window_handle: AnyWindowHandle,
    flush: impl FnOnce(&mut T, &mut Window, &mut Context<T>) + 'static,
) {
    let entity = cx.entity();
    cx.defer(move |cx| {
        let _ = window_handle.update(cx, move |_view, window, cx| {
            entity.update(cx, |this, cx| flush(this, window, cx));
        });
    });
}

/// Blur the input when the user clicks anywhere outside it. GPUI leaves
/// focus on the input when buttons or plain space are clicked, so the caret
/// never visibly leaves without this. The focus guard leaves the click
/// undisturbed when the platform already moved focus to another focusable
/// element (e.g. a second input).
pub(super) fn blur_on_click_out(
    state: Entity<InputState>,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    move |_event, window, cx| {
        let handle = Focusable::focus_handle(&state, cx);
        if handle.is_focused(window) {
            window.blur(cx);
        }
    }
}

/// Segmented 2-choice group (visual reference: ChatGPT.app Settings
/// "Default terminal location" bottom/right): active = filled rounded pill
/// (secondary bg + foreground text), inactive = plain muted text, no border.
// Only the macOS-only external-tool panels consume the segmented pair.
#[cfg(target_os = "macos")]
type SegmentedApply =
    Arc<dyn Fn(&mut SettingsView, &mut Context<SettingsView>) + Send + Sync + 'static>;

/// One segment of the pair: its label bundled with the callback that picks it,
/// so the builder stays under the argument-count lint without a params struct.
#[cfg(target_os = "macos")]
type SegmentedOption = (SharedString, SegmentedApply);

#[cfg(target_os = "macos")]
fn mock_segmented(
    id: impl Into<gpui::ElementId>,
    active: bool,
    label: SharedString,
    theme: &Theme,
    view: Entity<SettingsView>,
    apply: SegmentedApply,
) -> AnyElement {
    let id = id.into();
    let (bg, fg, weight) = if active {
        (theme.secondary, theme.foreground, gpui::FontWeight::MEDIUM)
    } else {
        (
            gpui::transparent_black(),
            theme.muted_foreground,
            gpui::FontWeight::NORMAL,
        )
    };
    let hover_fg = theme.foreground;
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded(px(6.))
        .bg(bg)
        .text_xs()
        .font_weight(weight)
        .text_color(fg)
        .cursor_pointer()
        .hover(move |s| s.text_color(hover_fg))
        .on_click(move |_ev, _window, cx| {
            view.update(cx, |this, cx| {
                apply(this, cx);
                cx.notify();
            });
        })
        .child(label)
        .into_any_element()
}

#[cfg(target_os = "macos")]
pub(super) fn build_segmented_pair(
    id_prefix: &'static str,
    active_is_left: bool,
    left: SegmentedOption,
    right: SegmentedOption,
    theme: &Theme,
    view: Entity<SettingsView>,
) -> AnyElement {
    let (left_label, pick_left) = left;
    let (right_label, pick_right) = right;
    h_flex()
        .gap_2()
        .child(mock_segmented(
            format!("{}-L", id_prefix),
            active_is_left,
            left_label,
            theme,
            view.clone(),
            pick_left,
        ))
        .child(mock_segmented(
            format!("{}-R", id_prefix),
            !active_is_left,
            right_label,
            theme,
            view,
            pick_right,
        ))
        .into_any_element()
}
