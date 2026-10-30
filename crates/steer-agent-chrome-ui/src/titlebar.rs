//! The single-row toolbar (38px) — [native traffic-light slot | sidebar
//! toggle | ←/→ session-history nav | session picker (window-centered,
//! capped at the dropdown's width; opens the "search + 10 most recent"
//! selector) | "open in editor" | layout toggles ×2 | app brand mark].
//!
//! The control groups hang off two `flex_1` wings; the picker trigger is
//! absolutely positioned (out of flow) so it sits exactly on the window's
//! horizontal center line — flex distribute rounds and wing budgets would
//! otherwise pull it off by half the traffic-slot asymmetry.
//!
//! Window dragging is carried by the toolbar's empty space
//! (`app_owns_titlebar_drag = true`): a row-level mouse-down moves the
//! window, a double-click maximizes, and interactive children stop
//! propagation themselves.

use crate::primitives::{IconButtonState, icon_button};
use crate::session_list::{SessionStatus, single_line_title};
use crate::shell::Shell;
use crate::theme::{
    ACCENT, BORDER, CARD_BG, FG, FG_DIM, FG_FAINT, FG_STRONG, LIST_HOVER, SUCCESS_GREEN, icon,
    icons,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, Render, SharedString, Stateful, StatefulInteractiveElement, Styled,
    Window, div, px, rgba,
};
use gpui_component::input::Input;

/// Content start: the native traffic-light cluster (starting at 12, ≈58
/// wide) + 6 spacing = 70 — the row's left padding; after the first tool
/// button (with its own 12px left padding) lands, its inner underlay sits
/// exactly at x=82, the calibrated position. The padding asymmetry (70
/// left, 12 right) cannot pull the out-of-flow picker off center: Taffy
/// measures absolute insets from the parent's padding box, which spans the
/// full-width row.
const TRAFFIC_SLOT: f32 = 70.;
/// The row's right padding (the left side is TRAFFIC_SLOT).
const ROW_PAD: f32 = 12.;
/// Toolbar height; the picker panel hangs directly below it.
pub(crate) const TITLE_H: f32 = 38.;
/// Picker trigger and dropdown width / max dropdown rows.
pub(crate) const PICKER_WIDTH: f32 = 360.;
const PICKER_LIMIT: usize = 10;

