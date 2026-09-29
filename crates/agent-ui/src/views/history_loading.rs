//! History-loading view: the pixel tetromino rain shown in the main column
//! while a reopened thread's chat snapshot is still in flight (the fold holds
//! no chat channel yet). Geometry and palette are the `history-loading`
//! design spec: 20px cells, top/left highlight + bottom/right shadow for the
//! 8-bit bevel, quantized (grid-stepped) motion.

use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt as _, AnyElement, FontWeight, Hsla, black, div, px, relative, rgb,
};
use gpui_component::{Theme, v_flex};

use crate::i18n;

/// Cell edge in px; every offset in this view is a multiple of it.
const CELL: f32 = 20.0;
/// Pixel outline thickness around each cell.
const OUTLINE_W: f32 = 2.0;
/// Rain vertical step count: the fall is quantized to this many grid steps
/// so pieces drop cell-by-cell instead of sliding.
const RAIN_STEPS: f32 = 24.0;
/// How many pieces fall at once (spread across the width by the seeded RNG).
const RAIN_PIECES: usize = 8;

/// One tetromino: color plus the four cell offsets in its bounding box.
struct Piece {
    color: Hsla,
    cells: [(f32, f32); 4],
    cols: f32,
    rows: f32,
}

fn pieces() -> [Piece; 7] {
    let c = |hex: u32| -> Hsla { rgb(hex).into() };
    [
        Piece {
            color: c(0x4FB3C6),
            cells: [(0., 0.), (1., 0.), (2., 0.), (3., 0.)],
            cols: 4.,
            rows: 1.,
        },
        Piece {
            color: c(0xD9B44A),
            cells: [(0., 0.), (1., 0.), (0., 1.), (1., 1.)],
            cols: 2.,
            rows: 2.,
        },
        Piece {
            color: c(0x9B7FC7),
            cells: [(1., 0.), (0., 1.), (1., 1.), (2., 1.)],
            cols: 3.,
            rows: 2.,
        },
        Piece {
            color: c(0x7FB069),
            cells: [(1., 0.), (2., 0.), (0., 1.), (1., 1.)],
            cols: 3.,
            rows: 2.,
        },
        Piece {
            color: c(0xCC6B6B),
            cells: [(0., 0.), (1., 0.), (1., 1.), (2., 1.)],
            cols: 3.,
            rows: 2.,
        },
        Piece {
            color: c(0x6B8FD4),
            cells: [(0., 0.), (0., 1.), (1., 1.), (2., 1.)],
            cols: 3.,
            rows: 2.,
        },
        Piece {
            color: c(0xD4926B),
            cells: [(2., 0.), (0., 1.), (1., 1.), (2., 1.)],
            cols: 3.,
            rows: 2.,
        },
    ]
}

/// One flat pixel cell at the given local (cell-grid) offset: solid fill
/// plus a darker pixel outline — no gloss, no bevel. Cells share edges, so
/// a tetromino reads as one chunky silhouette.
fn cell(color: Hsla, x: f32, y: f32) -> gpui::Div {
    div()
        .absolute()
        .left(px(x * CELL))
        .top(px(y * CELL))
        .size(px(CELL))
        .bg(color)
        .border(px(OUTLINE_W))
        .border_color(black().opacity(0.55))
}

/// A whole tetromino as a fixed-size box with absolutely placed cells.
fn piece_box(piece: &Piece) -> gpui::Div {
    let mut container = div()
        .relative()
        .w(px(piece.cols * CELL))
        .h(px(piece.rows * CELL));
    for &(x, y) in &piece.cells {
        container = container.child(cell(piece.color, x, y));
    }
    container
}

/// FNV-1a over the thread id: the deterministic seed for the rain layout, so
/// the same thread always opens with the same rain.
fn seed_from(thread_id: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in thread_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 11
    }

    fn unit(&mut self) -> f64 {
        (self.next() % 10_000) as f64 / 10_000.0
    }
}

/// One falling piece. The animation closure re-positions the box each frame;
/// the phase bakes in a staggered start so the pieces don't move in lockstep.
fn falling_piece(
    id: usize,
    piece: &Piece,
    left: f32,
    dur_secs: u64,
    phase: f32,
) -> impl IntoElement {
    div()
        .absolute()
        .left(relative(left))
        .size_full()
        .with_animation(
            format!("history-loading-rain-{id}"),
            Animation::new(Duration::from_secs(dur_secs)),
            move |el, delta| {
                // Wrap past both edges (one piece height above, one below) so
                // the fall is gapless, and quantize to the step grid.
                let t = (delta + phase) % 1.0;
                let stepped = (t * RAIN_STEPS).floor() / RAIN_STEPS;
                el.top(relative(-0.2 + stepped * 1.3))
            },
        )
        .child(piece_box(piece))
}

