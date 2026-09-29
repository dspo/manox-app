//! The conversation column's left-edge turn rail: the dsh TurnNavigator
//! mirrored onto the leading edge. One tick per user turn at a fixed pitch,
//! vertically centered over the message band; the active mark tracks the
//! message list's scroll position, hovering a mark previews the turn in a
//! card beside the rail, and clicking one jumps through the same
//! `reveal_message` path as the ⌘M popup navigator.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ClickEvent, Entity, FontWeight, Hsla,
    IntoElement, ListSizingBehavior, ListState, ParentElement as _, Pixels, ScrollStrategy,
    SharedString, Styled as _, Window, div, ease_out_quint, prelude::*, px, uniform_list,
};
use gpui_component::{ElementExt as _, Theme, h_flex, v_flex};

use crate::column::ChatColumn;
use crate::conversation::ConvItem;
use crate::i18n;
use crate::views::turn_navigator::collapse_whitespace;

/// Fixed pitch between neighbouring marks; overflow scrolls inside the rail.
pub const MARK_PITCH: f32 = 10.;
/// The rail strip's width (the marks' column).
pub const RAIL_WIDTH: f32 = 28.;
/// The rail strip's left inset inside the message band.
pub const RAIL_LEFT_INSET: f32 = 4.;
/// Left padding the transcript reserves while the rail is visible.
pub const GUTTER: f32 = 40.;
/// Cap on the marks ladder's height before it scrolls internally.
pub const MAX_RAIL_HEIGHT: f32 = 420.;
/// Minimum conversation-card width for the rail to mount.
pub const MIN_CARD_WIDTH: f32 = 640.;

const TICK_REST_W: f32 = 12.;
const TICK_HOVER_W: f32 = 18.;
const TICK_ACTIVE_W: f32 = 20.;
const TICK_H: f32 = 2.;
/// Hover preview card: width, height budget, and gap off the marks.
const PREVIEW_WIDTH: f32 = 300.;
const PREVIEW_HEIGHT: f32 = 100.;
const PREVIEW_GAP: f32 = 10.;
/// Preview enter / travel and active-tick tween durations (dsh's values).
const PREVIEW_ENTER_MS: u64 = 120;
const PREVIEW_TRAVEL_MS: u64 = 140;
const TICK_TWEEN_MS: u64 = 140;
/// Preview budgets: one prompt line, a few response lines.
const PROMPT_PREVIEW_CHARS: usize = 50;
const RESPONSE_PREVIEW_CHARS: usize = 120;

/// One rail mark: the conversation item the turn anchors on (its user
/// bubble) plus the bounded preview texts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RailTurn {
    pub item_ix: usize,
    pub prompt: String,
    pub response: String,
}

/// Project the conversation into ascending rail turns. The prompt is the
/// user bubble's text; the response is the turn's last non-empty assistant
/// reply (dsh's `findLast` rule). Both are collapsed and capped so the
/// preview stays bounded on huge turns. Assistant rows before the first user
/// bubble belong to no turn and are dropped.
pub fn collect_rail_turns<'a>(items: impl Iterator<Item = (usize, &'a ConvItem)>) -> Vec<RailTurn> {
    let mut turns: Vec<RailTurn> = Vec::new();
    for (ix, item) in items {
        match item {
            ConvItem::User { text, .. } => turns.push(RailTurn {
                item_ix: ix,
                prompt: cap_preview(text, PROMPT_PREVIEW_CHARS),
                response: String::new(),
            }),
            ConvItem::Assistant { text, .. } => {
                if text.trim().is_empty() {
                    continue;
                }
                if let Some(last) = turns.last_mut() {
                    last.response = cap_preview(text, RESPONSE_PREVIEW_CHARS);
                }
            }
            _ => {}
        }
    }
    turns
}

/// Collapse whitespace and cap at `limit` characters with a trailing
/// ellipsis when clipped.
fn cap_preview(text: &str, limit: usize) -> String {
    let normalized = collapse_whitespace(text);
    if normalized.chars().count() > limit - 1 {
        let head: String = normalized.chars().take(limit - 1).collect();
        let head = head.trim_end();
        format!("{head}…")
    } else {
        normalized
    }
}

