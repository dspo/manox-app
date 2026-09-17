//! A running indicator, as one of the shapes this crate *offers*.
//!
//! Nothing here is wired into a component by default. `ChainOfThoughtStep` takes
//! its marker as a slot, and so does `Reasoning` its icon, so a host picks
//! whatever says "in flight" in its own product — this spinner, the kit's
//! rotating `Spinner`, a static glyph, or something of its own. Shipping one
//! ready-made indicator here is a convenience, not a default: the components
//! have no opinion about it.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, App, Hsla, IntoElement, ParentElement, RenderOnce, SharedString,
    Styled as _, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{Sizable, Size};

/// Braille frames in animation order (classic 10-frame cycle).
const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// A text-based braille-dot spinner.
#[derive(IntoElement)]
pub struct BrailleSpinner {
    size: Size,
    speed: Duration,
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
            speed: Duration::from_millis(800),
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

impl RenderOnce for BrailleSpinner {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let el = div();
        let el = match self.size {
            Size::XSmall => el.text_xs(),
            Size::Small => el.text_sm(),
            _ => el.text_base(),
        };
        let frame_count = FRAMES.len() as f32;

        el.when_some(self.color, |this, color| this.text_color(color))
            .with_animation(
                "braille-spinner",
                Animation::new(self.speed).repeat(),
                move |this, delta| {
                    let frame = ((delta * frame_count) as usize).min(FRAMES.len() - 1);
                    this.child(SharedString::from(FRAMES[frame]))
                },
            )
            .into_element()
    }
}
