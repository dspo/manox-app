//! VS Code's session-status pixel spinner, on gpui primitives.
//!
//! Two variants, mirroring `pixelSpinner.css`:
//!
//! - `Grid` — six 2px dots in a 2×3 layout; each rises from above its cell,
//!   holds lit, then falls out of the 16px frame: the "working" glyph.
//! - `Ring` — the same six dots held in place at low opacity while a
//!   highlight sweeps clockwise: the "waiting for input" glyph.
//!
//! Everything is divs under one `Animation` clock — gpui rasterizes SVG
//! statically, so the motion lives in per-frame arithmetic. Each moving
//! keyframe segment is snapped by `steps(6, jump-none)` (grid) or
//! `steps(4, jump-none)` (ring) exactly as the CSS does; that snap is the
//! whole look, and `smooth()` trading it for linear interpolation is the
//! contrast that proves it.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, App, Hsla, IntoElement, ParentElement as _, RenderOnce,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

/// Native glyph geometry, in CSS pixels at zoom 1: a 16px frame holding a
/// 2×3 grid of 2px dots on a 4px pitch, content centered ((16−6)/2, (16−10)/2).
const FRAME: f32 = 16.;
const DOT: f32 = 2.;
const PITCH: f32 = 4.;
const GRID_X: f32 = 5.;
const GRID_Y: f32 = 3.;
/// A dot's travel: it enters from above its cell and exits below it, both
/// outside the frame — the container's clip is part of the design.
const RISE: f32 = -4.;
const FALL: f32 = 7.;

/// One grid cycle, in milliseconds (`monaco-pixel-spinner-dot-cycle`).
const GRID_PERIOD_MS: f32 = 1820.;
/// One ring cycle, in milliseconds (`monaco-pixel-spinner-ring-pulse`).
const RING_PERIOD_MS: f32 = 1200.;
/// Where the rise ends on every grid variant: 9.34% of the cycle.
const RISE_END: f32 = 0.0934;
/// A ring dot's resting opacity; the pulse lifts it to 1 and lets it decay.
const RING_BASE_ALPHA: f32 = 0.25;

/// Per-dot start delay (ms) and hold variant, in DOM order — the stagger
/// table `pixelSpinner.css` assigns via `nth-child`. The delays are phase
/// offsets in steady state, so the conveyor has no dead frame where every
/// dot is dark.
const GRID_DOTS: [(f32, GridHold); 6] = [
    (520., GridHold::Cycle),
    (650., GridHold::Cycle),
    (260., GridHold::Cycle),
    (390., GridHold::Cycle),
    (0., GridHold::Long),
    (130., GridHold::Short),
];

/// Per-dot start delay (ms), in DOM order: clockwise around the grid.
const RING_DOTS: [f32; 6] = [0., 200., 1000., 400., 800., 600.];

/// How long a grid dot stays lit before falling — the CSS keyframe variants
/// (`-cycle`, `-cycle-long`, `-cycle-short`). The spread is what keeps the
/// cascade from reading as mechanical.
#[derive(Clone, Copy)]
enum GridHold {
    Cycle,
    Long,
    Short,
}

impl GridHold {
    /// (hold end, fall end) as cycle fractions.
    fn ends(self) -> (f32, f32) {
        match self {
            Self::Cycle => (0.5714, 0.6648),
            Self::Long => (0.6429, 0.7363),
            Self::Short => (0.50, 0.5934),
        }
    }
}

/// Which glyph to draw: the falling cascade (working) or the clockwise
/// highlight (waiting for input).
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum PixelSpinnerVariant {
    #[default]
    Grid,
    Ring,
}

/// The pixel spinner: six 2px dots under one animation clock.
///
/// The closure folds normalized cycle time through each dot's delay and the
/// keyframe tables above; dot color carries the phase's opacity in its alpha.
/// `zoom` scales the whole glyph (1 = the native 16px), `speed` scales the
/// clock rate (1 = native; 0.25 = quarter speed for reading the hops).
#[derive(IntoElement)]
pub struct PixelSpinner {
    variant: PixelSpinnerVariant,
    color: Hsla,
    zoom: f32,
    speed: f32,
    smooth: bool,
    sharp: bool,
}

impl Default for PixelSpinner {
    fn default() -> Self {
        Self::new()
    }
}

