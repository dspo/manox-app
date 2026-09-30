//! History-loading view: a pixel meerkat played frame-by-frame in the main
//! column while a reopened thread's chat snapshot is still in flight (the
//! fold holds no chat channel yet). The sprite is a 12×14 cell grid defined
//! as character rows; 4 distinct frames play in a 6-slot loop (idle, bob,
//! blink, bob, idle, tail flick), so the bob and idle slots each repeat.

use std::time::Duration;

use gpui::prelude::*;
use gpui::{Animation, AnimationExt as _, AnyElement, FontWeight, Hsla, div, px, rgb};
use gpui_component::{Theme, v_flex};

use crate::i18n;

/// Cell edge in px; the sprite is 12×14 cells.
const CELL: f32 = 10.0;
/// Sprite grid size.
const GRID_W: f32 = 12.0;
const GRID_H: f32 = 14.0;
/// Playback slots advanced per second.
const SLOTS_PER_SEC: f64 = 3.0;
/// Playback order over [`FRAMES`]: idle, bob, blink, bob, idle, tail flick.
const CYCLE: [usize; 6] = [0, 1, 2, 1, 0, 3];
/// A reopened thread whose chat snapshot never lands (host error, missing
/// session) must not pin the loading page forever: after this long the gate
/// self-clears and the hero screen returns.
pub(crate) const HISTORY_TIMEOUT: Duration = Duration::from_secs(10);

/// Sprite palette: outline, body, belly, eye patches, nose, tail.
const PALETTE: [(&str, u32); 6] = [
    ("o", 0x4A3626),
    ("b", 0xC89B6D),
    ("l", 0xE6C79C),
    ("E", 0x3B2A20),
    ("d", 0x3B2A20),
    ("t", 0x5A4532),
];

/// Frame 0: eyes open, tail down. Rows are exactly 12 chars wide; the other
/// frames are hand-derived variants of this grid.
const F0: [&str; 14] = [
    "..oo....oo..",
    ".oddo..oddo.",
    ".obboooobbo.",
    ".obbbbbbbbo.",
    "obbEEbbEEbo.",
    "obbEEbbEEbbo",
    ".obbbddbbbo.",
    "..obbbbbbo.t",
    "..obllllbo.t",
    "..obllllbo.t",
    "..obllllbo.t",
    "..obllllbo.t",
    "..obb..bbo.t",
    "..oo....oo..",
];

/// Bob: every row shifts one cell down and the feet row falls off the grid —
/// the sprite reads as a small hop rather than a rigid translation.
const F1: [&str; 14] = [
    "............",
    "..oo....oo..",
    ".oddo..oddo.",
    ".obboooobbo.",
    ".obbbbbbbbo.",
    "obbEEbbEEbo.",
    "obbEEbbEEbbo",
    ".obbbddbbbo.",
    "..obbbbbbo.t",
    "..obllllbo.t",
    "..obllllbo.t",
    "..obllllbo.t",
    "..obllllbo.t",
    "..obb..bbo.t",
];

/// Blink: the eye patches collapse to a thin outline line one row BELOW the
/// brow (where the patch bottom used to be).
const F2: [&str; 14] = [
    "..oo....oo..",
    ".oddo..oddo.",
    ".obboooobbo.",
    ".obbbbbbbbo.",
    "obbbbbbbbbbo",
    "obboobboobbo",
    ".obbbddbbbo.",
    "..obbbbbbo.t",
    "..obllllbo.t",
    "..obllllbo.t",
    "..obllllbo.t",
    "..obllllbo.t",
    "..obb..bbo.t",
    "..oo....oo..",
];

/// Tail flick: the tail cells lift to head height on the right edge.
const F3: [&str; 14] = [
    "..oo....oo..",
    ".oddo..oddo.",
    ".obboooobbot",
    ".obbbbbbbbot",
    "obbEEbbEEbot",
    "obbEEbbEEbbt",
    ".obbbddbbbo.",
    "..obbbbbbo..",
    "..obllllbo..",
    "..obllllbo..",
    "..obllllbo..",
    "..obllllbo..",
    "..obb..bbo..",
    "..oo....oo..",
];

const FRAMES: [[&str; 14]; 4] = [F0, F1, F2, F3];

