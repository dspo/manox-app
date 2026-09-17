//! First-party GPUI components for agent conversation UI.
//!
//! The component set follows Vercel's AI Elements (`vercel/ai-elements`): the
//! same element names, the same props, and the same behaviour as the upstream
//! React components. It is a reimplementation of that *contract* on gpui +
//! gpui-component, not a port of upstream code — the semantics are shared, the
//! implementation is this repo's own.
//!
//! Boundary: this crate owns agent-conversation semantics. Rendering machinery
//! (markdown, terminal output) belongs to `manox-components`; conversation
//! state and the agent runtime belong to `agent-ui` and must not be reachable
//! from here. Components therefore take content and localized strings from the
//! caller rather than reading them from a session.

mod animation;

pub mod chain_of_thought;
pub mod reasoning;

pub use chain_of_thought::{ChainOfThought, ChainOfThoughtHeader, ChainOfThoughtStep};
pub use reasoning::{
    AUTO_CLOSE_DELAY, Reasoning, ReasoningContent, ReasoningEvent, ReasoningState,
    ReasoningTrigger, ThinkingMessage, default_thinking_message,
};
