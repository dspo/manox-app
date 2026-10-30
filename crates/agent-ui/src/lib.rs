//! manox UI layer, built on gpui-component.
//!
//! Workspace top-level view + `ConversationState` + views. Holds an
//! the AgentServer-backed `ClientStoreHandle` (the data mirror around the
//! gpui-free `manox_agent::ThreadHandle`) and subscribes to
//! `ThreadEvent` for incremental rendering.
pub mod assets;
pub(crate) mod chat_host;
pub use chat_host::WorkspaceChatHost;
// The chat foundation now lives in steer-agent-chat-ui (Phase 2); these
// re-exports keep every `crate::…` path inside agent-ui (and the tests)
// resolving unchanged.
pub use steer_agent_chat_ui::{ahp_store, cockpit, conversation, git_status, overlap_diag};
// The integrated browser (embedded webview host, browser tabs) is compiled
// out on Linux/FreeBSD: wry parents Linux child webviews through an Xlib
// handle only, so a window there cannot host one. Clipboard capability
// survives in `capability_host` on every platform.
#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
pub mod browser_host;
pub mod capability_host;
pub mod chatgpt_app;
pub mod chrome_assembly;
pub mod dispatch;
pub mod i18n;
pub mod menu;
pub mod model_catalog;
pub mod multiplexer;
pub mod preview_host;
pub mod project_menu;
pub mod project_registry;
/// The registry's working faces are crate-internal; only the test-support
/// diagnostics are externally visible (the integration regressions read it).
pub mod resource_registry;
pub mod sidebar_projection;
pub mod slash_command;
pub mod tool_tabs;
pub mod views;
pub mod vscode_app;
pub mod workspace;

pub use views::settings::SettingsView;
pub use workspace::Workspace;

pub use chatgpt_app::LaunchChatGptApp;
pub use vscode_app::LaunchVSCode;

// Open/close the right-side markdown composer, plus the global OpenSettings
// action that flips the Workspace into the Settings overlay. AskPrev/AskNext
// navigate between questions in the ask drawer (bound to arrow keys within the
// drawer's focus context).
pub use steer_agent_chat_ui::turn_navigator_key_bindings;
pub use steer_agent_chat_ui::{
    AskCancel, AskNext, AskPrev, CompletionConfirm, CompletionDismiss, CompletionDown,
    CompletionUp, ComposerRecallDown, ComposerRecallUp, CopySelectedTurn, FillComposerTurn,
    ToggleTurnNavigator, UndoLastQueued,
};

gpui::actions!(
    agent_ui,
    [
        OpenSettings,
        FocusConversation,
        BackgroundCurrentThread,
        ArchiveCurrentThread,
        DismissContextBubble
    ]
);

/// Keybindings for composer history recall.
///
/// Recall lives on `alt-up` / `alt-down` only: the bare arrows belong to the
/// `Input`'s own `MoveUp` / `MoveDown`, so within the composer they always
/// move the caret and never jump between past turns — at any caret position,
/// including the first line's start and the last line's end. The bindings
/// match the whole composer subtree (`composer > Input`) regardless of the
/// input's content, because nothing native claims these keystrokes; the
/// wrapper stops carrying the `composer` context while the completion
/// popover is open, which is the one state where recall stays out of the way.
pub fn composer_recall_key_bindings() -> Vec<gpui::KeyBinding> {
    vec![
        gpui::KeyBinding::new("alt-up", ComposerRecallUp, Some("composer > Input")),
        gpui::KeyBinding::new("alt-down", ComposerRecallDown, Some("composer > Input")),
    ]
}

/// Keybindings for the composer's streaming send actions: `⇧⌘↩` (interrupt)
/// has no upstream binding — gpui-base binds only `enter` / `shift-enter` /
/// `secondary-enter`, and overloading `Enter` for the chord would newline
/// AND submit. A dedicated action on the composer's own context keeps the
/// keystroke out of the textarea entirely.
pub fn composer_send_key_bindings() -> Vec<gpui::KeyBinding> {
    vec![gpui::KeyBinding::new(
        "secondary-shift-enter",
        steer_agent_chat_ui::ComposerSubmitInterrupt,
        Some("composer > Input"),
    )]
}

#[cfg(test)]
mod tests {
    use super::{composer_recall_key_bindings, composer_send_key_bindings};
    use steer_agent_chat_ui::turn_navigator_key_bindings;

    /// `KeyBinding::new` panics on an unparseable context predicate, so
    /// constructing every binding set is a startup crash regression test.
    #[test]
    fn key_bindings_parse() {
        assert_eq!(composer_recall_key_bindings().len(), 2);
        assert_eq!(composer_send_key_bindings().len(), 1);
        assert_eq!(turn_navigator_key_bindings().len(), 7);
    }
}
