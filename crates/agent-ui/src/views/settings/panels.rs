//! Right-pane renderers for the Settings overlay. Each `render_*_panel` is a
//! free function that takes `&mut SettingsView` so the closures it builds for
//! the various controls can update view state directly through the
//! `Entity<Self>` handle captured at render time.
//!
//! Visual style: a vertical stack of section
//! cards. Each card is a rounded rectangle with a subtle secondary background
//! and a single-column list of rows. A row is title (left) + control (right),
//! with an optional muted description line below the title. Rows are separated
//! by a single hairline border so the card reads as a list rather than a
//! grid of unrelated items.

use std::sync::Arc;

use gpui::{
    Anchor, AnyElement, AnyWindowHandle, App, Context, Entity, Focusable, Hsla, InteractiveElement,
    IntoElement, MouseDownEvent, ParentElement as _, SharedString, StatefulInteractiveElement,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::input::InputState;
use gpui_component::theme::Theme;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, WindowExt as _,
    button::Button,
    h_flex,
    menu::{DropdownMenu, PopupMenuItem},
    notification::Notification,
    switch::Switch,
    v_flex,
};

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

type BoolApply = Arc<dyn Fn(&mut SettingsView, bool) + Send + Sync + 'static>;
/// Persists a pick. The display value must flip only on a successful save, so
/// the whole save-then-apply sequence lives in one closure that returns the
/// outcome — the caller (which has the window) surfaces a failure as a toast.
type PostApply = Arc<
    dyn Fn(&mut SettingsView, &SharedString, &mut Context<SettingsView>) -> Result<(), String>
        + Send
        + Sync
        + 'static,
>;

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

/// Build a Switch bound to a field via the view entity. The `apply` closure
/// runs inside `Entity::update`, so it can mutate view state freely.
fn mock_switch(
    id: impl Into<gpui::ElementId>,
    checked: bool,
    view: Entity<SettingsView>,
    apply: BoolApply,
) -> AnyElement {
    Switch::new(id)
        .checked(checked)
        .on_click(move |&new_val, _window, cx| {
            view.update(cx, |this, cx| {
                apply(this, new_val);
                cx.notify();
            });
        })
        .into_any_element()
}

/// A dropdown whose pick persists via `post`. The display value flips only on
/// a successful save (enforced inside `post`); a save failure is surfaced as an
/// in-app toast here, where the window is in scope.
fn mock_dropdown_with_post(
    id: impl Into<gpui::ElementId>,
    label: SharedString,
    options: Vec<(SharedString, SharedString)>,
    view: Entity<SettingsView>,
    post: PostApply,
) -> AnyElement {
    Button::new(id)
        .label(label)
        .dropdown_caret(true)
        .outline()
        .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _window, _cx| {
            options.iter().fold(menu, |menu, (display, token)| {
                let display = display.clone();
                let token = token.clone();
                let view = view.clone();
                let post = post.clone();
                menu.item(
                    PopupMenuItem::new(display.clone()).on_click(move |_ev, window, cx| {
                        if let Err(msg) = view.update(cx, |this, cx| post(this, &token, cx))
                            && !msg.is_empty()
                        {
                            window.push_notification(
                                Notification::error(msg)
                                    .title(i18n::t("settings-save-failed-title")),
                                cx,
                            );
                        }
                    }),
                )
            })
        })
        .into_any_element()
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

// --- General panel --------------------------------------------------------

/// The General panel hosts the UI language picker — the one row with a real
/// persistence backend (`settings.toml` `ui_language`).
pub fn render_general(view: &mut SettingsView, cx: &mut Context<SettingsView>) -> AnyElement {
    let theme = cx.theme().clone();
    let entity = cx.entity();
    let muted = theme.muted_foreground;

    let language_section = section_card(
        &theme,
        vec![
            section_header("settings-section-general-misc"),
            row_with_control(
                i18n::t("settings-row-ui-language"),
                Some(muted_text(i18n::t("settings-desc-ui-language"), muted)),
                mock_dropdown_with_post(
                    "ui-language",
                    view.ui_language.clone(),
                    // Endonym is the label, token the persisted value; both come
                    // from the enum so the picker cannot drift from the parser.
                    [steer_i18n::Language::ZhCn, steer_i18n::Language::En]
                        .into_iter()
                        .map(|lang| {
                            (
                                SharedString::from(lang.endonym()),
                                SharedString::from(lang.token()),
                            )
                        })
                        .collect(),
                    entity,
                    Arc::new(|this, value, cx| this.persist_ui_language(value.clone(), cx)),
                ),
            ),
        ],
    );

    panel_scroll(v_flex().w_full().gap_4().child(language_section))
}