pub(crate) fn render(shell: &Shell, window: &mut Window, cx: &mut Context<Shell>) -> AnyElement {
    let toggle_sidebar = cx.listener(|this, _: &ClickEvent, _w, cx| {
        // A pinned sidebar (host-injected surface, e.g. the settings nav)
        // stays up: it is that mode's only exit.
        if !this.sidebar_pinned {
            this.show_sidebar = !this.show_sidebar;
            cx.notify();
        }
    });
    let toggle_panel = cx.listener(|this, _: &ClickEvent, window, cx| {
        this.toggle_panel(window, cx);
        cx.notify();
    });
    let toggle_right = cx.listener(|this, _: &ClickEvent, _w, cx| {
        this.right.update(cx, |pane, cx| pane.toggle_visible(cx));
        cx.notify();
    });
    let nav_back = cx.listener(|this, _: &ClickEvent, window, cx| {
        this.nav_back(window, cx);
        cx.notify();
    });
    let nav_forward = cx.listener(|this, _: &ClickEvent, window, cx| {
        this.nav_forward(window, cx);
        cx.notify();
    });
    let open_editor = cx.listener(|this, _: &ClickEvent, window, cx| {
        this.open_editor(window, cx);
        cx.notify();
    });
    let avail = shell.nav_avail(cx);

    // Drag-to-move is armed on mouse-down and only fired once the pointer
    // actually moves — a plain press+release on the row is a click (or part
    // of one on an interactive child), and starting an interactive move on
    // the press would let the window manager swallow the release (same
    // pattern as gpui-component's TitleBar).
    let drag = window.use_state(cx, |_, _| TitlebarDrag { armed: false });

    div()
        .id("titlebar")
        .on_mouse_down_out(window.listener_for(&drag, |state, _, _, _| state.armed = false))
        .on_mouse_down(
            MouseButton::Left,
            window.listener_for(&drag, |state, e: &gpui::MouseDownEvent, window, _| {
                // Double click zooms/restores; a plain press only arms the
                // drag (fired on the first move).
                if e.click_count >= 2 {
                    state.armed = false;
                    window.zoom_window();
                } else {
                    state.armed = true;
                }
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            window.listener_for(&drag, |state, _, _, _| state.armed = false),
        )
        .on_mouse_move(window.listener_for(&drag, |state, _, window, _| {
            if state.armed {
                state.armed = false;
                window.start_window_move();
            }
        }))
        .flex()
        .relative()
        .w_full()
        .h(px(TITLE_H))
        .flex_shrink_0()
        // The traffic-light slot exists only on macOS; elsewhere the row
        // starts at the normal inset.
        .pl(px(if cfg!(target_os = "macos") {
            TRAFFIC_SLOT
        } else {
            ROW_PAD
        }))
        .pr(px(ROW_PAD))
        .gap(px(6.))
        .items_center()
        .bg(crate::theme::SHELL_BG)
        .text_size(px(12.5))
        .text_color(FG_DIM)
        // Two flex_1 wings flank the picker; the picker itself is
        // out-of-flow (absolute, see session_picker) at the window's center
        // line. The wings only host the controls — the traffic-slot budget
        // stays on the row's left padding, so the wings' outer boxes stay
        // equal and nothing else competes with the center.
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(px(6.))
                .child(icon_button(
                    "tb-sidebar",
                    icons::LAYOUT_SIDEBAR_LEFT,
                    15.,
                    // A pinned sidebar (host-injected surface, e.g. the
                    // settings nav) paints disabled: the click is a no-op.
                    if shell.sidebar_pinned {
                        IconButtonState::Disabled
                    } else if shell.show_sidebar {
                        IconButtonState::On
                    } else {
                        IconButtonState::Off
                    },
                    move |e, w, cx| toggle_sidebar(e, w, cx),
                ))
                // Session-history navigation: a move with no edge behind/ahead
                // stays inert and paints dimmed (the host's history stack owns
                // the edges).
                .child(nav_button(
                    "tb-back",
                    icons::ARROW_LEFT,
                    avail.back,
                    "chrome-titlebar-back",
                    move |e, w, cx| nav_back(e, w, cx),
                ))
                .child(nav_button(
                    "tb-fwd",
                    icons::ARROW_RIGHT,
                    avail.forward,
                    "chrome-titlebar-forward",
                    move |e, w, cx| nav_forward(e, w, cx),
                )),
        )
        // Right wing, flush against the row's right padding.
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .justify_end()
                .gap(px(6.))
                // "Open in editor": hands the foreground session's workspace to
                // the host (which launches the user's editor against it).
                .child(
                    icon_button(
                        "tb-editor",
                        icons::CODE,
                        13.,
                        IconButtonState::Off,
                        move |e, w, cx| open_editor(e, w, cx),
                    )
                    .tooltip(|window, cx| {
                        gpui_component::tooltip::Tooltip::new(steer_i18n::t(
                            "chrome-titlebar-open-editor",
                        ))
                        .build(window, cx)
                    }),
                )
                .child(icon_button(
                    "tb-panel",
                    icons::LAYOUT_PANEL,
                    15.,
                    if shell.show_panel {
                        IconButtonState::On
                    } else {
                        IconButtonState::Off
                    },
                    move |e, w, cx| toggle_panel(e, w, cx),
                ))
                // Right-pane toggle.
                .child(icon_button(
                    "tb-right",
                    icons::LAYOUT_SIDEBAR_RIGHT,
                    15.,
                    if shell.right.read(cx).visible {
                        IconButtonState::On
                    } else {
                        IconButtonState::Off
                    },
                    move |e, w, cx| toggle_right(e, w, cx),
                ))
                // The app brand mark: a deliberate non-interactive slot, held
                // out of the window-drag surface (the mouse-down is swallowed)
                // so pressing it never drags the window; the host injects the
                // mark, the shell keeps a generic glyph as the fallback.
                .child(
                    div()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .size(px(22.))
                        .rounded_full()
                        .bg(ACCENT)
                        .text_color(crate::theme::BADGE_BLUE_FG)
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_shrink_0()
                        .children(match shell.brand_element() {
                            Some(el) => vec![el],
                            None => vec![
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(icon(icons::CODE, 12.))
                                    .into_any_element(),
                            ],
                        }),
                )
                // Linux/FreeBSD client-side decorations: our own window
                // controls at the very right (macOS uses the system traffic
                // lights over the left slot).
                .children(window_controls(window)),
        )
        // The session picker trigger: out-of-flow (mounted last so it paints
        // above both wings), pinned to the window's center line at a fixed
        // width — [`picker_left`] is the same origin its dropdown derives
        // from (see `Shell::render`), so the panel re-opens exactly over the
        // trigger.
        .child(session_picker(shell, window, cx))
        .into_any_element()
}

/// Drag-arm state for the toolbar row (see `render`).
struct TitlebarDrag {
    armed: bool,
}

impl Render for TitlebarDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The window controls at the toolbar's right end — Linux/FreeBSD's stand-in
/// for the macOS traffic lights, shown only while this window actually draws
/// client-side decorations (where gpui had to fall back to server-side ones,
/// the window manager's titlebar already carries its own set and ours would
/// stack a duplicate). The compositor's declared capabilities trim what we
/// offer: a tiling compositor may honor neither minimize nor maximize; close
/// is always ours.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn window_controls(window: &Window) -> Vec<AnyElement> {
    use gpui::Decorations;

    if !matches!(window.window_decorations(), Decorations::Client { .. }) {
        return Vec::new();
    }
    let supported = window.window_controls();
    let mut controls = Vec::new();
    if supported.minimize {
        controls.push(control_button("win-minimize", icons::MINIMIZE, |window| {
            window.minimize_window();
        }));
    }
    if supported.maximize {
        let restore = window.is_maximized();
        controls.push(control_button(
            "win-maximize",
            if restore {
                icons::COPY
            } else {
                icons::MAXIMIZE
            },
            |window| window.zoom_window(),
        ));
    }
    controls.push(control_button("win-close", icons::CLOSE, |window| {
        window.remove_window();
    }));
    controls
}

