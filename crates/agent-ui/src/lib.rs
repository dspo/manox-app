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
pub use steer_agent_chat_ui::{
    ahp_store, chat_fold, cockpit, conversation, git_status, overlap_diag,
};
pub mod browser_host;
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
pub mod settings_shell;
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

/// A gpui-shell host's replacement for the in-app Settings surface: when
/// installed, ⌘,/Settings… opens the host's script-driven floating dialog
/// instead of the Workspace overlay. Main-thread only (gpui entities).
pub type SettingsLauncher = std::rc::Rc<dyn Fn(&mut gpui::Window, &mut gpui::App)>;

thread_local! {
    static SETTINGS_LAUNCHER: std::cell::RefCell<Option<SettingsLauncher>> =
        const { std::cell::RefCell::new(None) };
}

/// Installs the gpui-shell Settings launcher (see [`SettingsLauncher`]).
pub fn set_settings_launcher(launcher: SettingsLauncher) {
    SETTINGS_LAUNCHER.with(|slot| *slot.borrow_mut() = Some(launcher));
}

/// Runs the installed Settings launcher, if a gpui-shell host mounted one.
pub(crate) fn run_settings_launcher(window: &mut gpui::Window, cx: &mut gpui::App) -> bool {
    SETTINGS_LAUNCHER.with(|slot| {
        if let Some(launch) = slot.borrow().as_ref() {
            launch(window, cx);
            true
        } else {
            false
        }
    })
}

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
pub fn composer_send_key_bindings(cx: &gpui::App) -> Vec<gpui::KeyBinding> {
    // `secondary-` resolves against the host OS, which 0.3.10's static
    // `KeyBinding::new` cannot do — the load constructor can.
    let predicate = gpui::KeyBindingContextPredicate::parse("composer > Input")
        .expect("composer > Input context parses")
        .into();
    vec![
        gpui::KeyBinding::load(
            "secondary-shift-enter",
            Box::new(steer_agent_chat_ui::ComposerSubmitInterrupt),
            Some(predicate),
            false,
            None,
            cx,
        )
        .expect("secondary-shift-enter resolves per host OS"),
    ]
}

#[cfg(test)]
mod tests {
    use super::{composer_recall_key_bindings, composer_send_key_bindings};
    use steer_agent_chat_ui::turn_navigator_key_bindings;

    /// `KeyBinding::new` panics on an unparseable context predicate, so
    /// constructing every binding set is a startup crash regression test.
    #[gpui::test]
    fn key_bindings_parse(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            assert_eq!(composer_recall_key_bindings().len(), 2);
            assert_eq!(composer_send_key_bindings(cx).len(), 1);
            assert_eq!(turn_navigator_key_bindings().len(), 7);
        });
    }
}
