//! The create/edit-project dialog surface (the project entity's §2.4 form) —
//! a props-driven modal: the name field (the placeholder is host-supplied,
//! so the follow-the-primary rule stays host semantics), the source-folder
//! rows (the primary badge MOVES between rows — order is never the badge's
//! carrier), and the cancel/confirm action row. The chrome owns layout and
//! interaction only: the draft state, the folder picker and the registry
//! writes are host work, wired through the callbacks exactly like
//! [`crate::session_list`]'s.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Entity, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui_component::input::{Escape, Input, InputState};

use crate::primitives::badge;
use crate::session_list::{OnUnit, OnWindowApp};
use crate::theme::{
    ACCENT, ACCENT_HOVER, BORDER, CARD_BG, CARD_RADIUS, ERR_RED, FG, FG_DIM, FG_FAINT, LIST_HOVER,
    SHELL_BG, icon, icons,
};

/// A source-folder row as projected by the host: the absolute path verbatim
/// (the tooltip's payload and the action callbacks' key), the display
/// basename, and the primary badge.
#[derive(Clone, PartialEq)]
pub struct ProjectRootRow {
    pub path: String,
    pub label: String,
    pub primary: bool,
}

/// Root-targeted action (set-primary / remove), carrying the row's path.
pub type OnRootPath = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// The dialog's fixed geometry — a form card, not a panel: wide enough for a
/// path row's badge + remove control, narrow enough to read as a dialog.
const DIALOG_W: f32 = 460.;
const ROOT_ROW_H: f32 = 30.;

/// The project dialog (creation and editing share one form; `edit` swaps the
/// title, the confirm label and the remove row). Confirm is inert while the
/// root set is empty — a project needs at least one folder.
#[derive(IntoElement)]
pub struct ProjectDialog {
    pub edit: bool,
    /// The name field (host-owned entity; the placeholder rides it).
    pub name_input: Entity<InputState>,
    pub roots: Vec<ProjectRootRow>,
    /// Inline failure line under the roots card (red, no popup). Host
    /// formatted — save failures carry the registry's error detail.
    pub error: Option<String>,
    pub confirm_disabled: bool,
    pub on_add_folder: OnUnit,
    pub on_set_primary: OnRootPath,
    pub on_remove_root: OnRootPath,
    /// Dismissal without saving — the ✕, the cancel button and Escape all
    /// route here (a plain window callback, so the key path needs no click).
    pub on_cancel: OnWindowApp,
    pub on_confirm: OnUnit,
    /// Edit mode only: the remove-project row (the host confirms before
    /// acting — the chrome renders the affordance, not the verdict).
    pub on_remove: Option<OnUnit>,
}

impl RenderOnce for ProjectDialog {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let title_key = if self.edit {
            "project-edit-title"
        } else {
            "project-create-title"
        };
        let confirm_key = if self.edit {
            "project-save"
        } else {
            "project-create-confirm"
        };
        let on_cancel = self.on_cancel.clone();

        div()
            .w(px(DIALOG_W))
            .bg(CARD_BG)
            .border_1()
            .border_color(BORDER)
            .rounded(px(CARD_RADIUS))
            .shadow_lg()
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .text_size(px(12.5))
            .text_color(FG)
            // Escape dismisses from wherever focus sits inside the dialog:
            // the raw key (focus on the card itself) and the input's
            // re-raised Escape action (focus in the name field) both land.
            .on_key_down({
                let on_cancel = on_cancel.clone();
                move |ev: &KeyDownEvent, w, cx| {
                    if ev.keystroke.key == "escape" {
                        cx.stop_propagation();
                        on_cancel(w, cx);
                    }
                }
            })
            .on_action({
                let on_cancel = on_cancel.clone();
                move |_: &Escape, w, cx| on_cancel(w, cx)
            })
            // Header: the mode title + the ✕ dismissal.
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_size(px(13.))
                            .child(steer_i18n::t(title_key)),
                    )
                    .child(
                        div()
                            .id("project-dialog-close")
                            .on_click({
                                let on_cancel = on_cancel.clone();
                                move |_, w, cx| {
                                    cx.stop_propagation();
                                    on_cancel(w, cx);
                                }
                            })
                            .p(px(3.))
                            .rounded(px(4.))
                            .text_color(FG_FAINT)
                            .hover(|style| style.text_color(FG).bg(LIST_HOVER))
                            .child(icon(icons::CLOSE, 13.)),
                    ),
            )
            // Name field.
            .child(field_label(
                "project-dialog-name-label",
                "project-name-label",
            ))
            .child(
                div().h(px(28.)).child(
                    Input::new(&self.name_input)
                        .appearance(true)
                        .h_full()
                        .w_full(),
                ),
            )
            // Source folders.
            .child(field_label(
                "project-dialog-roots-label",
                "project-roots-label",
            ))
            .child(
                div()
                    .w_full()
                    .border_1()
                    .border_color(BORDER)
                    .rounded(px(6.))
                    .p(px(4.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(self.roots.iter().map(|row| root_row(row, &self)))
                    .child(add_folder_row(&self)),
            )
            .children(
                self.error
                    .clone()
                    .map(|message| div().text_size(px(11.5)).text_color(ERR_RED).child(message)),
            )
            .child(action_row(&self, confirm_key))
    }
}

/// A field label — the dim 12px caption above each control.
fn field_label(id: &'static str, key: &'static str) -> impl IntoElement {
    div()
        .id(id)
        .text_size(px(12.))
        .text_color(FG_DIM)
        .child(steer_i18n::t(key))
}

