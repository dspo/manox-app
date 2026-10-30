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
    ACCENT, BADGE_BLUE_BG, BADGE_BLUE_FG, BORDER, CARD_BG, CARD_BORDER, FG, FG_DIM, FG_FAINT,
    FG_STRONG, FONT_UI, IconAsset, LIST_HOVER, SHELL_BG, SUCCESS_GREEN, WARN_AMBER, icon, icons,
};
use ai_elements::{PixelSpinner, PixelSpinnerVariant};
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
    /// The time-grouping stamp (unix seconds): the row's own `updated_at`,
    /// except a container tree's child rows, which the projection stamps
    /// with their captain's — a tree lands in ONE recency bucket, contiguous
    /// under its chevron.
    pub sort_stamp: i64,
    /// The row's position in the HOST's sidebar order (0 = top). The
    /// chrome's local `(pinned, rank)` sort reproduces the host's
    /// interaction-ranked order — no local timestamp reshuffles it. Child
    /// rows carry their captain's rank; rows the host never ranked (live
    /// external sessions) carry `i64::MAX` and sort last.
    pub rank: i64,
    pub status: SessionStatus,
    /// What the row IS — the leading slot's glyph and the row menu's
    /// semantics ride this (the legacy sidebar's RowIcon/RowKind pair,
    /// unified: threads show the status slot, live non-thread sessions show
    /// their brand mark and close instead of archive).
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
    /// A row with container-tree children renders its collapse chevron
    /// before the status slot (the row itself does not indent — nesting
    /// shows via the chevron only).
    pub team_leader: bool,
}

/// The row's primary status, one glyph in the 16px slot: `Pending` the
/// amber pixel question mark (every kind shares the glyph — the tooltip
/// copy carries the difference), `Running` the pixel grid, `Unread` the
/// filled green completion dot, `Idle` the empty slot. A stopped turn's
/// error is never a list state: it renders as turn-level content inside
/// the session.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SessionStatus {
    Running,
    Pending(PendingKind),
    Unread,
    Idle,
}

/// What a parked turn waits on. The slot renders one glyph for every kind;
/// only the tooltip copy branches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingKind {
    /// A tool/permission approval awaiting the user.
    Auth,
    /// An unanswered question awaiting the user.
    Ask,
}

/// What a session row is: the legacy sidebar's unified item abstraction —
/// manox threads and live non-thread sessions (launched CLI agents, plain
/// terminals) render through ONE row component whose leading slot and menu
/// semantics branch on this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SessionRowKind {
    /// A manox thread: the status slot leads; the row menu is the
    /// thread's (pin / archive / tag / copy id).
    #[default]
    Thread,
    /// A live non-thread session: its brand mark leads (the status ladder
    /// does not apply to a PTY); the row menu closes the session.
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
    /// The rows the group renders. The shell folds a long group at a fixed
    /// row limit (5): `hidden` counts the rows beyond the fold, and
    /// `expanded` marks a fold the user (or an active-row reveal) lifted —
    /// the footer row under the group is the chrome's.
    pub rows: Vec<SessionRowData>,
    pub hidden: usize,
    pub expanded: bool,
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

/// The pending question mark on the grid's dot-matrix language: 2px dots
/// at a 3px pitch (col, row) — the arc, the shoulder, the hook, the empty
/// separator row, the detached dot; 8px wide x 14px tall in the 16px slot.
const QUEST_DOTS: [(f32, f32); 6] = [(0., 0.), (1., 0.), (2., 0.), (2., 1.), (1., 2.), (1., 4.)];

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
    /// The inline title editor (row id + input), mounted on that row's
    /// title line (the external-session rename). At most one in flight.
    pub title_edit: Option<(String, Entity<InputState>)>,
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
    /// Escape inside the inline title editor.
    pub on_title_edit_cancel: Option<OnWindowApp>,
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
    /// The fold footer under a group (show more / show less); the payload is
    /// the group's key.
    pub on_show_more: OnId,
    /// The batch-selection mode's checked set (`Some` = selecting). Thread
    /// rows render a check circle in place of the status slot and a click
    /// toggles the check instead of switching sessions; external rows are
    /// not selectable.
    pub selection: Option<Vec<String>>,
    /// Row click while selecting (toggle that row's check).
    pub on_selection_toggle: OnId,
    /// The action bar's 全选 (check every selectable thread row).
    pub on_selection_all: OnUnit,
    /// The action bar's 归档 (commit the batch; the host dispatches the
    /// archive write and leaves selection mode).
    pub on_selection_archive: OnUnit,
    /// The action bar's 取消 (leave selection mode with nothing archived).
    pub on_selection_cancel: OnUnit,
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
            .when_some(self.selection.clone(), |this, selection| {
                this.child(selection_bar(
                    selection.len(),
                    self.on_selection_all.clone(),
                    self.on_selection_archive.clone(),
                    self.on_selection_cancel.clone(),
                ))
            })
    }
}

