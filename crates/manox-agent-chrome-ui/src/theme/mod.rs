//! Chrome design tokens: the 2026-Light palette, the codicon table, and the
//! font-family constants. This module carries colors/fonts/icons only — no
//! components and no business state.

pub mod icons;
pub mod palette;

pub use icons::{Icon, icon};
pub use palette::*;

/// UI font family. gpui's platform alias `.SystemUIFont` resolves to the
/// system UI face (SF Pro / 苹方 on macOS) — the look the agents window was
/// calibrated against.
pub const FONT_UI: &str = ".SystemUIFont";
/// Monospace family — the embedded `sf-mono.ttf` registers under its own
/// family name `.SF NS Mono` (gpui's `add_fonts` uses the embedded name).
pub const FONT_MONO: &str = ".SF NS Mono";
/// Icon font family (the embedded codicon.ttf's family name).
pub const FONT_ICON: &str = "codicon";

/// Register the chrome fonts (codicon glyphs + SF Mono). Idempotent at the
/// text-system level; call once during app init, before the first chrome
/// frame renders, so the first paint already resolves to these faces.
///
/// The codicon.ttf copy carries the empty `m` glyph mapping — the macOS
/// text system silently refuses to load a font with no `m` glyph, which
/// turns every glyph into a tofu block, so do not replace it with a vanilla
/// codicon build.
pub fn register_fonts(cx: &mut gpui::App) {
    cx.text_system()
        .add_fonts(vec![
            std::borrow::Cow::Borrowed(include_bytes!("../../assets/fonts/codicon.ttf")),
            std::borrow::Cow::Borrowed(include_bytes!("../../assets/fonts/sf-mono.ttf")),
        ])
        .expect("failed to register chrome fonts");
}
