//! The sidebar session tree — SessionList (fixed rows + workspace groups +
//! session rows + the bottom Customizations block).
//!
//! Pixel-calibrated against the agents-window reference: every session row is a
//! fixed-height 66px three-line card — title (16px status slot + 6px gap) /
//! tag line (short-id chip + user chip) / info line (last-active time) — all
//! three lines sharing one left baseline with no right-aligned content and no
//! per-row indentation (hierarchy is expressed by the group header alone).
//! All row actions (pin / archive / tags / copy id) live exclusively in the
//! right-click menu ([`SessionList::on_row_menu`]); the row surface itself
//! carries no controls. Header overlap rule: the Sessions title fills the
//! width underneath, the right-side controls (New / sort / search) float
//! above it with an opaque background — a too-narrow sidebar occludes the
//! title rather than squeezing the controls.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use crate::theme::{
    ACCENT, BADGE_BLUE_BG, BADGE_BLUE_FG, BORDER, CARD_BG, CARD_BORDER, ERR_RED, FG, FG_DIM,
    FG_FAINT, FG_STRONG, FONT_UI, IconAsset, LIST_HOVER, SHELL_BG, icon, icons,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnimationExt, App, ClickEvent, Entity, FocusHandle, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, Pixels, RenderOnce, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, div, linear_color_stop, linear_gradient, px,
};
use gpui_component::input::{Input, InputState};
use gpui_component::{ElementExt as _, Sizable as _};

use crate::primitives::{IconButtonState, icon_button, kbd_chip, small_icon_button};

/// The sidebar's two grouping modes: by workspace (project) — the default,
/// drag-reorderable — or by last-activity time buckets (today / yesterday /
/// last 7 days / earlier).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SidebarGrouping {
    #[default]
    Workspace,
    Time,
}

impl SidebarGrouping {
    pub fn is_time(self) -> bool {
        self == Self::Time
    }
}

/// Action callback taking a row id (select / toggle group / menu target).
pub type OnId = Rc<dyn Fn(&String, &mut Window, &mut App)>;
/// Action callback with no payload (New).
pub type OnUnit = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
/// Row-menu callback (id + open position, right-click).
pub type OnRowMenu = Rc<dyn Fn(&String, gpui::Point<Pixels>, &mut Window, &mut App)>;
/// Hover-enter/leave callback (row id, entered?).
pub type OnHover = Rc<dyn Fn(&String, bool, &mut Window, &mut App)>;
/// Plain window callback (tag-edit cancel on Escape).
pub type OnWindowApp = Rc<dyn Fn(&mut Window, &mut App)>;
/// Drag-over-group callback updating the drop marker (dragged, target, before-half?).
pub type OnGroupDragMove = Rc<dyn Fn(&String, &String, bool, &mut Window, &mut App)>;
/// Drop-commit callback for group reordering (dragged, target, before-half?).
pub type OnGroupMove = Rc<dyn Fn(&String, &String, bool, &mut Window, &mut App)>;
/// Group-menu callback (state key, the group's project path when it is a
/// workspace group, open position). The HOST builds the menu — the project
/// actions (launch agents / editor, remove project) are host semantics the
/// chrome carries no opinion on; an absent callback leaves the header with
/// no menu surface.
pub type OnGroupMenu = Rc<dyn Fn(&str, Option<&str>, gpui::Point<Pixels>, &mut Window, &mut App)>;

/// One session row as projected by the host. The chrome owns only the
/// interaction state around these (selection, collapse, order); it never
/// fetches or interprets sessions itself.
#[derive(Clone, PartialEq)]
pub struct SessionRowData {
    pub id: String,
    /// The id chip's text — the host-supplied short form (a thread's uuid
    /// prefix, an external row's uuid segment), uniform 8 chars across row
    /// kinds. The chip's CLICK still copies the full id.
    pub short_id: String,
    pub title: String,
    /// Last-active unix seconds — the info line's display source.
    pub updated_at: i64,
    /// The re-sort stamp: the row's own `updated_at`, except team members,
    /// which the projection stamps with their leader's — a team sorts (and
    /// survives a pin re-order) as one unit, contiguous under its chevron.
    pub sort_stamp: i64,
    pub status: SessionStatus,
    /// What the row IS — the leading slot's glyph and the row menu's
    /// semantics ride this (the legacy sidebar's RowIcon/RowKind pair,
    /// unified: threads show the five-state machine, live non-thread
    /// sessions show their brand mark and close instead of archive).
    pub kind: SessionRowKind,
    pub pinned: bool,
    /// The store-partition flag behind the menu's archive/unarchive toggle.
    /// Premise: the wire snapshot rides the ACTIVE partition only, so this
    /// reads false on every row the shell is fed today — the unarchive half
    /// is for the archived-partition rows the wire will grow; until then the
    /// menu always offers archive.
    pub archived: bool,
    /// The persisted user tag chip on the tag line.
    pub tag: Option<String>,
    /// A team leader renders its collapse chevron before the status slot
    /// (the row itself does not indent — nesting shows via the chevron only).
    pub team_leader: bool,
}

/// The row's five-state machine (parity with the agent-ui sidebar). States
/// where a turn stopped and waits for a human (`PendingAuth`/`PendingPlan`)
/// render the filled blue dot; `Unread` the hollow one; `Running` the pixel
/// grid; `Errored` the danger triangle; `Idle` the empty slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SessionStatus {
    Idle,
    Running,
    Unread,
    PendingAuth,
    PendingPlan,
    Errored,
}