impl PixelSpinner {
    pub fn new() -> Self {
        Self {
            variant: PixelSpinnerVariant::default(),
            color: gpui::blue(),
            zoom: 1.,
            speed: 1.,
            smooth: false,
            sharp: false,
        }
    }

    pub fn variant(mut self, variant: PixelSpinnerVariant) -> Self {
        self.variant = variant;
        self
    }

    /// The dot color; the animation only modulates its alpha.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = color;
        self
    }

    /// Scale of the whole glyph, 1 = the native 16px frame.
    pub fn zoom(mut self, zoom: f32) -> Self {
        self.zoom = zoom;
        self
    }

    /// Clock rate multiplier, 1 = native; below 1 slows the cycle down.
    pub fn speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }

    /// Linear interpolation instead of the steps snap — the "lost the pixel
    /// feel" arm of the comparison.
    pub fn smooth(mut self) -> Self {
        self.smooth = true;
        self
    }

    /// Square dots: drop the 50% radius that makes 2px dots read as pixels.
    pub fn sharp(mut self) -> Self {
        self.sharp = true;
        self
    }
}

impl RenderOnce for PixelSpinner {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let zoom = if self.zoom > 0. { self.zoom } else { 1. };
        let rate = if self.speed > 0. { self.speed } else { 1. };
        let period_ms = match self.variant {
            PixelSpinnerVariant::Grid => GRID_PERIOD_MS,
            PixelSpinnerVariant::Ring => RING_PERIOD_MS,
        } / rate;
        div()
            .size(px(FRAME * zoom))
            .relative()
            .overflow_hidden()
            .with_animation(
                "pixel-spinner",
                Animation::new(Duration::from_millis(period_ms as u64)).repeat(),
                move |el, t| {
                    let now_ms = t * period_ms;
                    el.children((0..6usize).map(|i| {
                        let (col, row) = ((i % 2) as f32, (i / 2) as f32);
                        let (dy, alpha) = match self.variant {
                            PixelSpinnerVariant::Grid => {
                                let (delay, hold) = GRID_DOTS[i];
                                let u =
                                    (now_ms - delay).rem_euclid(GRID_PERIOD_MS) / GRID_PERIOD_MS;
                                grid_phase(u, hold, self.smooth)
                            }
                            PixelSpinnerVariant::Ring => {
                                let u = (now_ms - RING_DOTS[i]).rem_euclid(RING_PERIOD_MS)
                                    / RING_PERIOD_MS;
                                (0., ring_alpha(u, self.smooth))
                            }
                        };
                        let mut dot_color = self.color;
                        dot_color.a *= alpha;
                        div()
                            .absolute()
                            .left(px((GRID_X + col * PITCH) * zoom))
                            .top(px((GRID_Y + row * PITCH + dy) * zoom))
                            .size(px(DOT * zoom))
                            .when(!self.sharp, |dot| dot.rounded_full())
                            .bg(dot_color)
                    }))
                },
            )
    }
}

/// One grid dot's keyframes at cycle phase `u ∈ [0,1)`: rise from `RISE`
/// (transparent), hold lit in the cell, fall to `FALL` fading out — the
/// `-cycle` family of CSS keyframes. Each moving segment snaps through
/// `steps(6, jump-none)` unless `smooth`.
fn grid_phase(u: f32, hold: GridHold, smooth: bool) -> (f32, f32) {
    let (hold_end, fall_end) = hold.ends();
    if u < RISE_END {
        let f = segment(u / RISE_END, 6., smooth);
        (RISE + (0. - RISE) * f, f)
    } else if u < hold_end {
        (0., 1.)
    } else if u < fall_end {
        let f = segment((u - hold_end) / (fall_end - hold_end), 6., smooth);
        (FALL * f, 1. - f)
    } else {
        (FALL, 0.)
    }
}

/// A ring dot's opacity at phase `u`: steps up to full over the first sixth
/// of the cycle, then decays back to rest over the remainder.
fn ring_alpha(u: f32, smooth: bool) -> f32 {
    let peak = 1. / 6.;
    if u < peak {
        RING_BASE_ALPHA + (1. - RING_BASE_ALPHA) * segment(u / peak, 4., smooth)
    } else {
        1. - (1. - RING_BASE_ALPHA) * segment((u - peak) / (1. - peak), 4., smooth)
    }
}

