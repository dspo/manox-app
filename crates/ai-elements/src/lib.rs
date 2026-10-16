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

mod animation;

pub mod ask;
pub mod code_view;
pub mod meerkat;
pub mod pixel_spinner;
pub mod reasoning;
pub mod shell;
pub mod spinner;
pub mod tool;

pub use ask::{AskBlock, AskOption, AskState, AskSubmitted, AskUserQuestion};
pub use code_view::{CodeLine, CodeLineKind, CodeView};
pub use meerkat::{FRAME_NAMES, Meerkat, NATIVE_CELL};
pub use pixel_spinner::{PixelSpinner, PixelSpinnerVariant};
pub use reasoning::{
    AUTO_CLOSE_DELAY, Reasoning, ReasoningContent, ReasoningEvent, ReasoningState,
    ReasoningTrigger, ThinkingMessage, default_thinking_message,
};
pub use shell::{ShellLine, ShellSpanKind, ShellView};
pub use spinner::BrailleSpinner;
pub use tool::{
    AUTO_FOLD_DELAY, Tool, ToolEvent, ToolHeader, ToolState, ToolStatus, format_tool_duration,
};