/// The mark at the reading line, given the list's logical top item: the last
/// turn whose anchor is at or above it. The list's floor (tail-follow)
/// reports `count` as the top, which resolves to the newest turn; rows above
/// the first user bubble resolve to the first mark.
pub fn active_mark(top_item_ix: usize, turns: &[RailTurn]) -> Option<usize> {
    if turns.is_empty() {
        return None;
    }
    Some(
        turns
            .iter()
            .rposition(|turn| turn.item_ix <= top_item_ix)
            .unwrap_or(0),
    )
}

/// [`active_mark`] fed straight from the message list's scroll position.
pub fn active_rail_turn(list_state: &ListState, turns: &[RailTurn]) -> Option<usize> {
    active_mark(list_state.logical_scroll_top().item_ix, turns)
}

/// The marks ladder's laid-out height: its content size, capped by
/// `MAX_RAIL_HEIGHT` and the rail box (which shrinks it on short bands).
fn ladder_height(mark_count: usize, box_h: Pixels) -> Pixels {
    px(mark_count as f32 * MARK_PITCH)
        .min(px(MAX_RAIL_HEIGHT))
        .min(box_h)
}

/// The ladder's top inside the rail box: centered while shorter than the
/// box, flush while filling it. An unmeasured box (`None`, first frame)
/// assumes a flush top until the capture lands.
fn ladder_top(mark_count: usize, box_h: Option<Pixels>) -> Pixels {
    match box_h {
        None => px(0.),
        Some(h) => (h - ladder_height(mark_count, h)) / 2.,
    }
}

/// A mark's center y inside the rail box, from the ladder's scroll offset.
fn mark_center_y(mark_ix: usize, ladder_top: Pixels, scroll_top: Pixels) -> Pixels {
    ladder_top + px(mark_ix as f32 * MARK_PITCH + MARK_PITCH / 2.) - scroll_top
}

/// Clamp the preview's top so the card stays inside the rail box. An
/// unmeasured box skips the clamp's ceiling; the band's own
/// `overflow_hidden` is the backstop for that first frame.
fn preview_top(mark_y: Pixels, box_h: Option<Pixels>) -> Pixels {
    let centered = (mark_y - px(PREVIEW_HEIGHT / 2.)).max(px(0.));
    match box_h {
        None => centered,
        Some(h) => centered.min((h - px(PREVIEW_HEIGHT)).max(px(0.))),
    }
}

fn lerp_px(a: Pixels, b: Pixels, delta: f32) -> Pixels {
    px(f32::from(a) + (f32::from(b) - f32::from(a)) * delta)
}

fn lerp_hsla(a: Hsla, b: Hsla, delta: f32) -> Hsla {
    Hsla {
        h: a.h + (b.h - a.h) * delta,
        s: a.s + (b.s - a.s) * delta,
        l: a.l + (b.l - a.l) * delta,
        a: a.a + (b.a - a.a) * delta,
    }
}