/// What a session row is: the legacy sidebar's unified item abstraction —
/// manox threads and live non-thread sessions (launched CLI agents, plain
/// terminals) render through ONE row component whose leading slot and menu
/// semantics branch on this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SessionRowKind {
    /// A manox thread: the five-state status glyph leads; the row menu is
    /// the thread's (pin / archive / tag / copy id).
    #[default]
    Thread,
    /// A live non-thread session: its brand mark leads (the five-state
    /// machine does not apply to a PTY); the row menu closes the session.
    /// `icon` is the host-supplied brand asset path.
    External { icon: IconAsset },
}

/// One workspace group.
#[derive(Clone, PartialEq)]
pub struct SessionGroup {
    /// Display name (the header label).
    pub name: String,
    /// Stable state key — collapse state and drag identity ride THIS, never
    /// the display name (which follows the UI language in time grouping and
    /// must not become state).
    pub key: String,
    pub collapsed: bool,
    /// The group's project directory, host-supplied — the project menu's
    /// launch target. `None` on time buckets and on the no-project bucket;
    /// the menu still opens there, scoped to the host's fallback cwd, minus
    /// the remove-project row.
    pub project: Option<String>,
    pub rows: Vec<SessionRowData>,
}

/// A fixed sidebar row (Automations / Chats …) — labels come from the host.
#[derive(Clone, PartialEq)]
pub struct FixedRow {
    pub icon: IconAsset,
    pub label: String,
    pub badge: Option<String>,
}

/// A Customizations block row.
#[derive(Clone, PartialEq)]
pub struct CustomizationRow {
    pub icon: IconAsset,
    pub label: String,
    pub count: Option<u32>,
}

// ---- thread-item geometry -------------------------------------------------

/// Uniform three-line row height (identical across the four states so a state
/// change never reflows the list). 66 = pad_y(5) + 20 + 2 + 17 + 2 + 15 + 5.
const ROW_H: f32 = 66.;
const PAD_X: f32 = 8.;
/// The status slot every state reserves (16px, even when visually empty).
const LEAD: f32 = 16.;
/// Status-slot → text gap; title / tags / info share the resulting baseline.
const GAP: f32 = 6.;
/// Team-leader chevron slot (icon + margin) prepended to the baseline.
const CHEVRON_SLOT: f32 = 15.;
const PAD_Y: f32 = 5.;
const L1_H: f32 = 20.;
const L2_H: f32 = 17.;
const L3_H: f32 = 15.;
const LINE_GAP: f32 = 2.;
const ROW_R: f32 = 6.;
const TITLE_SIZE: f32 = 12.5;

/// Marquee parameters: 24px/s, a 600ms hold once per cycle, a 24px gap
/// separating the two track copies; runs
/// only while hovered and the title is actually truncated. The track is
/// [copy][gap][copy] and every cycle scrolls exactly one period, so the
/// wrap-around lands on pixel-identical content — a seamless loop.
const MARQUEE_SPEED: f32 = 24.;
const MARQUEE_PAUSE: f32 = 0.6;
const MARQUEE_GAP: f32 = 24.;
const MARQUEE_FADE_W: f32 = 14.;

/// The running pixel grid: a 2×3 dot matrix, 2px dots, 2px gaps, one
/// 1820ms stepped cycle; the long/short variants stretch or shrink the
/// middle hold so the cascade reads as a wave.
const PIXEL_GRID_MS: f32 = 1820.;
/// Per-dot keyframe variants: dots 1-4 the standard cycle, dot 5 long, dot 6
/// short (delays in ms — the calibration's values).
const PIXEL_DOTS: [(f32, SpinVariant); 6] = [
    (520., SpinVariant::Cycle),
    (650., SpinVariant::Cycle),
    (260., SpinVariant::Cycle),
    (390., SpinVariant::Cycle),
    (0., SpinVariant::Long),
    (130., SpinVariant::Short),
];

/// Which keyframe set a pixel-grid dot follows (the long/short variants
/// stretch or shrink the middle hold so the cascade reads as a wave).
#[derive(Clone, Copy)]
enum SpinVariant {
    Cycle,
    Long,
    Short,
}

/// SessionList: props-driven; interactions surface through callbacks, never
/// through global state.
#[derive(IntoElement)]
pub struct SessionList {
    pub fixed_rows: Vec<FixedRow>,
    pub groups: Vec<SessionGroup>,
    pub customizations: Vec<CustomizationRow>,
    pub selected: Option<String>,
    /// Show the "No chats" empty hint under the Chats row.
    pub no_chats_hint: bool,
    /// The hovered row id (hover = 500 title weight + marquee trigger).
    pub hovered: Option<String>,
    /// The inline tag editor (row id + input), mounted on that row's tag
    /// line. At most one edit in flight.
    pub tag_edit: Option<(String, Entity<InputState>)>,
    /// The sidebar grouping mode (the sort button's state).
    pub grouping: SidebarGrouping,
    /// The sort button: workspace grouping ↔ time buckets.
    pub on_toggle_grouping: OnUnit,
    /// The search button: expand/collapse the filter row.
    pub on_toggle_search: OnUnit,
    /// Whether the filter input row is expanded.
    pub filter_open: bool,
    /// Whether a filter term is active (non-empty) — with zero surviving
    /// groups the list shows the no-match hint.
    pub filter_active: bool,
    /// The filter input (mounted inside the filter row while open).
    pub filter_input: Option<Entity<InputState>>,
    /// The filter row's clear (×) control: collapse and reset.
    pub on_filter_clear: Option<OnWindowApp>,
    /// Per-row title clip-box width, written unguarded by each row's
    /// `on_prepaint` every frame; the marquee reads the last painted value.
    pub title_box_w: Rc<RefCell<HashMap<String, Pixels>>>,
    /// Per-row focus handles: `track_focus` + up/down row navigation.
    pub row_focus: Rc<HashMap<String, FocusHandle>>,
    pub on_select: OnId,
    pub on_toggle_group: OnId,
    pub on_new: OnUnit,
    pub on_hover_row: OnHover,
    /// Row menu (right-click anywhere on the row): the only action surface.
    pub on_row_menu: OnRowMenu,
    /// Group-menu surface (the header's ellipsis button + right-click): the
    /// host builds the menu. Workspace grouping only — a time bucket is not
    /// a launch target.
    pub on_group_menu: Option<OnGroupMenu>,
    /// Escape inside the inline tag editor.
    pub on_tag_edit_cancel: Option<OnWindowApp>,
    /// Double-click on a row's user tag chip: begin the RENAME editor (the
    /// old shell's chip double-click semantics).
    pub on_tag_rename: OnId,
    /// Group reorder commit (dragged → before/after edge of target).
    pub on_move_group: OnGroupMove,
    /// Drag-over-group: update the drop marker (insertion-line position).
    pub on_drag_move_group: OnGroupDragMove,
    /// Current drop marker (dragged, target, before?); drives the 2px accent
    /// insertion line.
    pub group_drag_marker: Option<(String, String, bool)>,
}

