//! The launcher — the new-conversation page's chrome face (design §3.1): the
//! heading, the project chip (the target's display, menu mounted by the
//! host), the light composer (model pick + access switch, no attachments and
//! no slash — those live on the attached session's composer), and the
//! direct-open action row. A props-driven component like
//! [`crate::project_dialog`]: layout and interaction only — the target
//! registry, the pick state, the popups' content and every gesture (send /
//! agent / terminal) are host work wired through the callbacks.

use std::rc::Rc;

use gpui::{
    App, Entity, InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Window, deferred, div, px,
};
use gpui_component::Sizable as _;
use gpui_component::input::{Textarea, TextareaState};
use gpui_component::menu::PopupMenu;

use crate::session_list::{OnUnit, OnWindowApp};
use crate::theme::{
    ACCENT, BORDER, CARD_BG, CARD_RADIUS, FG, FG_DIM, FG_FAINT, FONT_MONO, FONT_UI, LIST_HOVER,
    icon, icons,
};

/// The page card's content column: wide enough for the composer's picks row,
/// narrow enough to read as a focused launch surface.
const CARD_W: f32 = 620.;
const ACTION_H: f32 = 30.;

/// An access pick: `true` = full access, `false` = sandbox.
pub type OnAccessPick = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// One direct-open action (the row's pill): a Ghostty terminal or an external
/// CLI agent. `enabled: false` renders inert with the `unavailable` tooltip
/// (the host detects the CLI's presence; the chrome renders the verdict).
/// `cascade` carries the host-built model popup ONLY on the pill it anchors
/// (the host's open-state bookkeeping); `None` mounts nothing.
pub struct LauncherAction {
    pub id: &'static str,
    pub icon: crate::theme::IconAsset,
    pub label: SharedString,
    pub enabled: bool,
    /// The disabled state's tooltip (the localized "not detected" line).
    pub unavailable: Option<SharedString>,
    pub cascade: Option<Entity<PopupMenu>>,
    pub on_click: OnUnit,
}

/// The launcher page (the pre-container launch pad). `project_label` is the
/// target's display (host-resolved: a project's display name, an ad-hoc
/// folder's basename, or the localized none line). The popups ride the host's
/// own menu entities — the chrome only anchors them.
#[derive(IntoElement)]
pub struct Launcher {
    pub project_label: String,
    pub project_menu_open: bool,
    pub project_menu: Option<Entity<PopupMenu>>,
    /// The composer input (host-owned multi-line entity; ⏎ rides the
    /// host's PressEnter subscription — the light composer's submit is the
    /// Enter gesture, no send pill).
    pub input: Entity<TextareaState>,
    /// The model pick's display (`provider · model · effort`, the composer
    /// chip's own shape) and its popup slot.
    pub model_label: String,
    pub model_menu_open: bool,
    pub model_menu: Option<Entity<PopupMenu>>,
    /// The access switch's current face (`true` = full access).
    pub access_full: bool,
    pub on_project_chip: OnWindowApp,
    pub on_model_pick: OnWindowApp,
    pub on_access_pick: OnAccessPick,
    pub actions: Vec<LauncherAction>,
}

impl RenderOnce for Launcher {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div().size_full().flex().justify_center().child(
            div()
                .w(px(CARD_W))
                .flex()
                .flex_col()
                .justify_center()
                .gap(px(18.))
                .text_size(px(12.5))
                .text_color(FG)
                // Vertically centered like the hero, pulled up one notch so
                // the action row keeps optical weight in the lower half.
                .pb(px(48.))
                .child(
                    div()
                        .text_base()
                        .font_weight(gpui::FontWeight::BLACK)
                        .text_center()
                        .child(steer_i18n::t("launcher-heading")),
                )
                .child(project_chip(&self))
                .child(composer(&self))
                .child(action_heading())
                .child(action_row(&self)),
        )
    }
}

/// The project chip: the target's icon + display + the chevron, an anchored
/// host-menu slot below it.
fn project_chip(launcher: &Launcher) -> impl IntoElement {
    let on_chip = launcher.on_project_chip.clone();
    div()
        .flex()
        .justify_center()
        .relative()
        .child(
            div()
                .id("launcher-project-chip")
                .on_click(move |_, w, cx| {
                    cx.stop_propagation();
                    on_chip(w, cx);
                })
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(10.))
                .py(px(5.))
                .rounded(px(CARD_RADIUS))
                .border_1()
                .border_color(BORDER)
                .bg(CARD_BG)
                .cursor_pointer()
                .hover(|style| style.bg(LIST_HOVER))
                .child(icon(icons::FOLDER, 13.).text_color(FG_DIM))
                .child(
                    div()
                        .max_w(px(280.))
                        .truncate()
                        .text_size(px(12.))
                        .child(launcher.project_label.clone()),
                )
                .child(icon(icons::CHEVRON_DOWN, 12.).text_color(FG_FAINT)),
        )
        .children(
            launcher
                .project_menu_open
                .then(|| launcher.project_menu.clone())
                .flatten()
                .map(anchored_below),
        )
}