/// Jump callback: receives the anchor item index of the clicked mark.
pub type JumpFn = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// Build the rail for the current frame: marks, hover preview, and the
/// per-frame bookkeeping (active-tick tween endpoints, preview travel
/// origin, active-follow). Returns `None` below two turns.
///
/// The bookkeeping writes land before the elements are built and repaint
/// the same frame, so no notify is sent — the frame is already in flight.
pub fn render_turn_rail(
    theme: &Theme,
    chat: &Entity<ChatColumn>,
    turns: Vec<RailTurn>,
    on_jump: JumpFn,
    cx: &mut App,
) -> Option<AnyElement> {
    let count = turns.len();
    if count < 2 {
        return None;
    }
    let active = active_rail_turn(&chat.read(cx).list_state, &turns)?;

    let box_h_raw = chat.read(cx).turn_rail_box_h.get();
    let box_h = (box_h_raw > px(0.)).then_some(box_h_raw);
    let scroll_top = chat
        .read(cx)
        .turn_rail_scroll
        .0
        .borrow()
        .base_handle
        .offset()
        .y;
    let hover = chat.read(cx).turn_rail_hover;
    let travel_from = match (
        chat.read(cx).turn_rail_preview_mark,
        chat.read(cx).turn_rail_preview_top,
        hover,
    ) {
        (Some(prev_mark), Some(top), Some(ix)) if prev_mark != ix => Some(top),
        _ => None,
    };
    let new_preview_top = hover.map(|ix| {
        preview_top(
            mark_center_y(ix, ladder_top(count, box_h), scroll_top),
            box_h,
        )
    });

    chat.update(cx, |chat, _| {
        if chat.turn_rail_active != Some(active) {
            chat.turn_rail_active_from = chat.turn_rail_active;
            chat.turn_rail_active = Some(active);
            chat.turn_rail_active_gen += 1;
        }
        // The active mark centers only while the pointer is elsewhere;
        // inside the rail the marks never travel under the hand.
        if !chat.turn_rail_pointer_inside && chat.turn_rail_followed != Some((active, count)) {
            chat.turn_rail_followed = Some((active, count));
            chat.turn_rail_scroll
                .scroll_to_item(active, ScrollStrategy::Nearest);
        }
        // Preview bookkeeping: an appearance replays the enter run with no
        // travel origin; a re-target keeps the last painted top as the
        // travel origin (dsh's indicator_from discipline — snapshot only
        // when the target changes).
        match (chat.turn_rail_preview_mark, hover) {
            (None, Some(_)) => {
                chat.turn_rail_preview_mark = hover;
                chat.turn_rail_preview_top = None;
                chat.turn_rail_preview_gen += 1;
            }
            (Some(prev_mark), Some(ix)) if prev_mark != ix => {
                chat.turn_rail_preview_mark = Some(ix);
            }
            _ => {}
        }
        if hover.is_none() {
            chat.turn_rail_preview_mark = None;
            chat.turn_rail_preview_top = None;
        } else {
            chat.turn_rail_preview_top = new_preview_top;
        }
    });

    let enter_gen = chat.read(cx).turn_rail_preview_gen;

    let item_ixes: Vec<usize> = turns.iter().map(|turn| turn.item_ix).collect();
    let chat_for_marks = chat.clone();
    let theme_for_marks = theme.clone();
    let scroll = chat.read(cx).turn_rail_scroll.clone();
    let marks = uniform_list(
        "turn-rail-marks",
        count,
        move |visible_range, _window, cx| {
            let state = chat_for_marks.read(cx);
            let active = state.turn_rail_active;
            let from_active = state.turn_rail_active_from;
            let active_gen = state.turn_rail_active_gen;
            let hover = state.turn_rail_hover;
            visible_range
                .map(|ix| {
                    render_mark_row(
                        ix,
                        item_ixes[ix],
                        active,
                        from_active,
                        active_gen,
                        hover,
                        &theme_for_marks,
                        chat_for_marks.clone(),
                        on_jump.clone(),
                    )
                })
                .collect::<Vec<_>>()
        },
    )
    .with_sizing_behavior(ListSizingBehavior::Infer)
    .max_h(px(MAX_RAIL_HEIGHT))
    .w_full()
    .track_scroll(&scroll);

    let preview = hover.filter(|ix| *ix < count).map(|ix| {
        let top = new_preview_top.expect("top computed for every hover");
        render_preview(theme, &turns[ix], ix, enter_gen, travel_from, top)
    });

    let box_h_cell = chat.read(cx).turn_rail_box_h.clone();
    let chat_for_hover = chat.clone();
    Some(
        div()
            .id("turn-rail")
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(RAIL_LEFT_INSET))
            .w(px(RAIL_WIDTH))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .debug_selector(|| "turn-rail".into())
            .on_prepaint(move |bounds, _window, _app| box_h_cell.set(bounds.size.height))
            .on_hover(move |inside, _window, cx| {
                chat_for_hover.update(cx, |chat, cx| {
                    chat.turn_rail_pointer_inside = *inside;
                    if !inside && chat.turn_rail_hover.is_some() {
                        chat.turn_rail_hover = None;
                    }
                    cx.notify();
                });
            })
            .child(marks)
            .children(preview)
            .into_any_element(),
    )
}

