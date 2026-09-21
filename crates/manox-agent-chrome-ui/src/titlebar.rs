//! The single-row toolbar (38px) — [native traffic-light slot | sidebar
//! toggle | ←/→ | session picker (fills the middle; opens the
//! "search + 10 most recent" selector) | run/split | VS logo | the
//! "Sync Changes 1↑" pill | layout toggles ×2 | account avatar].
//!
//! Window dragging is carried by the toolbar's empty space
//! (`app_owns_titlebar_drag = true`): a row-level mouse-down moves the
//! window, a double-click maximizes, and interactive children stop
//! propagation themselves.

use crate::primitives::icon_button;
use crate::shell::Shell;
use crate::theme::{
    ACCENT, BORDER, CARD_BG, FG, FG_DIM, FG_FAINT, FG_STRONG, LIST_HOVER, icon, icons,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton,
    ParentElement, SharedString, Stateful, StatefulInteractiveElement, Styled, Window, div, px,
    rgba,
};
use gpui_component::input::Input;

/// Content start: the native traffic-light cluster (starting at 12, ≈58
/// wide) + 6 spacing = 70 — after the first tool button (with its own 12px
/// left padding) lands, its inner underlay sits exactly at x=82, the
/// calibrated position.
const TRAFFIC_SLOT: f32 = 70.;
/// Picker dropdown width / max rows.
const PICKER_WIDTH: f32 = 360.;
const PICKER_LIMIT: usize = 10;

pub(crate) fn render(shell: &Shell, _window: &mut Window, cx: &mut Context<Shell>) -> AnyElement {
    let toggle_sidebar = cx.listener(|this, _: &ClickEvent, _w, cx| {
        this.show_sidebar = !this.show_sidebar;
        cx.notify();
    });
    let toggle_panel = cx.listener(|this, _: &ClickEvent, window, cx| {
        this.toggle_panel(window, cx);
        cx.notify();
    });
    let toggle_right = cx.listener(|this, _: &ClickEvent, _w, cx| {
        this.right.update(cx, |pane, cx| pane.toggle_visible(cx));
        cx.notify();
    });

    div()
        .id("titlebar")
        .on_mouse_down(MouseButton::Left, |e: &gpui::MouseDownEvent, window, _| {
            // Empty space / non-interactive children: drag to move, double
            // click to zoom/restore.
            if e.click_count >= 2 {
                window.zoom_window();
            } else {
                window.start_window_move();
            }
        })
        .flex()
        .w_full()
        .h(px(38.))
        .flex_shrink_0()
        .pl(px(TRAFFIC_SLOT))
        .pr(px(12.))
        .gap(px(6.))
        .items_center()
        .bg(crate::theme::SHELL_BG)
        .text_size(px(12.5))
        .text_color(FG_DIM)
        .child(icon_button(
            "tb-sidebar",
            icons::LAYOUT_SIDEBAR_LEFT,
            15.,
            shell.show_sidebar,
            move |e, w, cx| toggle_sidebar(e, w, cx),
        ))
        .child(icon_button(
            "tb-back",
            icons::ARROW_LEFT,
            14.,
            false,
            |_, _, _| {},
        ))
        .child(icon_button(
            "tb-fwd",
            icons::ARROW_RIGHT,
            14.,
            false,
            |_, _, _| {},
        ))
        // The session picker: fills the middle span, click opens the
        // selector.
        .child(session_picker(shell, cx))
        .child(icon_button("tb-run", icons::PLAY, 14., false, |_, _, _| {}))
        .child(icon_button(
            "tb-split",
            icons::SPLIT_HORIZONTAL,
            14.,
            false,
            |_, _, _| {},
        ))
        // The VS logo block stays non-interactive: it remains part of the
        // window-drag surface.
        .child(
            div()
                .size(px(18.))
                .rounded(px(4.))
                .bg(ACCENT)
                .text_color(crate::theme::BADGE_BLUE_FG)
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .child(icon(icons::CODE, 12.)),
        )
        // The "Sync Changes" + 1↑ pill.
        .child(
            div()
                .id("tb-sync")
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .h(px(24.))
                .px(px(8.))
                .gap(px(5.))
                .rounded(px(5.))
                .bg(ACCENT)
                .text_color(crate::theme::BADGE_BLUE_FG)
                .flex()
                .items_center()
                .flex_shrink_0()
                .child(icon(icons::SYNC, 13.))
                .child(div().child(manox_i18n::t("chrome-titlebar-sync")))
                .child(
                    div()
                        .px(px(4.))
                        .rounded(px(6.))
                        .bg(rgba(0xFFFFFF40))
                        .text_size(px(10.5))
                        .flex()
                        .items_center()
                        .child("1↑"),
                ),
        )
        .child(icon_button(
            "tb-panel",
            icons::LAYOUT_PANEL,
            15.,
            shell.show_panel,
            move |e, w, cx| toggle_panel(e, w, cx),
        ))
        // Right-pane toggle.
        .child(icon_button(
            "tb-right",
            icons::LAYOUT_SIDEBAR_RIGHT,
            15.,
            shell.right.read(cx).visible,
            move |e, w, cx| toggle_right(e, w, cx),
        ))
        // Account avatar.
        .child(
            div()
                .id("tb-avatar")
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .size(px(22.))
                .rounded_full()
                .bg(rgba(0x7099D8FF))
                .text_color(crate::theme::BADGE_BLUE_FG)
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .child(icon(icons::ACCOUNT, 13.)),
        )
        .into_any_element()
}