/// Spinner: four accent cells cycling clockwise around a diamond, 90° per
/// step (the design's discrete rotation — positions jump, never tween).
fn spinner(theme: &Theme) -> gpui::Div {
    let accent = theme.colors.accent;
    let centers = [(0.0f32, -1.0f32), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)];
    let mut ring = div().relative().w(px(CELL * 4.0)).h(px(CELL * 4.0));
    for (i, &(cx, cy)) in centers.iter().enumerate() {
        ring = ring.child(cell(accent, 0.0, 0.0).with_animation(
            format!("history-loading-spinner-{i}"),
            Animation::new(Duration::from_millis(1600)),
            move |el, delta| {
                let step = (delta * 4.0).floor() as i32 % 4;
                // Rotate the cell center by 90° per step: (x, y) → (-y, x).
                let (mut x, mut y) = (cx, cy);
                for _ in 0..step {
                    let next = (-y, x);
                    x = next.0;
                    y = next.1;
                }
                // Cell top-left from the 4×4 box's corner: the ring center
                // sits at 1.5 cells in, and the center offset needs half a
                // cell subtracted to land on the top-left.
                el.left(px((x + 1.5) * CELL)).top(px((y + 1.5) * CELL))
            },
        ));
    }
    ring
}

/// Settled pieces along the bottom edge: the terrain the rain is piling onto
/// (full opacity — the strongest pixel anchor on the page). Table of
/// `(piece index, grid x of the piece's box, grid y)`; y=0 is the top row.
fn terrain() -> gpui::Div {
    let catalog = pieces();
    let placed: [(usize, f32, f32); 5] = [
        (4, 0., 1.),  // Z
        (5, 4., 1.),  // J
        (3, 8., 1.),  // S
        (1, 12., 1.), // O
        (2, 16., 1.), // T
    ];
    let mut ground = div().relative().w(px(CELL * 19.0)).h(px(CELL * 2.0));
    for &(index, bx, by) in &placed {
        let piece = &catalog[index];
        for &(dx, dy) in &piece.cells {
            ground = ground.child(cell(piece.color, bx + dx, by + dy));
        }
    }
    ground
}

/// The full loading page: rain backdrop, settled terrain, centered pixel
/// spinner + heading + thread id. No composer — the thread is not ready.
pub(crate) fn render_history_loading(theme: &Theme, thread_id: &str) -> AnyElement {
    let mut rng = Lcg(seed_from(thread_id));
    let catalog = pieces();
    let mut rain = div().absolute().inset_0().opacity(0.22);
    for i in 0..RAIN_PIECES {
        let piece = &catalog[(rng.next() as usize) % catalog.len()];
        let left = (i as f32 + rng.unit() as f32) / RAIN_PIECES as f32;
        let dur = 6 + rng.next() % 6; // 6–11s
        let phase = rng.unit() as f32;
        rain = rain.child(falling_piece(i, piece, left, dur, phase));
    }

    v_flex()
        .flex_1()
        .w_full()
        .relative()
        .overflow_hidden()
        .bg(theme.background)
        .child(rain)
        .child(
            v_flex()
                .absolute()
                .inset_0()
                .justify_end()
                .items_center()
                .pb_6()
                .child(terrain()),
        )
        .child(
            v_flex()
                .absolute()
                .inset_0()
                .justify_center()
                .items_center()
                .gap_4()
                .child(spinner(theme))
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
    fn seed_is_stable_per_thread() {
        assert_eq!(seed_from("t_abc"), seed_from("t_abc"));
        assert_ne!(seed_from("t_abc"), seed_from("t_abd"));
    }

    #[test]
    fn rain_layout_is_deterministic() {
        let layout = |id: &str| {
            let mut rng = Lcg(seed_from(id));
            (0..RAIN_PIECES)
                .map(|_| (rng.next(), rng.unit().to_bits(), rng.unit().to_bits()))
                .collect::<Vec<_>>()
        };
        assert_eq!(layout("t_9f3a1c"), layout("t_9f3a1c"));
        assert_ne!(layout("t_9f3a1c"), layout("t_other"));
    }

    #[test]
    fn every_tetromino_has_four_cells_inside_its_box() {
        for piece in &pieces() {
            assert_eq!(piece.cells.len(), 4);
            for &(x, y) in &piece.cells {
                assert!(x >= 0.0 && x < piece.cols);
                assert!(y >= 0.0 && y < piece.rows);
            }
        }
    }
}