impl RenderOnce for SessionList {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        // Flat visible-row order for the up/down focus walk (collapsed
        // groups contribute nothing).
        let order: Vec<String> = self
            .groups
            .iter()
            .flat_map(|g| (!g.collapsed).then(|| g.rows.iter().map(|r| r.id.clone())))
            .flatten()
            .collect();

        div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .py(px(4.))
            .px(px(8.))
            .gap(px(1.))
            .text_color(FG)
            .child(header(
                self.on_new.clone(),
                self.grouping,
                self.on_toggle_grouping.clone(),
                self.on_toggle_search.clone(),
                self.filter_open,
            ))
            .when(self.filter_open, |this| {
                this.child(filter_row(
                    self.filter_input.clone(),
                    self.on_filter_clear.clone(),
                ))
            })
            .child(
                div()
                    .id("sessions-scroll")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .children(self.fixed_rows.iter().map(fixed_row))
                    .when(self.no_chats_hint, |this| {
                        // The hint sits directly under the Chats row.
                        this.child(
                            div()
                                .mx(px(20.))
                                .py(px(3.))
                                .px(px(6.))
                                .text_size(px(12.))
                                .text_color(FG_FAINT)
                                .child(steer_i18n::t("chrome-no-chats")),
                        )
                    })
                    .children(self.groups.iter().map(|g| group(&self, g, &order, window)))
                    // A filter active on a fully-filtered-out list: an empty
                    // state of its own, distinct from "no chats yet".
                    .when(
                        self.filter_active && !self.no_chats_hint && self.groups.is_empty(),
                        |this| {
                            this.child(
                                div()
                                    .mx(px(20.))
                                    .py(px(3.))
                                    .px(px(6.))
                                    .text_size(px(12.))
                                    .text_color(FG_FAINT)
                                    .child(steer_i18n::t("chrome-sidebar-no-match")),
                            )
                        },
                    ),
            )
            .child(customizations(&self.customizations))
    }
}

/// A localized hover tooltip view (gpui-component Tooltip over the Root).
fn hover_tooltip(text: &'static str) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView + 'static {
    move |window, cx| gpui_component::tooltip::Tooltip::new(steer_i18n::t(text)).build(window, cx)
}

/// Header: the Sessions title + New button (⌘N kbd chip) + sort/search.
/// The sort button toggles workspace ↔ time grouping (lit in time mode); the
/// search button expands the filter row (lit while open). Overlap rule: the
/// title layer fills the width underneath, the controls float above it with
/// an opaque background — a narrow sidebar occludes the title (the reference
/// screenshot's "Se…" truncation) instead of squeezing the controls.
fn header(
    on_new: OnUnit,
    grouping: SidebarGrouping,
    on_toggle_grouping: OnUnit,
    on_toggle_search: OnUnit,
    filter_open: bool,
) -> impl IntoElement {
    div()
        .relative()
        .w_full()
        .h(px(32.))
        .flex_shrink_0()
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .h_full()
                .w_full()
                .flex()
                .items_center()
                .px(px(8.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(FG_STRONG)
                        .child(steer_i18n::t("chrome-sessions-title")),
                ),
        )
        .child(
            div()
                .absolute()
                .top(px(3.))
                .right_0()
                .h(px(29.))
                .flex()
                .items_center()
                .gap(px(4.))
                .pl(px(8.))
                .bg(CARD_BG)
                .child(
                    div()
                        .id("new-session")
                        .on_click(move |e, w, cx| on_new(e, w, cx))
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .py(px(2.))
                        .px(px(6.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(BORDER)
                        .text_size(px(12.))
                        .text_color(FG)
                        .hover(|style| style.bg(LIST_HOVER))
                        .child(steer_i18n::t("chrome-new"))
                        .child(kbd_chip("⌘N")),
                )
                .child(
                    icon_button(
                        "sort",
                        icons::SORT_PRECEDENCE,
                        14.,
                        if grouping.is_time() {
                            IconButtonState::On
                        } else {
                            IconButtonState::Off
                        },
                        move |e, w, cx| on_toggle_grouping(e, w, cx),
                    )
                    .tooltip(hover_tooltip("chrome-sidebar-grouping")),
                )
                .child(
                    icon_button(
                        "search",
                        icons::SEARCH,
                        14.,
                        if filter_open {
                            IconButtonState::On
                        } else {
                            IconButtonState::Off
                        },
                        move |e, w, cx| on_toggle_search(e, w, cx),
                    )
                    .tooltip(hover_tooltip("chrome-sidebar-search")),
                ),
        )
}

/// The expanded filter row (under the header): search glyph + input + clear.
/// Typing narrows the list live; × collapses the row and resets the term.
fn filter_row(
    filter_input: Option<Entity<InputState>>,
    on_clear: Option<OnWindowApp>,
) -> impl IntoElement {
    div()
        .id("sidebar-filter")
        .w_full()
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(26.))
        .px(px(6.))
        .rounded(px(5.))
        .border_1()
        .border_color(BORDER)
        .bg(CARD_BG)
        .flex_shrink_0()
        .text_color(FG_DIM)
        .child(icon(icons::SEARCH, 13.))
        .child(div().flex_1().min_w_0().children(filter_input.map(|input| {
            Input::new(&input)
                .appearance(false)
                .h_full()
                .w_full()
                .text_size(px(12.))
        })))
        .children(on_clear.map(|cb| {
            small_icon_button(
                "sidebar-filter-clear",
                icons::CLOSE,
                10.,
                FG_FAINT,
                FG,
                move |_, w, cx| cb(w, cx),
            )
        }))
}

/// Fixed row: icon + label + badge. Not implemented yet — the row renders
/// dimmed and says so on hover; it stays inert until a real surface exists.
fn fixed_row(row: &FixedRow) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("fixed-{}", row.label)))
        .w_full()
        .flex()
        .items_center()
        .gap(px(8.))
        .py(px(5.))
        .px(px(8.))
        .rounded(px(4.))
        .flex_shrink_0()
        .text_color(FG_FAINT)
        .tooltip(hover_tooltip("chrome-unimplemented"))
        .child(icon(row.icon, 15.))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_size(px(12.5))
                .child(row.label.clone()),
        )
        .children(row.badge.clone().map(badge))
}

