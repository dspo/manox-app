//! manox UI layer, built on gpui-component.
//!
//! Workspace top-level view + `ConversationState` + views. Holds an
//! the AgentServer-backed `ClientStoreHandle` (the data mirror around the
//! gpui-free `manox_agent::ThreadHandle`) and subscribes to
//! `ThreadEvent` for incremental rendering.
pub mod assets;
pub(crate) mod chat_host;
pub use chat_host::WorkspaceChatHost;
// The chat foundation now lives in manox-agent-chat-ui (Phase 2); these
// re-exports keep every `crate::…` path inside agent-ui (and the tests)
// resolving unchanged.
pub use manox_agent_chat_ui::{
    client_store, client_store_handle, cockpit, conversation, git_status, journal_fold,
    journal_translate, overlap_diag, server_note_translate,
};
pub mod browser_host;
pub mod chatgpt_app;
pub mod chrome_assembly;
#[cfg(test)]
mod client_store_handle_tests;
pub mod dispatch;
pub mod external_session;
pub mod i18n;
pub mod menu;
pub mod multiplexer;
pub mod sidebar_projection;
pub mod sidebar_view;
pub mod slash_command;
pub(crate) mod source_gates;
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
pub use manox_agent_chat_ui::turn_navigator_key_bindings;
pub use manox_agent_chat_ui::{
    AskCancel, AskNext, AskPrev, CompletionConfirm, CompletionDismiss, CompletionDown,
    CompletionUp, ComposerRecallDown, ComposerRecallUp, CopySelectedTurn, FillComposerTurn,
    ToggleCockpitTasks, ToggleTurnNavigator, UndoLastQueued,
};

gpui::actions!(
    agent_ui,
    [
        ToggleEditor,
        ToggleEditorPreview,
        CloseEditor,
        OpenSettings,
        NewTerminalTab,
        CloseTerminalTab,
        FocusTerminal,
        FocusConversation,
        OpenBrowserTab,
        CloseBrowserTab,
        BackgroundCurrentThread,
        ArchiveCurrentThread
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

#[cfg(test)]
mod tests {
    use super::composer_recall_key_bindings;
    use manox_agent_chat_ui::turn_navigator_key_bindings;

    /// `KeyBinding::new` panics on an unparseable context predicate, so
    /// constructing every binding set is a startup crash regression test.
    #[test]
    fn key_bindings_parse() {
        assert_eq!(composer_recall_key_bindings().len(), 2);
        assert_eq!(turn_navigator_key_bindings().len(), 7);
    }
}