/// One 10px hit row with its tick visual. The row is the click/hover target;
/// the tick is purely visual, left-anchored (the mirrored image of dsh's
/// right-anchored marks).
#[allow(clippy::too_many_arguments)]
fn render_mark_row(
    ix: usize,
    item_ix: usize,
    active: Option<usize>,
    from_active: Option<usize>,
    active_gen: u64,
    hover: Option<usize>,
    theme: &Theme,
    chat: Entity<ChatColumn>,
    on_jump: JumpFn,
) -> AnyElement {
    let is_active = active == Some(ix);
    let is_hover = hover == Some(ix) && !is_active;
    let (target_w, target_color) = if is_active {
        (px(TICK_ACTIVE_W), theme.foreground)
    } else if is_hover {
        (px(TICK_HOVER_W), theme.muted_foreground)
    } else {
        (px(TICK_REST_W), theme.border)
    };

    // The previous active mark tweens back to rest while the new one tweens
    // up (dsh's 140ms pair). Each change runs under a fresh id keyed by the
    // generation, so it starts from the resting style instead of resuming.
    let (rest_color, active_color) = (theme.border, theme.foreground);
    let tick: AnyElement = if is_active && from_active.is_some() {
        div()
            .w(target_w)
            .h(px(TICK_H))
            .rounded_full()
            .with_animation(
                format!("turn-rail-tick-{ix}-{active_gen}"),
                Animation::new(Duration::from_millis(TICK_TWEEN_MS)).with_easing(ease_out_quint()),
                move |el, delta| {
                    el.w(lerp_px(px(TICK_REST_W), target_w, delta))
                        .bg(lerp_hsla(rest_color, active_color, delta))
                },
            )
            .into_any_element()
    } else if !is_active && from_active == Some(ix) {
        div()
            .w(target_w)
            .h(px(TICK_H))
            .rounded_full()
            .with_animation(
                format!("turn-rail-tick-{ix}-{active_gen}"),
                Animation::new(Duration::from_millis(TICK_TWEEN_MS)).with_easing(ease_out_quint()),
                move |el, delta| {
                    el.w(lerp_px(px(TICK_ACTIVE_W), target_w, delta))
                        .bg(lerp_hsla(active_color, rest_color, delta))
                },
            )
            .into_any_element()
    } else {
        div()
            .w(target_w)
            .h(px(TICK_H))
            .rounded_full()
            .bg(target_color)
            .into_any_element()
    };

    let chat_for_enter = chat.clone();
    let chat_for_leave = chat.clone();
    h_flex()
        .id(("turn-rail-mark", ix))
        .h(px(MARK_PITCH))
        .w_full()
        .items_center()
        .debug_selector(move || format!("turn-rail-mark-{item_ix}"))
        .on_hover(move |hovered, _window, cx| {
            if *hovered {
                chat_for_enter.update(cx, |chat, cx| {
                    if chat.turn_rail_hover != Some(ix) {
                        chat.turn_rail_hover = Some(ix);
                        cx.notify();
                    }
                });
            } else {
                // Only retract the row's own mark: a stale leave landing
                // after the next row's enter must not clear it (gpui hover
                // crossing).
                chat_for_leave.update(cx, |chat, cx| {
                    if chat.turn_rail_hover == Some(ix) {
                        chat.turn_rail_hover = None;
                        cx.notify();
                    }
                });
            }
        })
        .on_click(move |_: &ClickEvent, window, cx| on_jump(item_ix, window, cx))
        .child(tick)
        .into_any_element()
}

