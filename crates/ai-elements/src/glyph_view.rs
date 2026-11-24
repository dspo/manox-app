//! The entity mount a spinner glyph rides: one small view per animated
//! spinner, observed onto the shared [`TickClock`].
//!
//! Mounts go through window-scoped keyed state, so re-rendering the host
//! re-fetches the same entity instead of churning a fresh one per frame, and
//! unmounting the host (a scrolled-out row, a settled tool call) prunes the
//! state — the observer detaches and the clock loses a consumer.
//!
//! What a tick does NOT do is spare the ancestors: `mark_view_dirty` walks
//! the glyph's whole view chain, and the keyed-state bridge notifies the
//! mounting view as well. The win is that the beat count is one constant
//! rate for the whole app and the ancestors' renders are cheap — they emit
//! cached child entities (see `SidebarLens`, `ActivityStepView`) whose
//! subtrees replay from the view cache. A `.view()` mount is not free; it
//! is constant-rate plus cheap re-emits.

use std::fmt::Debug;

use gpui::{App, ElementId, Entity, Render, Window, prelude::*};

use crate::pixel_spinner::PixelSpinner;
use crate::spinner::BrailleSpinner;
use crate::tick_clock::{TickClock, glyph_mounted, glyph_released};

/// Which glyph a [`SpinnerView`] draws.
#[derive(Clone, PartialEq)]
pub(crate) enum GlyphSpec {
    Braille(BrailleSpinner),
    Pixel(PixelSpinner),
}

/// One animated spinner as its own entity: the tick re-renders this view
/// (ancestors re-emit, cheaply — see the module doc).
pub struct SpinnerView {
    spec: GlyphSpec,
    clock: Entity<TickClock>,
}

impl Drop for SpinnerView {
    fn drop(&mut self) {
        glyph_released();
    }
}

impl SpinnerView {
    /// Mount the glyph as an entity under `key` (scoped by the mounting
    /// element's id path). The latest spec wins: a re-mount with a changed
    /// spec (a theme swap) updates the cached view in place.
    pub(crate) fn mount(
        spec: GlyphSpec,
        key: impl Into<ElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let clock = TickClock::shared(cx);
        clock.update(cx, |clock, _| clock.watch(window));
        let view = window.use_keyed_state((key.into(), "spinner-clock-view"), cx, {
            let clock = clock.clone();
            let spec = spec.clone();
            |_, cx| {
                cx.observe(&clock, |_, _, cx| cx.notify()).detach();
                glyph_mounted();
                Self { spec, clock }
            }
        });
        view.update(cx, |view, cx| {
            if view.spec != spec {
                view.spec = spec;
                cx.notify();
            }
        });
        view
    }
}

impl Render for SpinnerView {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        // reduce_motion: the static frame and no clock reliance — the same
        // contract the retired `with_animation` path honoured (the loop
        // above also stops beating, this arm covers glyphs mounted mid-run).
        if cx.reduce_motion() {
            return match &self.spec {
                GlyphSpec::Braille(spec) => spec.frame_at(0.).into_any_element(),
                GlyphSpec::Pixel(spec) => spec.hold_frame(),
            };
        }
        let ms = self.clock.read(cx).ms();
        match &self.spec {
            GlyphSpec::Braille(spec) => spec.frame_at(ms).into_any_element(),
            GlyphSpec::Pixel(spec) => spec.frame_at(ms),
        }
    }
}

impl Debug for SpinnerView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.spec {
            GlyphSpec::Braille(_) => f.write_str("SpinnerView(braille)"),
            GlyphSpec::Pixel(_) => f.write_str("SpinnerView(pixel)"),
        }
    }
}