fn badge(label: String) -> impl IntoElement {
    div()
        .py(px(1.))
        .px(px(4.))
        .rounded(px(4.))
        .bg(BADGE_BLUE_BG)
        .text_color(BADGE_BLUE_FG)
        .text_size(px(9.))
        .flex_shrink_0()
        .child(label)
}

/// Group drag payload (the group name); the ghost is a name pill.
#[derive(Clone)]
pub struct DraggedGroup(pub SharedString);

impl gpui::Render for DraggedGroup {
    fn render(&mut self, _w: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .px(px(6.))
            .py(px(2.))
            .rounded(px(4.))
            .bg(CARD_BG)
            .border_1()
            .border_color(BORDER)
            .text_size(px(12.))
            .child(self.0.clone())
    }
}

/// Group header (chevron + folder + name, collapsible; drag-reorder source) +
/// its rows. The drop target and insertion line hang on the **group
/// container** (the whole group is droppable); the insert position comes from
/// the header's drag-move handler reading which half of the target header the
/// pointer is in. gpui dispatches drag-move/drop by payload type and fires
/// every registered listener of that type, so each header checks its own
/// bounds and skips when the pointer is not over it — only the hit group
/// updates the marker. The header keeps its own padding — it is the one
/// element that still expresses project hierarchy; session rows run full
/// width beneath it.
fn group(
    list: &SessionList,
    g: &SessionGroup,
    order: &[String],
    window: &Window,
) -> impl IntoElement {
    let toggle = list.on_toggle_group.clone();
    let key = g.key.clone();
    let on_move = list.on_move_group.clone();
    let on_drag_move_cb = list.on_drag_move_group.clone();
    let marker = list.group_drag_marker.clone();
    // Drag reorder is a workspace-mode concept: in time grouping the order
    // is the recency sort, so the group header is not a drag source and the
    // container is not a drop target — a live insertion line whose commit
    // is silently discarded would be a fake control.
    let draggable = !list.grouping.is_time();

    let header_key = g.key.clone();
    let bounds_key = g.key.clone();
    let mut header = div()
        .id(SharedString::from(format!("grp-{}", g.key)))
        .debug_selector(move || format!("chrome-group-header-{}", bounds_key))
        .on_click(move |_, w, cx| toggle(&key, w, cx));
    if draggable {
        header = header
            .on_drag(DraggedGroup(SharedString::from(header_key.as_str())), {
                let payload = DraggedGroup(SharedString::from(header_key.as_str()));
                move |_, _, _, cx| {
                    use gpui::AppContext as _;
                    cx.stop_propagation();
                    let p = payload.clone();
                    cx.new(move |_| p.clone())
                }
            })
            .on_drag_move::<DraggedGroup>({
                let target = g.key.clone();
                let cb = on_drag_move_cb.clone();
                move |e: &gpui::DragMoveEvent<DraggedGroup>, w, cx| {
                    // Update the marker only while the pointer is inside this
                    // group header's vertical span.
                    if e.event.position.y < e.bounds.origin.y
                        || e.event.position.y > e.bounds.origin.y + e.bounds.size.height
                    {
                        return;
                    }
                    let before = e.event.position.y < e.bounds.origin.y + e.bounds.size.height / 2.;
                    (cb)(&e.drag(cx).0.to_string(), &target, before, w, cx);
                }
            });
    }
    // The project menu surface rides the header in workspace grouping: the
    // ellipsis button and a right-click anywhere on the header both hand the
    // host (key, project, open position) to build the menu from.
    if draggable && let Some(on_group_menu) = list.on_group_menu.clone() {
        let menu_key = g.key.clone();
        let menu_project = g.project.clone();
        header = header.on_mouse_down(gpui::MouseButton::Right, move |e, w, cx| {
            cx.stop_propagation();
            (on_group_menu)(&menu_key, menu_project.as_deref(), e.position, w, cx);
        });
    }
    let header = header
        .w_full()
        .flex()
        .items_center()
        .gap(px(5.))
        .py(px(4.))
        .px(px(6.))
        .text_size(px(12.5))
        .hover(|style| style.bg(LIST_HOVER))
        .child(icon(
            if g.collapsed {
                icons::CHEVRON_RIGHT
            } else {
                icons::CHEVRON_DOWN
            },
            13.,
        ))
        .child(icon(icons::FOLDER, 15.))
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .min_w_0()
                .truncate()
                .child(g.name.clone()),
        )
        .child(div().flex_1())
        .when_some(
            list.on_group_menu.clone().filter(|_| draggable),
            |el, on_menu| {
                let btn_key = g.key.clone();
                let btn_selector = g.key.clone();
                let btn_project = g.project.clone();
                el.child(
                    div()
                        .id(SharedString::from(format!("grp-menu-{}", g.key)))
                        .debug_selector(move || format!("chrome-group-menu-btn-{}", btn_selector))
                        .on_click(move |e, w, cx| {
                            cx.stop_propagation();
                            (on_menu)(&btn_key, btn_project.as_deref(), e.position(), w, cx);
                        })
                        .px(px(2.))
                        .rounded(px(3.))
                        .text_color(FG_FAINT)
                        .hover(|style| style.text_color(FG).bg(LIST_HOVER))
                        .child(icon(icons::MORE, 13.)),
                )
            },
        );

    let mut items: Vec<gpui::AnyElement> = vec![header.into_any_element()];
    if !g.collapsed {
        items.extend(
            g.rows
                .iter()
                .map(|r| session_row(list, r, order, window).into_any_element()),
        );
    }

    let mut slot = div()
        .id(SharedString::from(format!("grp-slot-{}", g.key)))
        .w_full()
        .flex()
        .flex_col()
        .relative();
    if draggable {
        slot = slot.on_drop::<DraggedGroup>({
            let target = g.key.clone();
            let on_move = on_move.clone();
            let marker = marker.clone();
            move |dragged: &DraggedGroup, w, cx| {
                if dragged.0.as_ref() == target.as_str() {
                    return;
                }
                // Commit at the insertion edge last recorded by drag-move
                // (defaulting to the leading edge).
                let before = marker
                    .as_ref()
                    .filter(|(_, t, _)| t == &target)
                    .map(|(_, _, b)| *b)
                    .unwrap_or(true);
                (on_move)(&dragged.0.to_string(), &target, before, w, cx);
            }
        });
    }
    // The insertion line reads (dragged, target, before); in time grouping
    // the marker stays None because no header is a drag source there.
    let slot = slot.children(marker.clone().and_then(|(dragged, target, before)| {
        (target == g.key && dragged != g.key).then(|| {
            let line = div()
                .absolute()
                .left_0()
                .right_0()
                .h(px(2.))
                .rounded_full()
                .bg(ACCENT);
            if before {
                line.top_0()
            } else {
                line.bottom_0()
            }
        })
    }));
    slot.children(items)
}

