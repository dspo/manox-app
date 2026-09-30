//! View-layer helpers and the conversation column's view modules.

pub mod completion;
pub mod context_rail;
pub mod message;
pub mod popup_menu;
pub mod subagents;
pub mod turn_navigator;
pub mod turn_rail;

use gpui::prelude::*;
use std::{cell::Cell, rc::Rc};

use gpui::{Div, ListState, Pixels, px};

/// Width-keyed invalidation for native message-list row heights.
///
/// The pinned official GPUI revision remeasures visible rows, but retains the
/// cached heights of off-screen rows across a width change. A height measured
/// at a narrow width can therefore survive after a resize and present as a
/// large blank range. Keeping this tracker outside the list's row callback
/// lets the application invalidate the entire cache after final layout has
/// produced a positive, definite width.
#[derive(Clone, Default)]
pub struct MessageListWidthInvalidator {
    last_width: Rc<Cell<Option<Pixels>>>,
}

impl MessageListWidthInvalidator {
    /// Returns `true` when callers must schedule one more frame to consume the
    /// invalidated cache. Sub-pixel jitter is ignored.
    pub fn update(&self, width: Pixels, state: &ListState) -> bool {
        if width <= px(0.) {
            return false;
        }
        let previous = self.last_width.replace(Some(width));
        let changed = previous.is_some_and(|previous| (previous - width).abs() > px(0.5));
        if changed {
            state.remeasure_items(0..state.item_count());
        }
        changed
    }
}

/// The conversation card's own width, recorded at prepaint by the host.
///
/// Layout budgets that must not be spent on the shell's furniture read this
/// instead of `window.bounds()`: the shell's sidebar and right pane are
/// siblings of the card, so the window width overstates the card by whatever
/// they claim — enough to mis-gate the context rail and to size the turn
/// navigator's panel wider than the card. Unset until the first prepaint;
/// callers fall back to their own estimate until then (the bubble's
/// clearance keeps that fallback conservative — see [`BubbleClearance`]).
#[derive(Clone, Default)]
pub struct CardWidth {
    last_width: Rc<Cell<Option<Pixels>>>,
}

impl CardWidth {
    /// Record this frame's measured card width. Returns `true` when it moved
    /// by more than half a pixel — the caller then schedules one more frame so
    /// the budgets computed from the stale value converge immediately.
    /// Sub-pixel jitter is ignored, and a non-positive width is not a
    /// measurement (an unlaid-out frame).
    pub fn set(&self, width: Pixels) -> bool {
        if width <= px(0.) {
            return false;
        }
        let previous = self.last_width.replace(Some(width));
        previous.is_none_or(|previous| (previous - width).abs() > px(0.5))
    }

    /// The last measured card width, when one has been laid out.
    pub fn get(&self) -> Option<Pixels> {
        self.last_width.get()
    }
}
/// The conversation info bubble's vertical budget, measured in prepaint:
/// the pill's top edge minus the conversation column's top edge (both in
/// window coordinates). `None` parts fall back to the caller's estimate for
/// exactly one frame, like [`CardWidth`].
#[derive(Clone, Default)]
pub struct BubbleClearance {
    column_top: Rc<Cell<Option<Pixels>>>,
    pill_top: Rc<Cell<Option<Pixels>>>,
}

impl BubbleClearance {
    pub fn set_column_top(&self, y: Pixels) -> bool {
        self.column_top
            .replace(Some(y))
            .is_none_or(|previous| (previous - y).abs() > px(0.5))
    }

    pub fn set_pill_top(&self, y: Pixels) -> bool {
        self.pill_top
            .replace(Some(y))
            .is_none_or(|previous| (previous - y).abs() > px(0.5))
    }

    /// The bubble's height cap: from the pill's top down to the column's
    /// top, minus a breathing margin (the `-38` slot apron + pill padding
    /// assume a ~30px pill; the margin absorbs the difference). Unmeasured
    /// frames and degenerately narrow bands floor at 160 — small enough to
    /// stay scrollable inside the column instead of spilling past its top.
    pub fn max_height(&self, fallback: Pixels) -> Pixels {
        match (self.pill_top.get(), self.column_top.get()) {
            (Some(pill), Some(column)) if pill - column > px(160.) => {
                (pill - column - px(8.)).min(fallback)
            }
            _ => fallback.min(px(160.)),
        }
    }
}

/// Wrap content in a full-width, centered container that adapts to the
/// window width (no cap — the host dropped the fixed content width so wide
/// windows get the full span).
///
/// Used by message entries and the input area. The horizontal inset keeps
/// content off the panel edges when the window shrinks near its minimum
/// width.
///
/// `min_w_0` on both the outer row and inner column breaks the min-content
/// chain end to end: without it the row's auto min-size = its widest child's
/// min-content (e.g. a long unbreakable code run in a message, or the composer
/// chip row), pinning the whole list to that width and forcing overflow into
/// the env-card gutter when the window narrows. With it the row shrinks with
/// the window, the input and chip-row gaps absorb the slack, and only the true
/// chip-row floor resists (enforced by `MIN_WINDOW_W`). The list clips any
/// residual incompressible content at its own edge (`overflow_x_hidden`) so it
/// never reaches the window as a horizontal scrollbar.
pub fn centered(child: impl gpui::IntoElement) -> Div {
    use gpui_component::{h_flex, v_flex};
    h_flex()
        .w_full()
        .min_w_0()
        .justify_center()
        .px_4()
        .child(v_flex().w_full().min_w_0().child(child))
}
