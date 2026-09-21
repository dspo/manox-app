//! The sidebar session tree — SessionList (fixed rows + workspace groups +
//! session rows + the bottom Customizations block).
//!
//! Pixel-calibrated against the agents-window reference: group headers
//! (chevron + folder + name) collapse; session rows are two-line fixed-height
//! 46px cards (status icon + title | time · pinned/unread meta); the selected
//! row is a white card with a stroke and floating pin/archive actions on its
//! right edge; hover is `#00000014`. Header overlap rule: the Sessions title
//! fills the width underneath, the right-side controls (New / sort / search)
//! float above it with an opaque background — a too-narrow sidebar occludes
//! the title rather than squeezing the controls.

use std::rc::Rc;
use std::time::Duration;

use crate::theme::{
    ACCENT, BADGE_BLUE_BG, BADGE_BLUE_FG, BORDER, CARD_BG, CARD_BORDER, ERR_RED, FG, FG_FAINT,
    FG_STRONG, Icon, LIST_HOVER, OK_GREEN, icon, icons,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnimationExt, App, ClickEvent, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels,
    RenderOnce, SharedString, Stateful, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::primitives::{icon_button, kbd_chip, small_icon_button};

/// Action callback taking a row id (select / toggle group / pin / archive).
pub type OnId = Rc<dyn Fn(&String, &mut Window, &mut App)>;
/// Action callback with no payload (New).
pub type OnUnit = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
/// Row-menu callback (id + open position, right-click / kebab).
pub type OnRowMenu = Rc<dyn Fn(&String, gpui::Point<Pixels>, &mut Window, &mut App)>;
/// Drag-over-group callback updating the drop marker (dragged, target, before-half?).
pub type OnGroupDragMove = Rc<dyn Fn(&String, &String, bool, &mut Window, &mut App)>;
/// Drop-commit callback for group reordering (dragged, target, before-half?).
pub type OnGroupMove = Rc<dyn Fn(&String, &String, bool, &mut Window, &mut App)>;

/// One session row as projected by the host. The chrome owns only the
/// interaction state around these (selection, collapse, order); it never
/// fetches or interprets sessions itself.
#[derive(Clone, PartialEq)]
pub struct SessionRowData {
    pub id: String,
    pub title: String,
    pub time: String,
    pub status: SessionStatus,
    pub pinned: bool,
    pub unread: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SessionStatus {
    Running,
    Completed,
    NeedsInput,
}

/// One workspace group.
#[derive(Clone, PartialEq)]
pub struct SessionGroup {
    pub name: String,
    pub collapsed: bool,
    pub rows: Vec<SessionRowData>,
}

/// A fixed sidebar row (Automations / Chats …) — labels come from the host.
#[derive(Clone, PartialEq)]
pub struct FixedRow {
    pub icon: Icon,
    pub label: String,
    pub badge: Option<String>,
}

/// A Customizations block row.
#[derive(Clone, PartialEq)]
pub struct CustomizationRow {
    pub icon: Icon,
    pub label: String,
    pub count: Option<u32>,
}

/// Uniform two-line row height (identical across the three states so a state
/// change never reflows the list).
const ROW_H: f32 = 46.;

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
    pub on_select: OnId,
    pub on_toggle_group: OnId,
    pub on_new: OnUnit,
    /// Pin/archive for the selected row (the host performs the real store
    /// write; the shell flips its local copy for immediate feedback).
    pub on_pin: Option<OnId>,
    pub on_archive: Option<OnId>,
    /// Row menu (kebab / right-click): copy Thread ID, pin, archive.
    pub on_row_menu: OnRowMenu,
    /// Group reorder commit (dragged → before/after edge of target).
    pub on_move_group: OnGroupMove,
    /// Drag-over-group: update the drop marker (insertion-line position).
    pub on_drag_move_group: OnGroupDragMove,
    /// Current drop marker (dragged, target, before?); drives the 2px accent
    /// insertion line.
    pub group_drag_marker: Option<(String, String, bool)>,
}

impl RenderOnce for SessionList {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .py(px(4.))
            .px(px(8.))
            .gap(px(1.))
            .text_color(FG)
            .child(header(self.on_new.clone()))
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
                                .child(manox_i18n::t("chrome-no-chats")),
                        )
                    })
                    .children(self.groups.iter().map(|g| group(&self, g))),
            )
            .child(customizations(&self.customizations))
    }
}

/// Header: the Sessions title + New button (⌘N kbd chip) + sort/search.
/// Overlap rule: the title layer fills the width underneath, the controls
/// float above it with an opaque background — a narrow sidebar occludes the
/// title (the reference screenshot's "Se…" truncation) instead of squeezing
/// the controls.
fn header(on_new: OnUnit) -> impl IntoElement {
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
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(FG_STRONG)
                        .child(manox_i18n::t("chrome-sessions-title")),
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
                        .child(manox_i18n::t("chrome-new"))
                        .child(kbd_chip("⌘N")),
                )
                .child(icon_button(
                    "sort",
                    icons::SORT_PRECEDENCE,
                    14.,
                    false,
                    |_, _, _| {},
                ))
                .child(icon_button(
                    "search",
                    icons::SEARCH,
                    14.,
                    false,
                    |_, _, _| {},
                )),
        )
}