/// The session picker (trigger + dropdown panel). The trigger row: a folder
/// glyph + the active session title + a chevron; clicking opens
/// "search + the 10 most recent", picking one switches the active session.
fn session_picker(shell: &Shell, cx: &mut Context<Shell>) -> AnyElement {
    let active_title: String = shell
        .active
        .as_ref()
        .and_then(|id| shell.sessions.iter().find(|s| &s.id == id))
        .map(|s| s.title.clone())
        .unwrap_or_else(|| shell.main.title(cx).to_string());

    let open_picker = cx.listener(|this, e: &ClickEvent, window, cx| {
        let anchor = match e {
            ClickEvent::Mouse(m) => m.down.position,
            _ => gpui::Point::default(),
        };
        this.toggle_session_picker(anchor, window, cx);
        cx.notify();
    });

    div()
        .id("session-picker-trigger")
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |e, w, cx| open_picker(e, w, cx))
        .flex_1()
        .min_w_0()
        .h(px(24.))
        .px(px(8.))
        .gap(px(5.))
        .rounded(px(5.))
        .bg(CARD_BG)
        .border_1()
        .border_color(BORDER)
        .text_color(FG)
        .items_center()
        .flex()
        .child(icon(icons::FOLDER_OPENED, 13.))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .child(active_title.clone()),
        )
        .child(icon(icons::CHEVRON_DOWN, 12.))
        .into_any_element()
}

/// The dropdown body: a search box + the filtered most-recent list (top 10
/// by update time). **Not positioned** — the shell root mounts it through
/// `deferred(anchored(...))` (see `shell.rs`).
pub(crate) fn picker_panel(shell: &Shell, cx: &mut Context<Shell>) -> AnyElement {
    let query = shell
        .picker_query
        .as_ref()
        .map(|q| q.read(cx).value().to_lowercase());
    let q = query.unwrap_or_default();

    let items: Vec<(String, String, String, bool)> = shell
        .sessions
        .iter()
        .filter(|s| {
            q.is_empty()
                || s.title.to_lowercase().contains(&q)
                || s.workspace.to_lowercase().contains(&q)
        })
        .take(PICKER_LIMIT)
        .map(|s| (s.id.clone(), s.title.clone(), s.workspace.clone(), s.unread))
        .collect();

    // Closures in the loop go through the entity handle (a `cx.listener`
    // return value borrows and cannot escape the FnMut).
    let shell_entity = cx.entity();
    let rows: Vec<AnyElement> = items
        .iter()
        .map(|it| {
            let (id, title, workspace, unread) = it;
            let ent = shell_entity.clone();
            let id = id.clone();
            let selected = shell.active.as_deref() == Some(id.as_str());
            let id_for_row = id.clone();
            let on_pick = move |_: &ClickEvent, w: &mut Window, cx: &mut App| {
                let id = id.clone();
                ent.update(cx, |this, cx| {
                    this.select(&id, w, cx);
                    this.session_picker_open = false;
                    cx.notify();
                });
            };
            row(&id_for_row, title, workspace, *unread, selected, on_pick).into_any_element()
        })
        .collect();

    div()
        .absolute()
        .top(px(31.))
        .left_0()
        .w(px(PICKER_WIDTH))
        .max_h(px(360.))
        .rounded(px(6.))
        .bg(CARD_BG)
        .border_1()
        .border_color(BORDER)
        .shadow_md()
        .py(px(4.))
        .px(px(4.))
        .flex()
        .flex_col()
        .gap(px(2.))
        .overflow_hidden()
        .text_size(px(12.5))
        .child(search_box(shell, cx))
        .child(
            div()
                .id("session-picker-list")
                .flex()
                .flex_col()
                .gap(px(1.))
                .max_h(px(300.))
                .overflow_y_scroll()
                .children(if rows.is_empty() {
                    vec![
                        div()
                            .py(px(8.))
                            .px(px(6.))
                            .text_color(FG_FAINT)
                            .child(manox_i18n::t("chrome-picker-empty"))
                            .into_any_element(),
                    ]
                } else {
                    rows
                }),
        )
        .into_any_element()
}

/// The search box, bound to `Shell.picker_query` (an InputState).
fn search_box(shell: &Shell, _cx: &mut Context<Shell>) -> AnyElement {
    let Some(query) = shell.picker_query.clone() else {
        return div().into_any_element();
    };
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .h(px(26.))
        .px(px(6.))
        .rounded(px(5.))
        .border_1()
        .border_color(BORDER)
        .flex_shrink_0()
        .text_color(FG_DIM)
        .child(icon(icons::SEARCH, 13.))
        .child(
            Input::new(&query)
                .appearance(false)
                .h_full()
                .w_full()
                .text_size(px(12.5)),
        )
        .into_any_element()
}

/// One candidate row: title + unread dot | workspace.
#[allow(clippy::too_many_arguments)]
fn row(
    id: &String,
    title: &String,
    workspace: &String,
    unread: bool,
    selected: bool,
    on_pick: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<gpui::Div> {
    div()
        .id(SharedString::from(format!("picker-{id}")))
        .on_click(on_pick)
        .w_full()
        .py(px(5.))
        .px(px(6.))
        .rounded(px(4.))
        .flex()
        .items_center()
        .gap(px(6.))
        .when(selected, |this| this.bg(LIST_HOVER))
        .hover(|style| style.bg(LIST_HOVER))
        .child(
            div()
                .size(px(6.))
                .rounded_full()
                .map(|el| {
                    if unread {
                        el.bg(ACCENT)
                    } else {
                        el.bg(rgba(0x00000000))
                    }
                })
                .flex_shrink_0(),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_color(FG_STRONG)
                .child(title.to_string()),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(FG_FAINT)
                .flex_shrink_0()
                .child(workspace.to_string()),
        )
}
