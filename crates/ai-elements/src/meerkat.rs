//! The history-loading sprite: a pixel meerkat played frame-by-frame while a
//! reopened thread's chat snapshot is still in flight.
//!
//! The sprite is a 12×14 cell grid defined as character rows; 4 distinct
//! frames play in a 6-slot loop (idle, bob, blink, bob, idle, tail flick),
//! so the bob and idle slots each repeat. Everything is divs under one
//! `Animation` clock, the same approach as [`crate::PixelSpinner`]: cycle
//! time picks the slot, the slot picks the frame, and only the active
//! frame's cells render — no stacked layers toggling opacity.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, App, Hsla, IntoElement, ParentElement as _, RenderOnce,
    Styled as _, Window, div, px, rgb,
};

/// Cell edge in px at the loading page's native scale.
pub const NATIVE_CELL: f32 = 10.0;
/// Sprite grid size, in cells.
const GRID_W: f32 = 12.0;
const GRID_H: f32 = 14.0;
/// Playback slots advanced per second at speed 1.
const SLOTS_PER_SEC: f32 = 3.0;
/// Playback order over [`FRAMES`]: idle, bob, blink, bob, idle, tail flick.
const CYCLE: [usize; 6] = [0, 1, 2, 1, 0, 3];

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

/// Frame names, in [`FRAMES`] order — the gallery labels, and what a freeze
/// index means.
pub const FRAME_NAMES: [&str; 4] = ["idle", "bob", "blink", "tail flick"];

fn palette_lookup(ch: char) -> Option<Hsla> {
    PALETTE
        .iter()
        .find(|(key, _)| key.starts_with(ch))
        .map(|(_, hex)| rgb(*hex).into())
}

/// Which CYCLE slot an animation delta falls in. The slot mapping (including
/// the wrap at delta 1.0) is the whole animation.
fn active_slot(delta: f32) -> usize {
    (delta * CYCLE.len() as f32).floor() as usize % CYCLE.len()
}

/// Precomputed solid cells of one frame: (x, y, color) in cell units.
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

/// The frame-loop animation: one full CYCLE per run at the given rate
/// multiplier, repeating. The tests assert on THIS builder, so dropping
/// `.repeat()` fails the build's regression pin rather than just changing
/// runtime behavior.
fn loop_animation(rate: f32) -> Animation {
    // gpui animations are oneshot by default; the loading page can stay up
    // arbitrarily long, so the loop must repeat.
    Animation::new(Duration::from_secs_f64(
        CYCLE.len() as f64 / (SLOTS_PER_SEC * rate) as f64,
    ))
    .repeat()
}

/// The pixel meerkat: the conversation-loading glyph.
///
/// `cell` scales the sprite ( [`NATIVE_CELL`] = the loading page's size),
/// `speed` multiplies the playback rate (1 = the page's 3 slots/s), and
/// `freeze` pins one frame statically for inspection.
#[derive(IntoElement)]
pub struct Meerkat {
    cell: f32,
    speed: f32,
    frozen: Option<usize>,
}

impl Default for Meerkat {
    fn default() -> Self {
        Self::new()
    }
}

impl Meerkat {
    pub fn new() -> Self {
        Self {
            cell: NATIVE_CELL,
            speed: 1.,
            frozen: None,
        }
    }

    /// Edge of one sprite cell, in px.
    pub fn cell(mut self, cell: f32) -> Self {
        self.cell = cell;
        self
    }

    /// Playback rate multiplier, 1 = the loading page's native 3 slots/s.
    pub fn speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }

    /// Render one frame statically instead of playing (out of range wraps).
    pub fn freeze(mut self, frame: usize) -> Self {
        self.frozen = Some(frame);
        self
    }
}

impl RenderOnce for Meerkat {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let cell = if self.cell > 0. {
            self.cell
        } else {
            NATIVE_CELL
        };
        let rate = if self.speed > 0. { self.speed } else { 1. };
        // Parsed once per render; the animation closure only indexes it.
        let table: Vec<Vec<(usize, usize, Hsla)>> = FRAMES.iter().map(frame_cells).collect();
        let base = div().relative().w(px(GRID_W * cell)).h(px(GRID_H * cell));
        match self.frozen {
            Some(ix) => base
                .children(
                    frame_cells_of(&table, ix)
                        .iter()
                        .map(|&(x, y, color)| sprite_cell(x, y, color, cell)),
                )
                .into_any_element(),
            None => base
                .with_animation("meerkat", loop_animation(rate), move |el, t| {
                    let frame = &table[CYCLE[active_slot(t)]];
                    el.children(
                        frame
                            .iter()
                            .map(|&(x, y, color)| sprite_cell(x, y, color, cell)),
                    )
                })
                .into_any_element(),
        }
    }
}

/// One frame's cells, freeze index wrapping modulo the frame count.
fn frame_cells_of(table: &[Vec<(usize, usize, Hsla)>], ix: usize) -> &Vec<(usize, usize, Hsla)> {
    &table[ix % FRAMES.len()]
}

/// One solid sprite pixel: a div at its cell position.
fn sprite_cell(x: usize, y: usize, color: Hsla, cell: f32) -> impl IntoElement {
    div()
        .absolute()
        .left(px(x as f32 * cell))
        .top(px(y as f32 * cell))
        .size(px(cell))
        .bg(color)
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
        // Asserts on the same builder the component mounts — dropping
        // `.repeat()` from `loop_animation` fails here.
        assert!(!loop_animation(1.).oneshot);
    }

    #[test]
    fn blink_and_tail_flick_differ_from_idle() {
        assert_ne!(F0[5], F2[5]);
        assert_ne!(F0[10], F3[10]);
    }

    #[test]
    fn freeze_wraps_out_of_range_indices() {
        let table = vec![
            frame_cells(&F0),
            frame_cells(&F1),
            frame_cells(&F2),
            frame_cells(&F3),
        ];
        assert_eq!(frame_cells_of(&table, 3), &frame_cells(&F3));
        // 7 % 4 wraps onto the tail flick again.
        assert_eq!(frame_cells_of(&table, 7), &frame_cells(&F3));
        assert_eq!(frame_cells_of(&table, 4), &frame_cells(&F0));
    }
}