fn session_row(
    list: &SessionList,
    data: &SessionRowData,
    order: &[String],
    window: &Window,
) -> Stateful<gpui::Div> {
    let selected = list.selected.as_deref() == Some(data.id.as_str());
    let hovered = list.hovered.as_deref() == Some(data.id.as_str());

    let focus = list.row_focus.get(&data.id);
    let ix = order.iter().position(|id| id == &data.id).unwrap_or(0);
    let neighbor = |at: usize| order.get(at).and_then(|id| list.row_focus.get(id)).cloned();
    let prev_focus = ix.checked_sub(1).and_then(neighbor);
    let next_focus = neighbor(ix + 1);

    // The shared left baseline for the tag + info lines; the title box
    // reaches it via slot + gap. Leader rows prepend the chevron slot.
    let text_indent = LEAD + GAP + if data.team_leader { CHEVRON_SLOT } else { 0. };

    let row = div()
        .id(SharedString::from(format!("sess-{}", data.id)))
        .debug_selector({
            let key = data.id.clone();
            move || format!("chrome-session-row-{key}")
        })
        .on_click({
            let on_select = list.on_select.clone();
            let id = data.id.clone();
            move |_, w, cx| on_select(&id, w, cx)
        })
        .on_mouse_down(gpui::MouseButton::Right, {
            let on_menu = list.on_row_menu.clone();
            let id = data.id.clone();
            move |e, w, cx| {
                cx.stop_propagation();
                (on_menu)(&id, e.position, w, cx);
            }
        })
        .on_hover({
            let on_hover = list.on_hover_row.clone();
            let id = data.id.clone();
            move |entered, w, cx| (on_hover)(&id, *entered, w, cx)
        })
        .when_some(focus.cloned(), |el, fh| {
            el.track_focus(&fh)
                .focus_visible(|style| style.border(px(1.5)).border_color(ACCENT))
                .on_key_down(move |ev: &gpui::KeyDownEvent, w, cx| {
                    match ev.keystroke.key.as_str() {
                        "up" => {
                            if let Some(f) = &prev_focus {
                                w.focus(f, cx);
                                cx.stop_propagation();
                            }
                        }
                        "down" => {
                            if let Some(f) = &next_focus {
                                w.focus(f, cx);
                                cx.stop_propagation();
                            }
                        }
                        _ => {}
                    }
                })
        })
        .w_full()
        .h(px(ROW_H))
        .flex()
        .flex_col()
        .py(px(PAD_Y))
        .px(px(PAD_X))
        .gap(px(LINE_GAP))
        .rounded(px(ROW_R))
        .flex_shrink_0()
        .overflow_hidden()
        // Line 1: status slot + title.
        .child(
            div()
                .h(px(L1_H))
                .flex()
                .items_center()
                .when(data.team_leader, |this| {
                    this.child(
                        div()
                            .mr(px(4.))
                            .flex_shrink_0()
                            .child(icon(icons::CHEVRON_DOWN, 11.)),
                    )
                })
                .child(
                    div()
                        .w(px(LEAD))
                        .h_full()
                        .mr(px(GAP))
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        // The leading slot branches on the row kind: threads
                        // run the five-state machine; a live non-thread
                        // session wears its brand mark (the legacy
                        // RowIcon::External slot).
                        .child(match data.kind {
                            SessionRowKind::Thread => {
                                status_slot(&data.id, data.status).into_any_element()
                            }
                            SessionRowKind::External { icon: brand } => div()
                                .text_color(FG_FAINT)
                                .child(icon(brand, 14.))
                                .into_any_element(),
                        }),
                )
                .child(title_line(list, data, selected, hovered, window)),
        )
        // Line 2: tag line — id chip first, then the user tag (or editor).
        .child(
            div()
                .h(px(L2_H))
                .min_w_0()
                .pl(px(text_indent))
                .flex()
                .items_center()
                .gap(px(4.))
                .child(id_tag(&data.short_id, &data.id))
                .children(match (&list.tag_edit, &data.tag) {
                    (Some((edit_id, input)), _) if edit_id == &data.id => Some(
                        tag_edit_input(input, list.on_tag_edit_cancel.clone()).into_any_element(),
                    ),
                    (_, Some(tag)) => Some(
                        user_tag(&data.id, tag.clone(), list.on_tag_rename.clone())
                            .into_any_element(),
                    ),
                    _ => None,
                }),
        )
        // Line 3: info — last-active time only.
        .child(
            div()
                .h(px(L3_H))
                .min_w_0()
                .pl(px(text_indent))
                .flex()
                .items_center()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(11.))
                        .text_color(FG_FAINT)
                        .child(format_active_time(data.updated_at, now_unix_secs())),
                ),
        );

    // Row surface (selected card / hover / rest) + the 1px stroke kept
    // transparent in the unselected states so all four share one height.
    if selected {
        row.bg(CARD_BG).border_1().border_color(CARD_BORDER)
    } else {
        row.border_1()
            .border_color(gpui::transparent_black())
            .hover(|style| style.bg(LIST_HOVER))
    }
}