/// Fixed row: icon + label + badge (the badge hugs the label; no flex fill).
fn fixed_row(row: &FixedRow) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(8.))
        .py(px(5.))
        .px(px(8.))
        .rounded(px(4.))
        .flex_shrink_0()
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
/// updates the marker.
fn group(list: &SessionList, g: &SessionGroup) -> impl IntoElement {
    let toggle = list.on_toggle_group.clone();
    let name = g.name.clone();
    let on_move = list.on_move_group.clone();
    let on_drag_move_cb = list.on_drag_move_group.clone();
    let marker = list.group_drag_marker.clone();

    let header_name = g.name.clone();
    let mut items: Vec<gpui::AnyElement> = vec![
        div()
            .id(SharedString::from(format!("grp-{name}")))
            .on_click(move |_, w, cx| toggle(&name, w, cx))
            .on_drag(DraggedGroup(SharedString::from(header_name.as_str())), {
                let payload = DraggedGroup(SharedString::from(header_name.as_str()));
                move |_, _, _, cx| {
                    use gpui::AppContext as _;
                    cx.stop_propagation();
                    let p = payload.clone();
                    cx.new(move |_| p.clone())
                }
            })
            .on_drag_move::<DraggedGroup>({
                let target = g.name.clone();
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
            })
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
                    .font_weight(gpui::FontWeight::BOLD)
                    .min_w_0()
                    .truncate()
                    .child(g.name.clone()),
            )
            .into_any_element(),
    ];
    if !g.collapsed {
        items.extend(
            g.rows
                .iter()
                .map(|r| session_row(list, r).into_any_element()),
        );
    }

    let slot_name = g.name.clone();
    div()
        .id(SharedString::from(format!("grp-slot-{slot_name}")))
        .w_full()
        .flex()
        .flex_col()
        .relative()
        .on_drop::<DraggedGroup>({
            let target = g.name.clone();
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
        })
        .children(marker.clone().and_then(|(dragged, target, before)| {
            (target == g.name && dragged != g.name).then(|| {
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
        }))
        .children(items)
}

/// One session row (two lines):
/// - line 1: conversation status icon + title (single-line truncated); when
///   selected, floating pin/archive/menu actions sit absolutely at the right
///   edge with an opaque background, covering the title on narrow widths
///   rather than squeezing it
/// - line 2: meta (last-active time · pinned/unread) + unread dot + short-id
///   tag
fn session_row(list: &SessionList, data: &SessionRowData) -> Stateful<gpui::Div> {
    let selected = list.selected.as_deref() == Some(data.id.as_str());

    // Line-1 leading status glyph: running = falling blocks;
    // needs-input/errored = red warning triangle; completed = empty slot.
    let leading: gpui::AnyElement = match data.status {
        SessionStatus::Running => running_blocks(&data.id).into_any_element(),
        SessionStatus::NeedsInput => div()
            .size(px(16.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .text_color(ERR_RED)
            .child(icon(icons::WARNING, 11.))
            .into_any_element(),
        SessionStatus::Completed => div().size(px(16.)).flex_shrink_0().into_any_element(),
    };

    let mut meta = data.time.clone();
    if data.pinned {
        meta.push_str(" · ");
        meta.push_str(&manox_i18n::t("chrome-row-pinned"));
    }
    if data.unread {
        meta.push_str(" · ");
        meta.push_str(&manox_i18n::t("chrome-row-unread"));
    }

    // Selected-state actions: absolutely positioned at the right edge with an
    // opaque background covering the title (the occlusion rule), plus a
    // kebab for the row menu (copy ID / pin / archive).
    let actions = selected.then(|| {
        let pinned = data.pinned;
        let on_pin = list.on_pin.clone();
        let on_archive = list.on_archive.clone();
        let on_menu = list.on_row_menu.clone();
        let id_pin = data.id.clone();
        let id_archive = data.id.clone();
        let id_menu = data.id.clone();
        div()
            .absolute()
            .top_0()
            .right_0()
            .h_full()
            .flex()
            .items_center()
            .gap(px(2.))
            .py(px(1.))
            .px(px(2.))
            .bg(if selected { CARD_BG } else { LIST_HOVER })
            .child(
                on_pin
                    .map(|cb| {
                        small_icon_button(
                            SharedString::from(format!("pin-{}", data.id)),
                            icons::PIN,
                            11.,
                            if pinned { OK_GREEN } else { FG_FAINT },
                            FG,
                            move |_, w, cx| cb(&id_pin, w, cx),
                        )
                    })
                    .map(IntoElement::into_any_element)
                    .unwrap_or_else(|| div().into_any_element()),
            )
            .child(
                on_archive
                    .map(|cb| {
                        small_icon_button(
                            SharedString::from(format!("archive-{}", data.id)),
                            icons::ARCHIVE,
                            11.,
                            FG_FAINT,
                            FG,
                            move |_, w, cx| cb(&id_archive, w, cx),
                        )
                    })
                    .map(IntoElement::into_any_element)
                    .unwrap_or_else(|| div().into_any_element()),
            )
            .child({
                let id = id_menu.clone();
                small_icon_button(
                    SharedString::from(format!("menu-{}", data.id)),
                    icons::MORE,
                    11.,
                    FG_FAINT,
                    FG,
                    move |e, w, cx| {
                        let pos = match e {
                            ClickEvent::Mouse(m) => m.up.position,
                            _ => gpui::Point::default(),
                        };
                        (on_menu)(&id, pos, w, cx);
                    },
                )
            })
    });

    let on_select = list.on_select.clone();
    let id = data.id.clone();

    let on_menu = list.on_row_menu.clone();
    let id_menu = data.id.clone();
    let row = div()
        .id(SharedString::from(format!("sess-{}", data.id)))
        .on_click(move |_, w, cx| on_select(&id, w, cx))
        .on_mouse_down(gpui::MouseButton::Right, {
            let id = id_menu.clone();
            move |e, w, cx| {
                cx.stop_propagation();
                (on_menu)(&id, e.position, w, cx);
            }
        })
        .w_full()
        .h(px(ROW_H))
        .mx(px(16.))
        .py(px(3.))
        .px(px(6.))
        .rounded(px(4.))
        .flex()
        .flex_col()
        .gap(px(1.))
        .flex_shrink_0()
        .overflow_hidden()
        // Line 1: status + title (fills the width, occluded by the trailing
        // actions when selected).
        .child(
            div()
                .relative()
                .w_full()
                .h(px(20.))
                .flex()
                .items_center()
                .gap(px(4.))
                .child(leading)
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_size(px(12.5))
                        .text_color(if selected { FG_STRONG } else { FG })
                        .child(data.title.clone()),
                )
                .children(actions),
        )
        // Line 2: meta (time · pinned) + flexible gap + unread dot + short-id
        // tag (click copies the full id).
        .child(
            div()
                .w_full()
                .h(px(16.))
                .pl(px(20.))
                .pr(px(2.))
                .flex()
                .items_center()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(11.))
                        .text_color(FG_FAINT)
                        .child(meta),
                )
                .child(div().flex_1())
                .when(data.unread, |this| {
                    this.child(
                        div()
                            .size(px(6.))
                            .rounded_full()
                            .bg(BADGE_BLUE_BG)
                            .flex_shrink_0(),
                    )
                })
                .child(id_tag(&data.id)),
        );

    // Row background (selected card / hover / rest) + a 1px stroke kept
    // transparent in the unselected states so all three share one height.
    if selected {
        row.bg(CARD_BG).border_1().border_color(CARD_BORDER)
    } else {
        row.border_1()
            .border_color(gpui::transparent_black())
            .hover(|style| style.bg(LIST_HOVER))
    }
}

