//! History-loading view: a pixel meerkat played frame-by-frame in the main
//! column while a reopened thread's chat snapshot is still in flight (the
//! fold holds no chat channel yet). The sprite is a 12×14 cell grid defined
//! as character rows; the 6-frame cycle bobs, blinks, and flicks its tail.

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
/// Frames per second of the cycle.
const FPS: f64 = 3.0;
/// Playback order: idle, bob, blink, bob, idle, tail-flick.
const CYCLE: [usize; 6] = [0, 1, 2, 1, 0, 3];

/// Sprite palette: outline, body, belly, eye patches, features, tail.
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

/// Bob: the whole sprite sits one cell lower — the idle "breathing" frame.
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

/// Blink: the eye patches close to a thin outline line at the brow row.
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
        .find(|(key, _)| key.chars().next() == Some(ch))
        .map(|(_, hex)| rgb(*hex).into())
}

/// One frame: a fixed-size box with a div per solid pixel.
fn frame_layer(grid: &[&str; 14]) -> gpui::Div {
    let mut layer = div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(div().relative().w(px(GRID_W * CELL)).h(px(GRID_H * CELL)));
    for (y, row) in grid.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            if let Some(color) = palette_lookup(ch) {
                layer = layer.child(
                    div()
                        .absolute()
                        .left(px(x as f32 * CELL))
                        .top(px(y as f32 * CELL))
                        .size(px(CELL))
                        .bg(color),
                );
            }
        }
    }
    layer
}

/// The full loading page: the animated meerkat, the heading, and the thread
/// id. No composer — the thread is not ready.
pub(crate) fn render_history_loading(theme: &Theme, thread_id: &str) -> AnyElement {
    let mut sprite = div().relative().w(px(GRID_W * CELL)).h(px(GRID_H * CELL));
    for (i, &frame_ix) in CYCLE.iter().enumerate() {
        let frames_total = CYCLE.len() as f32;
        sprite = sprite.child(frame_layer(&FRAMES[frame_ix]).with_animation(
            format!("history-loading-frame-{i}"),
            Animation::new(Duration::from_secs_f64(CYCLE.len() as f64 / FPS)),
            move |el, delta| {
                let active = (delta * frames_total).floor() as usize % CYCLE.len() == i;
                el.opacity(if active { 1.0 } else { 0.0 })
            },
        ));
    }

    v_flex()
        .flex_1()
        .w_full()
        .relative()
        .overflow_hidden()
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
                        .child(i18n::t("history-loading-heading")),
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
            assert_eq!(frame.len(), 14);
            for row in frame {
                assert_eq!(row.chars().count(), 12);
                for ch in row.chars() {
                    assert!(ch == '.' || palette_lookup(ch).is_some(), "bad char {ch}");
                }
            }
        }
    }

    #[test]
    fn every_frame_has_a_nose() {
        for frame in &FRAMES {
            let all: String = frame.concat();
            assert!(all.contains('d'), "nose missing");
        }
    }

    #[test]
    fn cycle_indices_are_valid_frames() {
        assert_eq!(CYCLE.len(), 6);
        for &ix in &CYCLE {
            assert!(ix < FRAMES.len());
        }
    }

    #[test]
    fn blink_and_tail_flick_differ_from_idle() {
        assert_ne!(F0[5], F2[5]);
        assert_ne!(F0[10], F3[10]);
    }
}
