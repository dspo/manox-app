//! A running indicator, as one of the shapes this crate *offers*.
//!
//! Nothing here is wired into a component by default. A host's step row
//! takes its marker as a slot, and so does `Reasoning` its icon, so a host
//! picks whatever says "in flight" in its own product — this spinner, the
//! kit's rotating `Spinner`, a static glyph, or something of its own.
//! Shipping one ready-made indicator here is a convenience, not a default:
//! the components have no opinion about it.

use gpui::{
    ElementId, Entity, Hsla, IntoElement, ParentElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{Sizable, Size};

use crate::glyph_view::{GlyphSpec, SpinnerView};

/// Braille frames in animation order (classic 10-frame cycle).
pub(crate) const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The frame index at `cycle_ms` — a pure function of the shared clock's
/// time, so every glyph on screen derives the SAME frame from the SAME
/// beat (the "one fleet, one frame" guarantee, testable without a window).
pub(crate) fn frame_index(cycle_ms: f64) -> usize {
    let frame = ((cycle_ms % CYCLE_MS) / CYCLE_MS * FRAMES.len() as f64) as usize;
    frame.min(FRAMES.len() - 1)
}

/// One braille cycle, in milliseconds: 10 frames over 800 ms. The shared
/// clock samples this at 30 fps — finer than the native 80 ms/frame, so
/// each frame shows for its true duration.
const CYCLE_MS: f64 = 800.;

/// A text-based braille-dot spinner.
///
/// The value is a *spec*: mount it with [`BrailleSpinner::view`] to get an
/// entity view driven by the shared glyph clock — a tick re-renders the
/// spinner alone instead of notifying the containing view's whole subtree.
/// (There is no inline render: an animated glyph without its own view is
/// exactly the whole-window-rebuild failure mode #320 fixed.)
#[derive(Clone, Copy, PartialEq)]
pub struct BrailleSpinner {
    size: Size,
    color: Option<Hsla>,
}

impl Default for BrailleSpinner {
    fn default() -> Self {
        Self::new()
    }
}

impl BrailleSpinner {
    pub fn new() -> Self {
        Self {
            size: Size::Medium,
            color: None,
        }
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl Sizable for BrailleSpinner {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}

impl BrailleSpinner {
    /// The glyph text this spec shows at `cycle_ms` — the comparable form
    /// of [`Self::frame_at`], so tests can pin "same clock time, same frame
    /// everywhere" across specs that differ in every visual knob.
    pub(crate) fn frame_text(&self, cycle_ms: f64) -> &'static str {
        FRAMES[frame_index(cycle_ms)]
    }

    /// The frame this spec shows `cycle_ms` into the (looping) animation.
    /// `reduce_motion` renders `frame_at(0.)` — the static first frame, the
    /// same contract the retired `with_animation` path honoured.
    pub(crate) fn frame_at(&self, cycle_ms: f64) -> impl IntoElement {
        let text = self.frame_text(cycle_ms);
        let el = div();
        let el = match self.size {
            Size::XSmall => el.text_xs(),
            Size::Small => el.text_sm(),
            _ => el.text_base(),
        };
        el.when_some(self.color, |this, color| this.text_color(color))
            .child(gpui::SharedString::from(text))
    }

    /// Mount as an entity view driven by the shared glyph clock. `key` is
    /// scoped by the mounting element's id path — a per-spinner suffix is
    /// only needed when two glyphs mount under one id.
    pub fn view(
        self,
        key: impl Into<ElementId>,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> Entity<SpinnerView> {
        SpinnerView::mount(GlyphSpec::Braille(self), key, window, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two specs differing in every visual knob show the SAME frame at the
    /// same clock time, advance together one frame-period later, and wrap
    /// identically across the cycle — "one fleet, one frame" pinned at the
    /// spec level (frame derivation depends on the clock time alone).
    #[test]
    fn the_same_clock_time_yields_the_same_frame_everywhere() {
        let a = BrailleSpinner::new().xsmall();
        let b = BrailleSpinner::new().small().color(gpui::red());
        for ms in [0., 1., 100., 412.5, 799.9, 12345.6] {
            assert_eq!(a.frame_text(ms), b.frame_text(ms), "ms={ms}");
            assert_eq!(a.frame_text(ms), a.frame_text(ms + CYCLE_MS));
            assert_eq!(b.frame_text(ms), b.frame_text(ms + CYCLE_MS));
        }
        // One frame period on is the NEXT frame, not the same one.
        assert_ne!(a.frame_text(0.), a.frame_text(CYCLE_MS / 10.));
    }
}
