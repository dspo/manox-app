//! manox-agent-chat-ui — the chat column's state foundation
//! (PLAN-CHROME-CHAT-SPLIT Phase 2, first tranche).
//!
//! What lives here today: the journal→UI projection pipeline
//! (`journal_translate` / `journal_fold` / `server_note_translate`), the
//! store mirror it feeds (`client_store` + `client_store_handle` — the
//! foreground leaf the conversation renders against), and the rail's data
//! sources (`cockpit`, `git_status`). The conversation column's rendering
//! (ConversationState, message views, composer) follows in later tranches —
//! those still reach the Workspace shell and need the `ChatHost` inversion
//! first.
//!
//! Dependency invariant (the executable definition of the chrome/chat
//! decoupling): this crate never touches terminals, webviews, or
//! external-agent wiring — no `terminal-ui`, no `manox-webview`, no
//! `manox-ext-agents` (checked by script/check-chat-crate-deps.sh in CI).
//! The multiplexer (agent-ui) constructs the store handles through its
//! agent-ui → chat-crate dependency.

pub mod client_store;
pub mod client_store_handle;
pub mod cockpit;
pub mod git_status;
pub mod journal_fold;
pub mod journal_translate;
pub mod server_note_translate;