fn palette_lookup(ch: char) -> Option<Hsla> {
    PALETTE
        .iter()
        .find(|(key, _)| key.starts_with(ch))
        .map(|(_, hex)| rgb(*hex).into())
}

/// Which CYCLE slot an animation delta falls in. Extracted for tests: the
/// slot mapping (including the wrap at delta 1.0) is the whole animation.
fn active_slot(delta: f32) -> usize {
    (delta * CYCLE.len() as f32).floor() as usize % CYCLE.len()
}

/// Precomputed solid cells of one frame: (x, y, color) in cell units. Parsing
/// runs once per page render, not once per frame layer.
fn frame_cells(grid: &[&str; 14]) -> Vec<(usize, usize, Hsla)> {
    let mut cells = Vec::new();
    for (y, row) in grid.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            if let Some(color) = palette_lookup(ch) {
                cells.push((x, y, color));
            }
        }
    }
    cells
}

/// One frame: a sprite-sized box with a div per solid pixel, filling the
/// (relative) parent it is mounted in.
fn frame_layer(cells: &[(usize, usize, Hsla)]) -> gpui::Div {
    let mut layer = div().absolute().inset_0();
    for &(x, y, color) in cells {
        layer = layer.child(
            div()
                .absolute()
                .left(px(x as f32 * CELL))
                .top(px(y as f32 * CELL))
                .size(px(CELL))
                .bg(color),
        );
    }
    layer
}

/// The full loading page: the animated meerkat, the heading, and the thread
/// id. No composer — the thread is not ready.
pub(crate) fn render_history_loading(theme: &Theme, thread_id: &str) -> AnyElement {
    // All four grids parse once here; the six layers share the results.
    let frame_table: Vec<Vec<(usize, usize, Hsla)>> = FRAMES.iter().map(frame_cells).collect();

    let mut sprite = div().relative().w(px(GRID_W * CELL)).h(px(GRID_H * CELL));
    for (i, &frame_ix) in CYCLE.iter().enumerate() {
        sprite = sprite.child(frame_layer(&frame_table[frame_ix]).with_animation(
            format!("history-loading-frame-{i}"),
            // gpui animations are oneshot by default; the loading page can
            // stay up arbitrarily long, so the loop must repeat.
            Animation::new(Duration::from_secs_f64(CYCLE.len() as f64 / SLOTS_PER_SEC)).repeat(),
            move |el, delta| {
                let active = active_slot(delta) == i;
                el.opacity(if active { 1.0 } else { 0.0 })
            },
        ));
    }

    v_flex()
        .flex_1()
        .w_full()
        .relative()
        .overflow_hidden()
        .debug_selector(|| "history-loading".into())
        .bg(theme.background)
        .child(
            v_flex()
                .absolute()
                .inset_0()
                .justify_center()
                .items_center()
                .gap_4()
                .child(sprite)
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(i18n::t("workspace-history-loading-heading")),
                )
                .child(
                    div()
                        .text_sm()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.muted_foreground)
                        .child(thread_id.to_string()),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_is_grid_width_and_uses_palette_chars() {
        for frame in &FRAMES {
            for row in frame {
                assert_eq!(row.chars().count(), 12);
                for ch in row.chars() {
                    assert!(ch == '.' || palette_lookup(ch).is_some(), "bad char {ch}");
                }
            }
        }
    }

    #[test]
    fn cycle_indices_are_valid_frames() {
        for &ix in &CYCLE {
            assert!(ix < FRAMES.len());
        }
    }

    #[test]
    fn slot_mapping_covers_the_cycle_and_wraps() {
        assert_eq!(active_slot(0.0), 0);
        assert_eq!(active_slot(0.2), 1);
        assert_eq!(active_slot(0.99), 5);
        // One-shot animations clamp delta to 1.0; the wrap must land on the
        // first slot, not panic on an out-of-range remainder.
        assert_eq!(active_slot(1.0), 0);
    }

    #[test]
    fn the_loop_animation_is_not_oneshot() {
        let animation =
            Animation::new(Duration::from_secs_f64(CYCLE.len() as f64 / SLOTS_PER_SEC)).repeat();
        assert!(!animation.oneshot);
    }

    #[test]
    fn blink_and_tail_flick_differ_from_idle() {
        assert_ne!(F0[5], F2[5]);
        assert_ne!(F0[10], F3[10]);
    }
}
