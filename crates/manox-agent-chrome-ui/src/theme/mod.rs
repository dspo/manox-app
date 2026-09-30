//! Chrome design tokens: the 2026-Light palette, the SVG icon table, and the
//! font-family constants. This module carries colors/fonts/icons only — no
//! components and no business state.

pub mod icons;
pub mod palette;

pub use icons::{IconAsset, icon};
pub use palette::*;

/// UI font family. gpui's platform alias `.SystemUIFont` resolves to the
/// system UI face (SF Pro / 苹方 on macOS) — the look the agents window was
/// calibrated against.
pub const FONT_UI: &str = ".SystemUIFont";
/// Monospace family — the embedded `sf-mono.ttf` registers under its own
/// family name `.SF NS Mono` (gpui's `add_fonts` uses the embedded name).
pub const FONT_MONO: &str = ".SF NS Mono";

/// Register the chrome fonts (SF Mono; icons are SVG assets and need no
/// font). Call once during app init, before the first chrome frame renders,
/// so the first paint already resolves to this face.
///
/// Embedded-font trap: the macOS text system silently refuses to load a font
/// with no `m` glyph (returns `Ok`, every glyph renders as tofu) — the
/// retired codicon.ttf needed an empty-`m` patch for exactly this. Any
/// replacement embedded font must carry an `m` glyph.
pub fn register_fonts(cx: &mut gpui::App) {
    cx.text_system()
        .add_fonts(vec![std::borrow::Cow::Borrowed(include_bytes!(
            "../../assets/fonts/sf-mono.ttf"
        ))])
        .expect("failed to register chrome fonts");
}