/// The user tag chip on the tag line — the same visual language as the
/// short-id chip (outlined mini-pill, truncated at 90px).
fn user_tag(id: &str, tag: String, on_rename: OnId) -> impl IntoElement {
    let id = id.to_string();
    div()
        .id(SharedString::from(format!("usertag-{id}")))
        .on_click(move |ev, w, cx| {
            // Swallow every click (the row's open-click must not fire from
            // chip work) and start the rename editor on the second click of
            // a double-click — the old shell's chip semantics.
            cx.stop_propagation();
            if ev.click_count() >= 2 {
                (on_rename)(&id, w, cx);
            }
        })
        .px(px(4.))
        .rounded(px(3.))
        .border_1()
        .border_color(BORDER)
        .text_size(px(10.))
        .text_color(FG_FAINT)
        .flex_shrink_0()
        .max_w(px(90.))
        .truncate()
        .child(tag)
}

/// The inline tag editor, mounted on the tag line: an invisible-border
/// xsmall input. Clicks on the wrapper never reach the row's open click; the
/// input's propagated `Escape` cancels (Enter/blur commit through the
/// subscription the shell holds).
fn tag_edit_input(input: &Entity<InputState>, on_cancel: Option<OnWindowApp>) -> impl IntoElement {
    div()
        .id("tag-edit")
        .w(px(90.))
        .flex_shrink_0()
        .on_click(|_, _, cx| cx.stop_propagation())
        .when_some(on_cancel, |el, cancel| {
            el.on_action(move |_: &gpui_component::input::Escape, w, cx| {
                cx.stop_propagation();
                cancel(w, cx);
            })
        })
        .child(Input::new(input).appearance(false).xsmall())
}

/// Short-id tag chip (click copies the full id).
fn id_tag(short_id: &str, id: &str) -> impl IntoElement {
    let short = short_id.to_string();
    let full = id.to_string();
    div()
        .id(SharedString::from(format!("tag-{id}")))
        .on_click(move |_e, _w, cx| {
            cx.stop_propagation();
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(full.clone()));
        })
        .px(px(4.))
        .rounded(px(3.))
        .border_1()
        .border_color(BORDER)
        .text_size(px(10.))
        .text_color(FG_FAINT)
        .flex_shrink_0()
        .child(short)
}

/// The 16px status slot's content, one glyph per five-state: parked turns
/// (`PendingAuth`/`PendingPlan`) the filled 8px blue dot, `Unread` the hollow
/// 6.5px one (distinct semantics, same blue), `Running` the pixel grid,
/// `Errored` the danger triangle, `Idle` nothing.
fn status_slot(id: &str, status: SessionStatus) -> impl IntoElement {
    match status {
        SessionStatus::Errored => div()
            .flex()
            .text_color(ERR_RED)
            .child(icon(icons::WARNING, 11.))
            .into_any_element(),
        SessionStatus::PendingAuth | SessionStatus::PendingPlan => div()
            .size(px(8.))
            .rounded_full()
            .bg(BADGE_BLUE_BG)
            .into_any_element(),
        SessionStatus::Running => pixel_grid(id).into_any_element(),
        SessionStatus::Unread => div()
            .size(px(6.5))
            .rounded_full()
            .border(px(1.5))
            .border_color(BADGE_BLUE_BG)
            .into_any_element(),
        SessionStatus::Idle => div().into_any_element(),
    }
}

