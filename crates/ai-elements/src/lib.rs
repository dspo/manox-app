//! First-party GPUI components for agent conversation UI.
//!
//! The component set follows Vercel's AI Elements (`vercel/ai-elements`): the
//! same element names, the same props, and the same behaviour as the upstream
//! React components. It is a reimplementation of that *contract* on gpui +
//! gpui-component, not a port of upstream code — the semantics are shared, the
//! implementation is this repo's own.
//!
//! Boundary: this crate owns agent-conversation semantics. Rendering machinery
//! (markdown, terminal output) belongs to `steer-components`; conversation
//! state and the agent runtime belong to `agent-ui` and must not be reachable
//! from here. Components therefore take content and localized strings from the
//! caller rather than reading them from a session.

//! Agent-facing display semantics: reasoning blocks, tool/shell cards, ask
//! cards, markdown-adjacent views — and the status glyphs.
//!
//! ## Spinner mounting contract
//!
//! The animated glyphs (`BrailleSpinner`, `PixelSpinner`) split by mount:
//!
//! - `.view(key, window, cx)` — the ANIMATED mount: a `SpinnerView` entity
//!   on the shared [`TickClock`]. All in-app mounts use this; there is no
//!   inline animated render (an animated glyph without its own view is the
//!   whole-window-rebuild failure mode #320 fixed). `reduce_motion`
//!   renders the static frame and stops the beat.
//! - plain `IntoElement`/`RenderOnce` — the STATIC mount: PixelSpinner's
//!   hold frame; `BrailleSpinner` has none (it has no static face — it is
//!   an animated-only glyph).
//!
//! `synced`/`max_fps` knobs are gone: one clock owns the rate.

mod animation;
mod glyph_view;
mod text_runs;

pub mod ask;
pub mod code_view;
pub mod meerkat;
pub mod pixel_spinner;
pub mod reasoning;
pub mod shell;
pub mod spinner;
pub mod tick_clock;
pub mod tool;

pub use ask::{AskBlock, AskOption, AskState, AskSubmitted, AskUserQuestion};
pub use code_view::{CodeLine, CodeLineKind, CodeView};
pub use glyph_view::SpinnerView;
pub use meerkat::{FRAME_NAMES, Meerkat, NATIVE_CELL};
pub use pixel_spinner::{PixelSpinner, PixelSpinnerVariant};
pub use reasoning::{
    AUTO_CLOSE_DELAY, Reasoning, ReasoningContent, ReasoningEvent, ReasoningState,
    ReasoningTrigger, ThinkingMessage, default_thinking_message,
};
pub use shell::{ShellLine, ShellSpanKind, ShellView};
pub use spinner::BrailleSpinner;
pub use tick_clock::{GLYPH_FPS, TickClock, live_glyphs};
pub use tool::{
    AUTO_FOLD_DELAY, Tool, ToolEvent, ToolHeader, ToolState, ToolStatus, format_tool_duration,
};