/// The light composer: the input plus its picks row (model chip with the
/// effort knob folded into its label, access segmented switch) — the attached
/// composer's footer cluster, minus everything that needs a live session.
fn composer(launcher: &Launcher) -> impl IntoElement {
    let on_model = launcher.on_model_pick.clone();
    div()
        .relative()
        .child(
            div()
                .border_1()
                .border_color(BORDER)
                .rounded(px(CARD_RADIUS))
                .bg(CARD_BG)
                .px(px(10.))
                .py(px(6.))
                .flex()
                .flex_col()
                .gap(px(4.))
                // Message-body typeface for the input, the hero composer's
                // own pairing (family + weight ride the wrapper context).
                .font_family(FONT_MONO)
                .font_weight(gpui::FontWeight::LIGHT)
                .child(
                    div().min_h(px(64.)).child(
                        Textarea::new(&launcher.input)
                            .appearance(false)
                            .h_full()
                            .w_full(),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .font_family(FONT_UI)
                        // The model chip is the row's only variable-width
                        // label: it shrinks and ellipsizes before the row
                        // overflows, the switch stays pinned.
                        .child(
                            div()
                                .id("launcher-model-chip")
                                .on_click(move |_, w, cx| {
                                    cx.stop_propagation();
                                    on_model(w, cx);
                                })
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .min_w_0()
                                .px(px(6.))
                                .py(px(2.))
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(|style| style.bg(LIST_HOVER))
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(11.5))
                                        .text_color(FG_DIM)
                                        .child(launcher.model_label.clone()),
                                )
                                .child(icon(icons::CHEVRON_DOWN, 11.).text_color(FG_FAINT)),
                        )
                        .child(div().flex_1())
                        .child(access_switch(
                            launcher.access_full,
                            launcher.on_access_pick.clone(),
                        )),
                ),
        )
        // The model cascade anchors above the card (a footer cluster opens
        // upward, the same face the composer's own selector keeps).
        .children(
            launcher
                .model_menu_open
                .then(|| launcher.model_menu.clone())
                .flatten()
                .map(anchored_above),
        )
}

/// The access switch: the composer footer's segmented two-tier control, one
/// click per tier, no popover.
fn access_switch(full: bool, on_pick: OnAccessPick) -> impl IntoElement {
    let selected_ix = usize::from(full);
    gpui_component::h_flex().flex_shrink_0().child(
        gpui_component::tab::TabBar::new("launcher-access-switch")
            .segmented()
            .small()
            .selected_index(selected_ix)
            .on_click(move |ix, window, cx| {
                on_pick(*ix == 1, window, cx);
            })
            .child(
                gpui_component::tab::Tab::new().label(steer_i18n::t("access-mode-sandbox-title")),
            )
            .child(
                gpui_component::tab::Tab::new()
                    .label(steer_i18n::t("access-mode-full-access-title")),
            ),
    )
}

/// The action row's lead-in: a centered hairline + caption divider.
fn action_heading() -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(div().flex_1().h(px(1.)).bg(BORDER))
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(11.))
                .text_color(FG_FAINT)
                .child(steer_i18n::t("launcher-action-heading")),
        )
        .child(div().flex_1().h(px(1.)).bg(BORDER))
}

/// The direct-open action row: one pill per action, wrapping — the terminal
/// and the three CLI agents read as one band on a 620px card.
fn action_row(launcher: &Launcher) -> impl IntoElement {
    div()
        .flex()
        .flex_wrap()
        .justify_center()
        .gap(px(8.))
        .children(launcher.actions.iter().map(action_pill))
}

/// One action pill: the brand glyph + the label. A disabled pill keeps its
/// shape, drops the affordance and carries the unavailable tooltip; an
/// enabled pill anchors its host-built cascade below itself while it carries
/// one.
fn action_pill(action: &LauncherAction) -> impl IntoElement {
    let on_click = action.on_click.clone();
    let base = div()
        .id(SharedString::from(format!("launcher-action-{}", action.id)))
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(ACTION_H))
        .px(px(12.))
        .rounded(px(CARD_RADIUS))
        .border_1()
        .border_color(BORDER)
        .bg(CARD_BG)
        .text_size(px(12.))
        .child(
            gpui_component::Icon::default()
                .path(action.icon.0)
                .size(px(13.)),
        )
        .child(div().child(action.label.clone()));
    let pill = if action.enabled {
        base.cursor_pointer()
            .hover(|style| style.bg(LIST_HOVER).border_color(ACCENT))
            .on_click(move |e, w, cx| {
                cx.stop_propagation();
                on_click(e, w, cx);
            })
    } else {
        let unavailable = action
            .unavailable
            .clone()
            .unwrap_or_else(|| SharedString::from(""));
        base.opacity(0.4).tooltip(move |window, cx| {
            gpui_component::tooltip::Tooltip::new(unavailable.clone()).build(window, cx)
        })
    };
    div()
        .relative()
        .child(pill)
        .children(action.cascade.clone().map(anchored_below))
}

/// A deferred popup slot below the anchor; the click guard keeps the menu's
/// own mouse-downs from dismissing it through the window layer.
fn anchored_below(menu: Entity<PopupMenu>) -> impl IntoElement {
    deferred(
        div()
            .absolute()
            .top_full()
            .mt(px(4.))
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(menu),
    )
    .with_priority(1)
}

/// A deferred popup slot above the anchor (footer-cluster popups open
/// upward).
fn anchored_above(menu: Entity<PopupMenu>) -> impl IntoElement {
    deferred(
        div()
            .absolute()
            .bottom_full()
            .mb(px(4.))
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(menu),
    )
    .with_priority(1)
}