/// The batch-selection action bar pinned under the list: the count, then
/// 全选 / 归档 / 取消. 归档 is lit only while something is checked — an
/// empty batch has nothing to commit.
fn selection_bar(
    count: usize,
    on_all: OnUnit,
    on_archive: OnUnit,
    on_cancel: OnUnit,
) -> impl IntoElement {
    let pill = |id: &'static str, label: String, accent: bool| {
        div()
            .id(id)
            .px(px(8.))
            .py(px(2.))
            .rounded(px(6.))
            .text_size(px(12.))
            .cursor_pointer()
            .child(label)
            .map(|this| {
                if accent {
                    this.bg(ACCENT).text_color(SHELL_BG)
                } else {
                    this.text_color(FG_DIM).hover(|style| style.bg(LIST_HOVER))
                }
            })
    };
    div()
        .w_full()
        .flex_shrink_0()
        .border_t_1()
        .border_color(BORDER)
        .pt(px(6.))
        .mt(px(2.))
        .flex()
        .items_center()
        .gap(px(6.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(12.))
                .text_color(FG_DIM)
                .child(steer_i18n::t_count("sidebar-selection-count", count as i64)),
        )
        .child(
            pill(
                "selection-all",
                steer_i18n::t("sidebar-selection-select-all"),
                false,
            )
            .on_click(move |e, w, cx| on_all(e, w, cx)),
        )
        .child(
            pill(
                "selection-archive",
                steer_i18n::t("sidebar-selection-archive"),
                true,
            )
            .map(|this| if count == 0 { this.opacity(0.4) } else { this })
            .on_click(move |e, w, cx| on_archive(e, w, cx)),
        )
        .child(
            pill(
                "selection-cancel",
                steer_i18n::t("sidebar-selection-cancel"),
                false,
            )
            .on_click(move |e, w, cx| on_cancel(e, w, cx)),
        )
}