/// The title line: a clip box whose width is recorded every frame (the
/// marquee's truncation test reads the last painted value). Hovered +
/// truncated → the marquee runs; otherwise a plain truncated title.
fn title_line(
    list: &SessionList,
    data: &SessionRowData,
    selected: bool,
    hovered: bool,
    window: &Window,
) -> gpui::AnyElement {
    let weight = if selected || hovered {
        FontWeight::MEDIUM
    } else {
        FontWeight::NORMAL
    };
    let text_w = natural_title_width(window, &data.title, weight);
    let box_w = list
        .title_box_w
        .borrow()
        .get(&data.id)
        .copied()
        .unwrap_or(px(0.));
    // The same comparison the text element runs before eliding (shape the
    // natural line, compare against the clip width) — see
    // `natural_title_width`; the marquee therefore triggers exactly when the
    // "…" suffix does.
    let truncated = box_w > px(0.) && text_w > box_w;
    let color = if selected { FG_STRONG } else { FG };

    let clip = div()
        .relative()
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_hidden()
        .on_prepaint({
            let metrics = Rc::clone(&list.title_box_w);
            let key = data.id.clone();
            move |bounds, _, _| {
                metrics.borrow_mut().insert(key.clone(), bounds.size.width);
            }
        });

    if hovered && truncated {
        // Seamless loop: the track carries the title TWICE separated by the
        // 24px between-pass gap, so the period is exactly one copy + gap.
        // Each cycle holds 600ms at x=0, then scrolls one full period — the
        // wrap back to x=0 lands on pixel-identical content (copy B where
        // copy A was), so there is no visible snap. Two copies always
        // suffice: the truncation condition gives text_w > box_w, so the
        // period alone already overshoots the clip width.
        let period = f32::from(text_w) + MARQUEE_GAP;
        let secs = period / MARQUEE_SPEED;
        let total = MARQUEE_PAUSE + secs;
        let fade: Hsla = if selected {
            CARD_BG.into()
        } else {
            SHELL_BG.into()
        };
        let title = data.title.clone();
        clip.child(
            div()
                .absolute()
                .top_0()
                .left(px(0.))
                .h_full()
                .with_animation(
                    SharedString::from(format!("marquee-{}", data.id)),
                    gpui::Animation::new(Duration::from_secs_f32(total)).repeat(),
                    move |el, t| {
                        let tt = t * total;
                        let x = if tt < MARQUEE_PAUSE {
                            0.
                        } else {
                            -((tt - MARQUEE_PAUSE) * MARQUEE_SPEED).min(period)
                        };
                        let copy = || {
                            div()
                                .whitespace_nowrap()
                                .flex_shrink_0()
                                .text_size(px(TITLE_SIZE))
                                .font_weight(weight)
                                .text_color(color)
                                .child(title.clone())
                        };
                        el.left(px(x))
                            .flex()
                            .flex_row()
                            .items_center()
                            .child(copy())
                            .child(div().w(px(MARQUEE_GAP)).flex_shrink_0())
                            .child(copy())
                    },
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .right_0()
                        .w(px(MARQUEE_FADE_W))
                        .h_full()
                        .bg(linear_gradient(
                            90.,
                            linear_color_stop(gpui::transparent_black(), 0.),
                            linear_color_stop(fade, 1.),
                        )),
                ),
        )
        .into_any_element()
    } else {
        clip.child(
            div()
                .truncate()
                .text_size(px(TITLE_SIZE))
                .font_weight(weight)
                .text_color(color)
                .child(data.title.clone()),
        )
        .into_any_element()
    }
}