/// `steps(n, jump-none)`: a segment's progress snapped to n stops spanning
/// [0, 1] with no interpolation — the hop that makes it read as pixels.
fn segment(progress: f32, stops: f32, smooth: bool) -> f32 {
    if smooth {
        progress
    } else {
        (progress * stops).floor().min(stops - 1.) / (stops - 1.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `steps(n, jump-none)` per the CSS spec: n stops evenly spaced across
    /// [0, 1], both endpoints included, the last one reached before the
    /// segment ends and held there.
    #[test]
    fn steps_snap_holds_n_stops_spanning_zero_to_one() {
        let expected = [0., 0.2, 0.4, 0.6, 0.8, 1.];
        for (k, &want) in expected.iter().enumerate() {
            let got = segment(k as f32 / 6., 6., false);
            assert!((got - want).abs() < 1e-6, "stop {k}: {got} != {want}");
        }
        assert!((segment(0.999, 6., false) - 1.).abs() < 1e-6);
        assert!((segment(1., 6., false) - 1.).abs() < 1e-6);
    }

    #[test]
    fn smooth_replaces_the_snap_with_linear() {
        assert!((segment(0.25, 6., true) - 0.25).abs() < 1e-6);
        assert!((segment(0.25, 6., false) - 0.2).abs() < 1e-6);
    }

    /// The `-cycle` keyframes: rise from −4 (transparent), hold lit, fall to
    /// +7 fading out. The rise's last stop reaches the cell before the hold
    /// begins, and the fall's first stop leaves it — no jump at either edge.
    #[test]
    fn grid_dot_rises_holds_and_falls() {
        let (dy, alpha) = grid_phase(0., GridHold::Cycle, false);
        assert!(dy == RISE && alpha == 0.);
        let (dy, alpha) = grid_phase(RISE_END * 0.99, GridHold::Cycle, false);
        assert!(dy.abs() < 1e-6 && (alpha - 1.).abs() < 1e-6);
        assert_eq!(grid_phase(0.3, GridHold::Cycle, false), (0., 1.));
        let (dy, alpha) = grid_phase(0.5714, GridHold::Cycle, false);
        assert!(dy.abs() < 1e-6 && (alpha - 1.).abs() < 1e-6);
        let (dy, alpha) = grid_phase(0.62, GridHold::Cycle, false);
        assert!((dy - 4.2).abs() < 1e-5 && (alpha - 0.4).abs() < 1e-6);
        assert_eq!(grid_phase(0.6648, GridHold::Cycle, false), (FALL, 0.));
        assert_eq!(grid_phase(0.999, GridHold::Cycle, false), (FALL, 0.));
    }

    /// The long variant holds past where the cycle variant starts falling;
    /// the short one falls before it.
    #[test]
    fn hold_variants_shift_the_fall() {
        assert_eq!(grid_phase(0.60, GridHold::Long, false), (0., 1.));
        let (dy, _) = grid_phase(0.60, GridHold::Cycle, false);
        assert!(dy > 0.);
        let (dy, _) = grid_phase(0.52, GridHold::Short, false);
        assert!(dy > 0.);
        assert_eq!(grid_phase(0.52, GridHold::Cycle, false), (0., 1.));
    }

    /// A ring dot rests at quarter opacity, peaks at full just before the
    /// first sixth of the cycle, and snaps back down through steps(4).
    #[test]
    fn ring_rests_peaks_and_decays() {
        assert!((ring_alpha(0., false) - RING_BASE_ALPHA).abs() < 1e-6);
        assert!((ring_alpha(1. / 6. - 1e-6, false) - 1.).abs() < 1e-6);
        assert!((ring_alpha(0.5, false) - 0.75).abs() < 1e-6);
        assert!((ring_alpha(1. - 1e-6, false) - RING_BASE_ALPHA).abs() < 1e-6);
    }

    /// Both cycle edges are invisible (grid) or at rest (ring), so the wrap
    /// from one cycle into the next never shows a pop.
    #[test]
    fn cycle_edges_are_seamless() {
        assert_eq!(grid_phase(1. - 1e-6, GridHold::Cycle, false), (FALL, 0.));
        assert_eq!(grid_phase(0., GridHold::Cycle, false), (RISE, 0.));
        let tail = ring_alpha(1. - 1e-6, false);
        let head = ring_alpha(0., false);
        assert!((tail - head).abs() < 1e-6);
    }
}