/// A localized hover tooltip view (gpui-component Tooltip over the Root).
fn hover_tooltip(text: &'static str) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView + 'static {
    move |window, cx| gpui_component::tooltip::Tooltip::new(steer_i18n::t(text)).build(window, cx)
}

/// The status line's Fluent key. `None` for Idle: a resting row has nothing
/// to say, so it carries no tooltip (and the truncation marquee keeps the
/// hover).
fn status_key(status: SessionStatus) -> Option<&'static str> {
    match status {
        SessionStatus::Running => Some("sidebar-status-running"),
        SessionStatus::Pending(PendingKind::Auth) => Some("sidebar-status-pending-auth"),
        SessionStatus::Pending(PendingKind::Ask) => Some("sidebar-status-pending-ask"),
        SessionStatus::Unread => Some("sidebar-status-unread"),
        SessionStatus::Idle => None,
    }
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
        // The fold footer: hidden rows → show more (with the count), a
        // lifted fold → show less. A group within the limit carries neither.
        if g.hidden > 0 || g.expanded {
            let cb = list.on_show_more.clone();
            let key = g.key.clone();
            let selector_key = g.key.clone();
            let (label, id) = if g.hidden > 0 {
                (
                    steer_i18n::t_count("chrome-sidebar-show-more", g.hidden as i64).to_string(),
                    "more",
                )
            } else {
                (
                    steer_i18n::t("chrome-sidebar-show-less").to_string(),
                    "less",
                )
            };
            items.push(
                div()
                    .id(SharedString::from(format!("grp-fold-{key}")))
                    .debug_selector(move || format!("chrome-group-fold-{id}-{selector_key}"))
                    .on_click(move |_, w, cx| cb(&key, w, cx))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .py(px(3.))
                    .text_size(px(11.5))
                    .text_color(FG_FAINT)
                    .cursor_pointer()
                    .hover(|style| style.text_color(FG).bg(LIST_HOVER))
                    .child(label)
                    .into_any_element(),
            );
        }
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
    // Selection mode is a thread-row affair: an external row keeps its brand
    // mark and its click stays inert while a batch is being picked.
    let selectable = list.selection.is_some() && data.kind == SessionRowKind::Thread;
    let checked = list
        .selection
        .as_ref()
        .is_some_and(|picked| picked.iter().any(|id| id == &data.id));

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
            let on_toggle = list.on_selection_toggle.clone();
            let selecting = list.selection.is_some();
            let id = data.id.clone();
            move |_, w, cx| {
                if selectable {
                    on_toggle(&id, w, cx);
                } else if !selecting {
                    on_select(&id, w, cx);
                }
            }
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
        // Only rows with something to say carry a tooltip — an "idle" on
        // every resting row is noise and crowds the truncation marquee. The
        // translation happens when the tooltip opens, never on the render
        // path.
        .when_some(status_key(data.status), |el, key| {
            el.tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(steer_i18n::t(key)).build(window, cx)
            })
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
                        // run the status ladder; a live non-thread
                        // session wears its brand mark (the legacy
                        // RowIcon::External slot).
                        .child(match data.kind {
                            SessionRowKind::Thread if selectable => {
                                check_slot(checked).into_any_element()
                            }
                            SessionRowKind::Thread => status_slot(data.status).into_any_element(),
                            SessionRowKind::External { icon: brand } => div()
                                .text_color(FG_FAINT)
                                .child(icon(brand, 14.))
                                .into_any_element(),
                        }),
                )
                .child(match &list.title_edit {
                    Some((edit_id, input)) if edit_id == &data.id => {
                        title_edit_input(input, list.on_title_edit_cancel.clone())
                            .into_any_element()
                    }
                    _ => title_line(list, data, selected, hovered, window),
                }),
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

    // Row surface (checked / selected card / hover / rest) + the 1px stroke
    // kept transparent in the unselected states so all share one height.
    if checked {
        row.bg(CARD_BG).border_1().border_color(ACCENT)
    } else if selected {
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

/// The inline title editor, mounted on the title line in place of the
/// title: the same invisible-border xsmall input as the tag editor, but
/// filling the line. Clicks never reach the row's open click; the input's
/// propagated `Escape` cancels (Enter/blur commit through the shell's
/// subscription).
fn title_edit_input(
    input: &Entity<InputState>,
    on_cancel: Option<OnWindowApp>,
) -> impl IntoElement {
    div()
        .id("title-edit")
        .flex_1()
        .min_w_0()
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

/// The selection-mode leading slot: a faint empty circle, or the accent
/// check once the row is picked.
fn check_slot(checked: bool) -> impl IntoElement {
    div().flex().items_center().justify_center().map(|this| {
        if checked {
            this.text_color(ACCENT).child(icon(icons::CHECK, 14.))
        } else {
            this.text_color(FG_FAINT).child(icon(icons::CIRCLE, 12.))
        }
    })
}

/// The 16px status slot's content, one glyph per state: `Pending` the
/// amber pixel question mark, `Running` the shared PixelSpinner (the
/// conversation strip's own glyph, phase-locked so every running row
/// hops on the same beat), `Unread` the filled green completion dot,
/// `Idle` nothing.
fn status_slot(status: SessionStatus) -> impl IntoElement {
    match status {
        SessionStatus::Pending(_) => pending_mark().into_any_element(),
        SessionStatus::Running => PixelSpinner::new()
            .variant(PixelSpinnerVariant::Grid)
            .color(ACCENT.into())
            .synced()
            // 30fps catches every hop of the native cadence (~34ms per
            // stop); a fleet of running rows repaints a bounded number
            // of frames, not vsync.
            .max_fps(30.)
            .into_any_element(),
        SessionStatus::Unread => div()
            .size(px(6.5))
            .rounded_full()
            .bg(SUCCESS_GREEN)
            .into_any_element(),
        SessionStatus::Idle => div().into_any_element(),
    }
}

/// The pending glyph: 2px dots on the pixel grid's dot-matrix language —
/// top arc, right shoulder, hook back to centre, and the detached dot the
/// question mark needs to read as one (the empty row above it is the
/// separator). Static: a parked turn waits, it does not churn. Both
/// pending kinds share this mark; only the tooltip copy distinguishes them.
fn pending_mark() -> impl IntoElement {
    div()
        .size(px(16.))
        .relative()
        .children(QUEST_DOTS.iter().map(|&(col, row)| {
            div()
                .absolute()
                .left(px(4. + col * 3.))
                .top(px(1. + row * 3.))
                .size(px(2.))
                .rounded_full()
                .bg(WARN_AMBER)
        }))
}

/// The title flattened to one rendering line: every hard line break
/// collapses to a single space. `.truncate()`'s `whitespace_nowrap`
/// suppresses soft wraps only — gpui's line layout still splits on the
/// literal `\n`, which first-message-derived titles carry verbatim.
pub(crate) fn single_line_title(title: &str) -> String {
    title
        .replace('\r', "\n")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
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
    let title = single_line_title(&data.title);
    let text_w = natural_title_width(window, &title, weight);
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
        let title = title.clone();
        clip.child(
            div()
                .absolute()
                .top_0()
                .left(px(0.))
                .h_full()
                .with_animation(
                    SharedString::from(format!("marquee-{}", data.id)),
                    gpui::Animation::new(Duration::from_secs_f32(total))
                        .repeat()
                        // 24px/s reads as smooth at 30fps; unthrottled the
                        // repeat loop repaints the whole window at the
                        // display's refresh rate for as long as the row is
                        // hovered.
                        .with_max_fps(30.),
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
                .child(title),
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

    /// The row title renders as exactly one line: every hard break (\n,
    /// \r\n, runs) collapses to one space, and leading/trailing breaks
    /// vanish.
    #[test]
    fn title_flattens_hard_line_breaks() {
        assert_eq!(
            single_line_title("https://ebwkms2o.mirror.aliyuncs.com\n阿里云镜像"),
            "https://ebwkms2o.mirror.aliyuncs.com 阿里云镜像"
        );
        assert_eq!(single_line_title("a\r\n\rb"), "a b");
        assert_eq!(single_line_title("a \n b"), "a b");
        assert_eq!(single_line_title("\n  a  "), "a");
        assert_eq!(single_line_title("no breaks"), "no breaks");
    }

    /// The status line's key mapping and its zh rendering — the two
    /// pending kinds share the glyph and split only here, and Idle carries
    /// no key at all (a resting row has no tooltip). The locale is pinned
    /// because `init_i18n` alone follows the developer's own settings.toml.
    #[test]
    fn status_tooltip_lines_split_the_pending_kinds() {
        init_i18n();
        steer_i18n::set_ui_language(steer_i18n::Language::ZhCn);
        let key = |status| status_key(status).expect("only Idle has no tooltip");
        assert_eq!(steer_i18n::t(key(SessionStatus::Running)), "运行中");
        assert_eq!(
            steer_i18n::t(key(SessionStatus::Pending(PendingKind::Auth))),
            "等待批准"
        );
        assert_eq!(
            steer_i18n::t(key(SessionStatus::Pending(PendingKind::Ask))),
            "等待回答"
        );
        assert_eq!(steer_i18n::t(key(SessionStatus::Unread)), "未读");
        assert_eq!(status_key(SessionStatus::Idle), None);
    }
}