/// Short-id tag chip (click copies the full id).
fn id_tag(id: &str) -> impl IntoElement {
    let short: String = id.chars().take(7).collect();
    let full = id.to_string();
    div()
        .id(SharedString::from(format!("tag-{id}")))
        .on_click(move |_e, _w, cx| {
            cx.stop_propagation();
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(full.clone()));
        })
        .ml(px(4.))
        .px(px(4.))
        .rounded(px(3.))
        .border_1()
        .border_color(BORDER)
        .text_size(px(10.))
        .text_color(FG_FAINT)
        .flex_shrink_0()
        .child(short)
}

/// The "falling blocks" indicator for a running thread: three small squares
/// light up top-down on a loop, brighter the closer to the "current" block
/// (540ms cycle, the calibration's parameters).
fn running_blocks(id: &str) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(1.))
        .items_center()
        .w(px(16.))
        .flex_shrink_0()
        .justify_center()
        .with_animation(
            SharedString::from(format!("blocks-{id}")),
            gpui::Animation::new(Duration::from_millis(540)).repeat(),
            |el, t| {
                let v = t * 3.;
                el.children((0..3).map(move |i| {
                    let d = (v - i as f32).abs().min(1.);
                    let mut c: Hsla = ACCENT.into();
                    c.a = 1.0 - d * (170. / 255.);
                    div().size(px(5.)).rounded(px(1.5)).bg(c)
                }))
            },
        )
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
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(manox_i18n::t("chrome-customizations")),
                ),
        )
        .children(rows.iter().map(|r| {
            div()
                .w_full()
                .mx(px(14.))
                .flex()
                .items_center()
                .gap(px(7.))
                .py(px(3.))
                .px(px(6.))
                .text_size(px(12.))
                .flex_shrink_0()
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