/// One 38×38 hover block, same semantics as gpui-component's window controls:
/// the mouse-down is swallowed so the row's drag-to-move never fires, and the
/// click carries the action.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn control_button(
    id: &'static str,
    glyph: crate::theme::IconAsset,
    action: impl Fn(&mut Window) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .w(px(38.))
        .h_full()
        .flex_shrink_0()
        .justify_center()
        .content_center()
        .items_center()
        .rounded(px(6.))
        .hover(|style| style.bg(crate::theme::LIST_HOVER))
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .on_click(move |_, window, _| action(window))
        .child(icon(glyph, 13.))
        .into_any_element()
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn window_controls(_window: &Window) -> Vec<AnyElement> {
    Vec::new()
}

/// A session-history nav button: live when the move has an edge to land on,
/// the shared flat-button's Disabled tone otherwise (same geometry, no
/// reflow, mouse-downs still swallowed, hover tooltip explains the move).
fn nav_button(
    id: &'static str,
    glyph: crate::theme::IconAsset,
    enabled: bool,
    tooltip_key: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    icon_button(
        id,
        glyph,
        14.,
        if enabled {
            IconButtonState::Off
        } else {
            IconButtonState::Disabled
        },
        on_click,
    )
    .tooltip(move |window, cx| {
        gpui_component::tooltip::Tooltip::new(steer_i18n::t(tooltip_key)).build(window, cx)
    })
    .into_any_element()
}

/// The picker trigger's and dropdown's shared x origin: the window's
/// horizontal center line at [`PICKER_WIDTH`]. Both faces derive their
/// placement from this one helper, so the panel always re-opens exactly
/// over its trigger.
pub(crate) fn picker_left(window: &Window) -> Pixels {
    (window.viewport_size().width - px(PICKER_WIDTH)) / 2.
}

/// The session picker (trigger + dropdown panel). The trigger row: a folder
/// glyph + the active session title + a chevron, fixed at [`PICKER_WIDTH`]
/// (the dropdown's width — the panel re-opens exactly over its trigger);
/// clicking opens "search + the 10 most recent", picking one switches the
/// active session. Absolutely positioned on the window's center line, so
/// flex never gets a vote; on a window narrower than the two wings' buttons
/// plus the trigger it overlays them.
fn session_picker(shell: &Shell, window: &mut Window, cx: &mut Context<Shell>) -> AnyElement {
    let left = picker_left(window);
    let active_title: String = shell
        .active
        .as_ref()
        .and_then(|id| shell.sessions.iter().find(|s| &s.id == id))
        .map(|s| single_line_title(&s.title))
        .unwrap_or_else(|| shell.main.title(cx).to_string());

    let open_picker = cx.listener(|this, _: &ClickEvent, window, cx| {
        this.toggle_session_picker(window, cx);
        cx.notify();
    });

    div()
        .id("session-picker-trigger")
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |e, w, cx| open_picker(e, w, cx))
        .absolute()
        .left(left)
        .top(px((TITLE_H - 24.) / 2.))
        .w(px(PICKER_WIDTH))
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
/// `deferred(anchored(...))` (see `shell.rs`), window-centered under the
/// titlebar, so it re-opens exactly over its trigger.
pub(crate) fn picker_panel(shell: &Shell, cx: &mut Context<Shell>) -> AnyElement {
    // Trimmed like the sidebar filter: a stray trailing space in the query
    // must not hide rows whose title ends on the typed term.
    let query = shell
        .picker_query
        .as_ref()
        .map(|q| q.read(cx).value().trim().to_lowercase());
    let q = query.unwrap_or_default();

    let items: Vec<(String, String, String, bool)> = shell
        .sessions
        .iter()
        .filter_map(|s| {
            // The query matches the one-line face the rows render — the
            // raw title's hard breaks never match a typed space.
            let title = single_line_title(&s.title);
            (q.is_empty()
                || title.to_lowercase().contains(&q)
                || s.workspace.to_lowercase().contains(&q))
            .then(|| {
                (
                    s.id.clone(),
                    title,
                    s.workspace.clone(),
                    matches!(s.status, SessionStatus::Unread),
                )
            })
        })
        .take(PICKER_LIMIT)
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
                            .child(steer_i18n::t("chrome-picker-empty"))
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

/// One candidate row: title + unread dot (the slot's green completion
/// marker — one fact, one color) | workspace.
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
                        el.bg(SUCCESS_GREEN)
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