/// The hover card: prompt line over response excerpt, fading in with a 4px
/// slide from the marks and traveling between marks without replaying the
/// enter run.
fn render_preview(
    theme: &Theme,
    turn: &RailTurn,
    ix: usize,
    enter_gen: u64,
    travel_from: Option<Pixels>,
    top: Pixels,
) -> AnyElement {
    let prompt: SharedString = if turn.prompt.is_empty() {
        i18n::t("turn-navigator-attachment-only")
    } else {
        SharedString::from(turn.prompt.clone())
    };
    let mut card = v_flex()
        .w(px(PREVIEW_WIDTH))
        .max_h(px(PREVIEW_HEIGHT))
        .overflow_hidden()
        .p_2()
        .gap_1()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .text_color(theme.popover_foreground)
        .shadow_md()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .truncate()
                .child(prompt),
        );
    if !turn.response.is_empty() {
        card = card.child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(turn.response.clone())),
        );
    }
    let entered = div()
        .with_animation(
            format!("turn-rail-preview-enter-{enter_gen}"),
            Animation::new(Duration::from_millis(PREVIEW_ENTER_MS)).with_easing(ease_out_quint()),
            |el, delta| el.opacity(delta).ml(px(4. * (1. - delta))),
        )
        .child(card);
    match travel_from {
        Some(from_top) => div()
            .absolute()
            .left(px(RAIL_WIDTH + PREVIEW_GAP))
            .with_animation(
                format!("turn-rail-preview-top-{ix}"),
                Animation::new(Duration::from_millis(PREVIEW_TRAVEL_MS))
                    .with_easing(ease_out_quint()),
                move |el, delta| el.top(lerp_px(from_top, top, delta)),
            )
            .child(entered)
            .into_any_element(),
        None => div()
            .absolute()
            .left(px(RAIL_WIDTH + PREVIEW_GAP))
            .top(top)
            .child(entered)
            .into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> ConvItem {
        ConvItem::User {
            text: text.to_string(),
            images: Vec::new(),
            meta: None,
        }
    }

    fn assistant(text: &str) -> ConvItem {
        ConvItem::Assistant {
            text: text.to_string(),
            streaming: false,
            token_usage: None,
            activity_header: false,
            entry_id: None,
            fork_unavailable: None,
        }
    }

    #[test]
    fn collects_ascending_turns_with_last_non_empty_response() {
        let items = [
            assistant("orphan reply"),
            user("first"),
            assistant("first reply"),
            assistant("  "),
            assistant("first reply two"),
            user("second"),
            assistant("second reply"),
        ];
        let turns = collect_rail_turns(items.iter().enumerate());
        assert_eq!(
            turns,
            vec![
                RailTurn {
                    item_ix: 1,
                    prompt: "first".into(),
                    response: "first reply two".into(),
                },
                RailTurn {
                    item_ix: 5,
                    prompt: "second".into(),
                    response: "second reply".into(),
                },
            ]
        );
    }

    #[test]
    fn caps_previews_with_trailing_ellipsis() {
        let long = "word ".repeat(40);
        let capped = cap_preview(&long, 20);
        assert!(capped.chars().count() <= 20);
        assert!(capped.ends_with('…'));

        let short = cap_preview("short", 20);
        assert_eq!(short, "short");
    }

    #[test]
    fn attachment_only_turn_has_empty_prompt() {
        let turns = collect_rail_turns([(0, &user(" "))].into_iter());
        assert_eq!(turns[0].prompt, "");
    }

    #[test]
    fn active_mark_tracks_reading_line() {
        let turns = collect_rail_turns(
            [
                user("a"),
                assistant("ra"),
                user("b"),
                assistant("rb"),
                user("c"),
            ]
            .iter()
            .enumerate(),
        );
        // Anchors at items 0, 2, 4.
        assert_eq!(active_mark(0, &turns), Some(0));
        assert_eq!(active_mark(2, &turns), Some(1));
        assert_eq!(active_mark(3, &turns), Some(1));
        assert_eq!(active_mark(5, &turns), Some(2));
        // The tail-follow floor reports `count` as the top → newest mark.
        assert_eq!(active_mark(usize::MAX, &turns), Some(2));
    }

    #[test]
    fn active_mark_is_none_without_turns() {
        assert_eq!(active_mark(0, &[]), None);
    }

    #[test]
    fn preview_top_clamps_inside_a_measured_box() {
        let box_h = px(300.);
        assert_eq!(preview_top(px(150.), Some(box_h)), px(100.));
        // Near the edges: pinned inside the box.
        assert_eq!(preview_top(px(40.), Some(box_h)), px(0.));
        assert_eq!(preview_top(px(290.), Some(box_h)), px(200.));
        // An unmeasured box only clamps the floor.
        assert_eq!(preview_top(px(-30.), None), px(0.));
        assert_eq!(preview_top(px(500.), None), px(450.));
    }

    #[test]
    fn ladder_geometry_centers_short_content() {
        let box_h = px(300.);
        assert_eq!(ladder_height(3, box_h), px(30.));
        assert_eq!(ladder_top(3, Some(box_h)), px(135.));
        // Tall content fills the box: flush top, capped height.
        assert_eq!(ladder_height(100, box_h), px(300.));
        assert_eq!(ladder_top(100, Some(box_h)), px(0.));
        assert_eq!(mark_center_y(2, px(135.), px(0.)), px(160.));
    }
}