/// One source-folder row: folder glyph + basename (whole path on hover) +
/// the primary badge or the set-as-primary action + the remove ✕.
fn root_row(row: &ProjectRootRow, dialog: &ProjectDialog) -> impl IntoElement {
    let path = row.path.clone();
    div()
        .id(SharedString::from(format!("project-root-{}", row.path)))
        .h(px(ROOT_ROW_H))
        .px(px(6.))
        .rounded(px(4.))
        .flex()
        .items_center()
        .gap(px(6.))
        .hover(|style| style.bg(LIST_HOVER))
        .tooltip(move |window, cx| {
            gpui_component::tooltip::Tooltip::new(path.clone()).build(window, cx)
        })
        .child(div().text_color(FG_FAINT).child(icon(icons::FOLDER, 14.)))
        .child(div().flex_1().min_w_0().truncate().child(row.label.clone()))
        // The badge is the primary fact itself — a state on the row, not a
        // button; every other row carries the set-as-primary action instead.
        .children(if row.primary {
            Some(badge(steer_i18n::t("project-root-primary").as_str()).into_any_element())
        } else {
            let path = row.path.clone();
            let on_set = dialog.on_set_primary.clone();
            Some(
                div()
                    .id(SharedString::from(format!(
                        "project-root-set-primary-{}",
                        row.path
                    )))
                    .on_click(move |_, w, cx| {
                        cx.stop_propagation();
                        on_set(path.as_str(), w, cx);
                    })
                    .px(px(4.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .text_size(px(10.5))
                    .text_color(FG_FAINT)
                    .hover(|style| style.text_color(FG))
                    .child(steer_i18n::t("project-root-set-primary"))
                    .into_any_element(),
            )
        })
        .child({
            let path = row.path.clone();
            let on_remove = dialog.on_remove_root.clone();
            div()
                .id(SharedString::from(format!(
                    "project-root-remove-{}",
                    row.path
                )))
                .on_click(move |_, w, cx| {
                    cx.stop_propagation();
                    on_remove(path.as_str(), w, cx);
                })
                .p(px(2.))
                .rounded(px(3.))
                .text_color(FG_FAINT)
                .hover(|style| style.text_color(ERR_RED))
                .child(icon(icons::CLOSE, 12.))
        })
}

/// The ＋ 添加文件夹… row: the host's native multi-select picker opener.
fn add_folder_row(dialog: &ProjectDialog) -> impl IntoElement {
    let on_add = dialog.on_add_folder.clone();
    div()
        .id("project-root-add")
        .on_click(move |e, w, cx| {
            cx.stop_propagation();
            on_add(e, w, cx);
        })
        .h(px(ROOT_ROW_H))
        .px(px(6.))
        .rounded(px(4.))
        .flex()
        .items_center()
        .gap(px(6.))
        .text_color(FG_DIM)
        .cursor_pointer()
        .hover(|style| style.bg(LIST_HOVER).text_color(FG))
        .child(icon(icons::ADD, 14.))
        .child(steer_i18n::t("project-root-add"))
}

/// The action row: the edit mode's remove row on the left, cancel/confirm on
/// the right (the accent pill carries the mode label).
fn action_row(dialog: &ProjectDialog, confirm_key: &'static str) -> impl IntoElement {
    let on_cancel = dialog.on_cancel.clone();
    let on_confirm = dialog.on_confirm.clone();
    let confirm_disabled = dialog.confirm_disabled;
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(6.))
        .children(dialog.on_remove.clone().map(|on_remove| {
            div()
                .id("project-dialog-remove")
                .on_click(move |e, w, cx| {
                    cx.stop_propagation();
                    on_remove(e, w, cx);
                })
                .px(px(8.))
                .py(px(2.))
                .rounded(px(6.))
                .text_size(px(12.))
                .text_color(ERR_RED)
                .cursor_pointer()
                .hover(|style| style.bg(LIST_HOVER))
                .child(steer_i18n::t("project-remove"))
                .into_any_element()
        }))
        .child(div().flex_1())
        .child(
            div()
                .id("project-dialog-cancel")
                .on_click(move |_, w, cx| {
                    cx.stop_propagation();
                    on_cancel(w, cx);
                })
                .px(px(8.))
                .py(px(2.))
                .rounded(px(6.))
                .text_size(px(12.))
                .text_color(FG_DIM)
                .cursor_pointer()
                .hover(|style| style.bg(LIST_HOVER).text_color(FG))
                .child(steer_i18n::t("project-cancel")),
        )
        .child(
            div()
                .id("project-dialog-confirm")
                .map(|this| {
                    if confirm_disabled {
                        // No roots, no save: keep the pill's shape, drop the
                        // affordance (no click, no hover wash).
                        this.opacity(0.4)
                    } else {
                        this
                    }
                })
                .when(!confirm_disabled, |this| {
                    this.on_click(move |e, w, cx| {
                        cx.stop_propagation();
                        on_confirm(e, w, cx);
                    })
                    .cursor_pointer()
                    .hover(|style| style.bg(ACCENT_HOVER))
                })
                .px(px(10.))
                .py(px(3.))
                .rounded(px(6.))
                .text_size(px(12.))
                .bg(ACCENT)
                .text_color(SHELL_BG)
                .child(steer_i18n::t(confirm_key)),
        )
}