/// Natural (unclipped) width of the title line — the marquee's truncation
/// probe, and the SAME expression the text element runs before eliding:
/// shape the natural line via `shape_text`, then compare
/// `line.size(line_height).width` against the clip width. Sharing the
/// renderer's shaping entry point and width accessor keeps the marquee
/// trigger and the "…" suffix one decision; the run mirrors the title div's
/// resolved style (root font family + state weight + size), the one piece
/// gpui does not expose from an element-build context. A shaping failure
/// degrades to width 0 (marquee off, the rendered ellipsis stays
/// authoritative).
fn natural_title_width(window: &Window, text: &str, weight: FontWeight) -> Pixels {
    let font_size = px(TITLE_SIZE);
    // Only feeds WrappedLine::size's height; width is line-height
    // independent. 1.28 is the shell root's relative line height.
    let line_height = px(TITLE_SIZE * 1.28);
    let run = gpui::TextRun {
        len: text.len(),
        font: gpui::Font {
            family: FONT_UI.into(),
            weight,
            ..gpui::Font::default()
        },
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_text(text.into(), font_size, &[run], None, None)
        .ok()
        .and_then(|lines| lines.first().map(|line| line.size(line_height).width))
        .unwrap_or(px(0.))
}

/// The running pixel grid: 2×3 dots inside a 16×16 clipped container. The
/// timeline is quantized into six discrete steps, each dot's phase shifted
/// by its delay; the container clips the fall.
fn pixel_grid(id: &str) -> impl IntoElement {
    div()
        .size(px(16.))
        .relative()
        .overflow_hidden()
        .with_animation(
            SharedString::from(format!("pxgrid-{id}")),
            gpui::Animation::new(Duration::from_millis(PIXEL_GRID_MS as u64)).repeat(),
            |el, t| {
                let q_ms = (t * 6.).floor().min(5.) / 5. * PIXEL_GRID_MS;
                el.children(PIXEL_DOTS.iter().enumerate().map(|(i, (delay, variant))| {
                    let (col, row) = ((i % 2) as f32, (i / 2) as f32);
                    let local = if q_ms < *delay {
                        0.
                    } else {
                        (q_ms - delay) % PIXEL_GRID_MS
                    };
                    let (dy, alpha) = spin_phase(local / PIXEL_GRID_MS, *variant);
                    div()
                        .absolute()
                        .left(px(5. + col * 4.))
                        .top(px(3. + row * 4. + dy))
                        .size(px(2.))
                        .rounded_full()
                        .bg(accent_alpha(alpha))
                }))
            },
        )
}

/// One dot's keyframes: rise from -4px (invisible), hold at 0 (opaque), fall
/// to +7px fading out. The long/short variants stretch or shrink the hold so
/// the cascade reads as a wave.
fn spin_phase(u: f32, variant: SpinVariant) -> (f32, f32) {
    let (hold, fall) = match variant {
        SpinVariant::Cycle => (0.5714, 0.6648),
        SpinVariant::Long => (0.6429, 0.7363),
        SpinVariant::Short => (0.50, 0.5934),
    };
    if u < 0.0934 {
        (-4., 0.)
    } else if u < hold {
        (0., 1.)
    } else if u < fall {
        let f = (u - hold) / (fall - hold);
        (7. * f, 1. - f)
    } else {
        (7., 0.)
    }
}

fn accent_alpha(a: f32) -> Hsla {
    let mut c: Hsla = ACCENT.into();
    c.a = a;
    c
}

fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The info-line timestamp: relative (`刚刚 / N分钟前 / N小时前`) within 72h,
/// then a local `MM-DD HH:MM` wall-clock stamp. The chrome's own rule — the
/// legacy sidebar's week-long relative window is intentionally not shared.
pub fn format_active_time(updated_at: i64, now: i64) -> String {
    let delta = (now - updated_at).max(0);
    if delta < 60 {
        steer_i18n::t("sidebar-time-just-now")
    } else if delta < 3_600 {
        steer_i18n::t_count("sidebar-time-minutes", delta / 60)
    } else if delta < 3 * 86_400 {
        steer_i18n::t_count("sidebar-time-hours", delta / 3_600)
    } else {
        absolute_stamp(updated_at)
    }
}

fn absolute_stamp(epoch: i64) -> String {
    use chrono::TimeZone as _;
    chrono::Local
        .timestamp_opt(epoch, 0)
        .single()
        .map(|t| t.format("%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

/// The bottom Customizations block.
fn customizations(rows: &[CustomizationRow]) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(1.))
        .pt(px(6.))
        .flex_shrink_0()
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(5.))
                .py(px(4.))
                .px(px(6.))
                .text_size(px(12.5))
                .child(icon(icons::CHEVRON_DOWN, 13.))
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .child(steer_i18n::t("chrome-customizations")),
                ),
        )
        .children(rows.iter().map(|r| {
            div()
                .id(SharedString::from(format!("custom-{}", r.label)))
                .w_full()
                .mx(px(14.))
                .flex()
                .items_center()
                .gap(px(7.))
                .py(px(3.))
                .px(px(6.))
                .text_size(px(12.))
                .flex_shrink_0()
                .text_color(FG_FAINT)
                .tooltip(hover_tooltip("chrome-unimplemented"))
                .child(icon(r.icon, 13.))
                .child(div().min_w_0().flex_1().truncate().child(r.label.clone()))
                .when_some(r.count, |this, c| {
                    this.child(
                        div()
                            .text_size(px(11.))
                            .text_color(FG_FAINT)
                            .child(c.to_string()),
                    )
                })
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_i18n() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(steer_i18n::init);
    }

    /// The three windows agree with the user rule: sub-minute → 刚刚,
    /// sub-hour → whole minutes, sub-72h → whole hours.
    #[test]
    fn active_time_windows() {
        init_i18n();
        let now = 1_000_000;
        assert_eq!(format_active_time(now, now), "刚刚");
        assert_eq!(format_active_time(now - 59, now), "刚刚");
        assert_eq!(format_active_time(now - 60, now), "1 分钟前");
        assert_eq!(format_active_time(now - 3_599, now), "59 分钟前");
        assert_eq!(format_active_time(now - 3_600, now), "1 小时前");
        assert_eq!(format_active_time(now - 259_199, now), "71 小时前");
    }

    /// At exactly 72h the display flips to the absolute stamp — an 11-char
    /// `MM-DD HH:MM` wall-clock shape (digit positions asserted instead of a
    /// literal so the test is timezone-independent). Future-dated rows (clock
    /// skew) clamp to 刚刚, never a negative delta.
    #[test]
    fn active_time_flips_to_absolute_at_72h() {
        init_i18n();
        let now = 1_800_000_000;
        let stamp = format_active_time(now - 259_200, now);
        assert_eq!(stamp.len(), 11, "MM-DD HH:MM shape, got {stamp:?}");
        assert_eq!(&stamp[2..3], "-");
        assert_eq!(&stamp[5..6], " ");
        assert_eq!(&stamp[8..9], ":");
        assert!(
            stamp
                .chars()
                .enumerate()
                .all(|(i, c)| matches!(i, 2 | 5 | 8) || c.is_ascii_digit()),
            "MM-DD HH:MM digit shape, got {stamp:?}"
        );
        assert_eq!(format_active_time(now + 30, now), "刚刚");
    }
}
