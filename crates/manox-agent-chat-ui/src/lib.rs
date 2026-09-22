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

pub mod ask_card;
pub mod client_store;
pub mod client_store_handle;
pub mod cockpit;
pub mod column;
pub mod conversation;
pub mod git_status;
pub mod host;
pub mod i18n;
pub mod journal_fold;
pub mod journal_translate;
pub mod overlap_diag;
pub mod server_note_translate;
pub mod views;

gpui::actions!(
    manox_agent_chat_ui,
    [
        AskPrev,
        AskNext,
        AskCancel,
        CompletionUp,
        CompletionDown,
        CompletionConfirm,
        CompletionDismiss,
        ComposerRecallUp,
        ComposerRecallDown,
        UndoLastQueued,
        ToggleTurnNavigator,
        CopySelectedTurn,
        FillComposerTurn,
        ToggleCockpitTasks,
    ]
);

/// Keybindings owned by the user-turn navigator.
///
/// Keeping these beside the actions lets the application and GPUI interaction
/// tests install the exact same bindings. The descendant context is more
/// specific than the input's own bindings, so navigation keys are intercepted
/// only while the navigator search field is focused.
pub fn turn_navigator_key_bindings() -> Vec<gpui::KeyBinding> {
    vec![
        #[cfg(target_os = "macos")]
        gpui::KeyBinding::new("cmd-m", ToggleTurnNavigator, None),
        #[cfg(not(target_os = "macos"))]
        gpui::KeyBinding::new("ctrl-m", ToggleTurnNavigator, None),
        #[cfg(target_os = "macos")]
        gpui::KeyBinding::new("cmd-c", CopySelectedTurn, Some("TurnNavigator > Input")),
        #[cfg(not(target_os = "macos"))]
        gpui::KeyBinding::new("ctrl-c", CopySelectedTurn, Some("TurnNavigator > Input")),
        // Enter jumps to the selected turn; the secondary modifier refills the
        // composer with it instead (reuse-and-edit, not locate).
        #[cfg(target_os = "macos")]
        gpui::KeyBinding::new("cmd-enter", FillComposerTurn, Some("TurnNavigator > Input")),
        #[cfg(not(target_os = "macos"))]
        gpui::KeyBinding::new(
            "ctrl-enter",
            FillComposerTurn,
            Some("TurnNavigator > Input"),
        ),
        gpui::KeyBinding::new("up", CompletionUp, Some("TurnNavigator > Input")),
        gpui::KeyBinding::new("down", CompletionDown, Some("TurnNavigator > Input")),
        gpui::KeyBinding::new("enter", CompletionConfirm, Some("TurnNavigator > Input")),
        gpui::KeyBinding::new("escape", CompletionDismiss, Some("TurnNavigator > Input")),
    ]
}