// --- MCP panel ------------------------------------------------------------

/// MCP servers: the merged config (mcp.toml + plugin `.mcp.json` layers)
/// with live connection state from the process registry. Each switch
/// persists to `settings.toml` (`[mcp] disabled`) and applies from the next
/// launch — the registry is built once at startup.
pub fn render_mcp(view: &mut SettingsView, cx: &mut Context<SettingsView>) -> AnyElement {
    let theme = cx.theme().clone();
    let entity = cx.entity();
    let muted = theme.muted_foreground;

    let config = manox_agent::mcp::load_merged_config();
    let servers: Vec<String> = config.mcp_servers.keys().cloned().collect();
    let connected: std::collections::HashMap<String, usize> = manox_agent::mcp::try_global()
        .map(|registry| {
            registry
                .servers()
                .iter()
                .map(|s| (s.name.clone(), s.tools.len()))
                .collect()
        })
        .unwrap_or_default();

    let top_header = v_flex()
        .gap_1()
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_base()
                        .font_weight(gpui::FontWeight::BLACK)
                        .child(i18n::t("settings-panel-mcp")),
                )
                .child(
                    Button::new("add-mcp")
                        .label(i18n::t("settings-btn-add-server"))
                        .outline()
                        .icon(Icon::new(IconName::Plus))
                        .on_click(|_ev, _window, _cx| {
                            // Server registration lives in mcp.toml / plugin
                            // manifests; a guided adder is a follow-up.
                            tracing::info!("add MCP server clicked (no-op in this build)");
                        })
                        .into_any_element(),
                )
                .into_any_element(),
        )
        .child(muted_text(i18n::t("settings-desc-mcp"), muted))
        .child(muted_text(i18n::t("settings-mcp-restart-note"), muted))
        .into_any_element();

    let mut server_rows: Vec<AnyElement> = vec![section_header("settings-section-mcp-servers")];
    if servers.is_empty() {
        server_rows.push(
            v_flex()
                .px_3()
                .py_3()
                .child(muted_text(i18n::t("settings-empty-mcp"), muted))
                .into_any_element(),
        );
    } else {
        for (ix, name) in servers.iter().enumerate() {
            let entity = entity.clone();
            let name = name.clone();
            let checked = !view.mcp_disabled.contains(&name);
            let status: SharedString = if !checked {
                i18n::t("settings-mcp-status-disabled")
            } else if let Some(tool_count) = connected.get(&name) {
                i18n::t_str(
                    "settings-mcp-tool-count",
                    &[("count", &tool_count.to_string())],
                )
            } else {
                i18n::t("settings-mcp-status-not-connected")
            };
            server_rows.push(row_with_control(
                i18n::t("settings-row-mcp-server-name"),
                None,
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_sm().child(name.clone()))
                    .child(div().text_xs().text_color(muted).child(status))
                    .child(mock_switch(
                        format!("mcp-{}", name),
                        checked,
                        entity,
                        Arc::new(move |this, enabled| {
                            if enabled {
                                this.mcp_disabled.remove(&name);
                            } else {
                                this.mcp_disabled.insert(name.clone());
                            }
                            let disabled: Vec<String> = this.mcp_disabled.iter().cloned().collect();
                            if let Err(err) = manox_agent::settings::set_mcp_disabled(disabled) {
                                tracing::warn!(error = %err, "failed to persist MCP switch");
                            }
                        }),
                    ))
                    .into_any_element(),
            ));
            if ix + 1 < servers.len() {
                server_rows.push(hairline(theme.border.opacity(0.6)));
            }
        }
    }
    let servers_section = section_card(&theme, server_rows);

    panel_scroll(
        v_flex()
            .w_full()
            .gap_4()
            .child(top_header)
            .child(servers_section),
    )
}
