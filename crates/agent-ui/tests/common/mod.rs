#![allow(dead_code)] // each test binary links the shared helpers it uses; the rest is unused per-binary

//! Shared harness for the interaction-card regression binaries. Each test
//! binary holds exactly ONE `#[gpui::test]`: the tests initialize
//! process-global singletons (`manox_agent::runtime`, `pi_providers`,
//! `thread_store`) whose global store entity is app-affine, so two tests in
//! one process would overwrite each other's store under the parallel test
//! harness (see the `workspace_overlap` binary for the same constraint).

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use agent_ui::Workspace;
use gpui::{
    AppContext as _, Context, Entity, IntoElement, Pixels, Render, TestAppContext, Window, px, size,
};
use gpui_component::Theme;
use manox_agent::db::{HistoryEntry, ThreadSummary};
use manox_agent::language_model::{LanguageModelToolUse, MessageContent, Role, TokenUsage};
use manox_agent::message::Message;
use manox_agent::thread_engine::{BackendNotice, ThreadEngine};
use manox_agent::{MessageProvenance, Thread};
use manox_harness::types::{ContentBlock, Model as PiModel};

/// Minimal backend for the workspace tests: no actor, fixed history. The
/// thread facade drives everything else through emitted `ThreadEvent`s.
pub struct FakeEngine {
    history: std::sync::Mutex<Vec<Message>>,
}

impl ThreadEngine for FakeEngine {
    fn is_running(&self) -> bool {
        false
    }
    fn set_cwd(&self, _path: PathBuf) {}
    fn history(&self) -> Vec<manox_agent::db::HistoryEntry> {
        self.history
            .lock()
            .unwrap()
            .clone()
            .into_iter()
            .map(manox_agent::db::HistoryEntry::Message)
            .collect()
    }
    fn request_token_usage(&self) -> std::collections::HashMap<String, TokenUsage> {
        std::collections::HashMap::new()
    }
    fn model(&self) -> Option<PiModel> {
        None
    }
    fn run(&self, _prompt: String, _images: Vec<ContentBlock>) {}
    fn steer(
        &self,
        _text: String,
        _images: Vec<ContentBlock>,
        _message_id: Option<String>,
    ) -> String {
        String::new()
    }
    fn cancel_steer(&self, _id: &str) -> bool {
        false
    }
    fn abort(&self) {}
    fn set_model(&self, _model: PiModel) {}
    fn set_thinking_level(&self, _level: Option<String>) {}
    fn open_session(&self, _path: PathBuf) {}
    fn active_session_path(&self) -> Option<PathBuf> {
        None
    }
    fn session_list(&self) -> Vec<ThreadSummary> {
        Vec::new()
    }
}

fn register_lilex(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.text_system()
            .add_fonts(vec![
                Cow::Borrowed(include_bytes!(
                    "../../../manox/assets/fonts/lilex/Lilex-Light.ttf"
                )),
                Cow::Borrowed(include_bytes!(
                    "../../../manox/assets/fonts/lilex/Lilex-Medium.ttf"
                )),
            ])
            .expect("Lilex fonts");
        let theme = Theme::global_mut(cx);
        theme.mono_font_family = "Lilex".into();
    });
}

pub fn init_harness(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    register_lilex(cx);
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init();
    });
    // The AgentServer replies to the gpui store pump across threads on the
    // real tokio runtime; that legitimate cross-thread wake is flagged by the
    // deterministic test scheduler unless parking is allowed — without this
    // the test flakes (the tokio reply lands while the pump is parked).
    cx.background_executor.allow_parking();
}

/// Open the production workspace shell and return the window + workspace.
pub fn open_workspace(
    cx: &mut TestAppContext,
) -> (gpui::WindowHandle<gpui_component::Root>, Entity<Workspace>) {
    let cell: std::rc::Rc<std::cell::RefCell<Option<Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let capture = cell.clone();
    let window = cx.open_window(size(px(1_120.), px(780.)), move |window, cx| {
        let workspace = cx.new(|cx| Workspace::new(window, cx));
        *capture.borrow_mut() = Some(workspace.clone());
        gpui_component::Root::new(workspace, window, cx)
    });
    cx.run_until_parked();
    (window, cell.borrow().clone().expect("workspace captured"))
}

/// A thread facade with a fake engine: `Thread::landing` defers engine
/// creation, so the test swaps in the fake before anything runs. Returns the
/// gpui-free `ThreadHandle`; the workspace binds its own `ClientStoreHandle`
/// on `attach_thread`, and tests drive it through `emit`.
pub fn fake_thread(
    cx: &mut TestAppContext,
    history: Vec<Message>,
) -> manox_agent::thread::ThreadHandle {
    let (_, events) = tokio::sync::mpsc::unbounded_channel::<BackendNotice>();
    cx.update(|_cx| {
        let thread = Thread::landing(PathBuf::from("/tmp"));
        thread.with_mut(|t| {
            t.set_engine_for_test(
                Arc::new(FakeEngine {
                    history: std::sync::Mutex::new(history),
                }),
                events,
            )
        });
        thread
    })
}

/// A landing thread facade with a pinned id, as the history-gate tests attach
/// one (defers engine creation, so no host is needed to observe the gate).
pub fn landing_thread(id: &str) -> manox_agent::thread::ThreadHandle {
    manox_agent::thread::Thread::landing_with_id(
        manox_agent::ThreadId(id.to_string()),
        PathBuf::from("/tmp"),
    )
}

/// A minimal chat `Message` with a single text block, as the transcript
/// builders seed one.
pub fn msg(id: &str, role: Role, text: &str) -> Message {
    Message {
        id: id.to_string(),
        timestamp: 0,
        parent_id: None,
        provenance: if role == Role::User {
            MessageProvenance::User
        } else {
            MessageProvenance::Assistant
        },
        role,
        content: vec![MessageContent::Text(text.to_string())],
        ui: None,
    }
}

/// A transcript taller than the viewport (`filler`-padded answers): with
/// short content the native `gpui::list` re-anchors every layout at its
/// floor (chat-log semantics) and no scroll position can survive.
pub fn tall_history(turns: usize) -> Vec<HistoryEntry> {
    let filler = "lorem ipsum ".repeat(40);
    (0..turns)
        .flat_map(|turn| {
            vec![
                HistoryEntry::Message(msg(
                    &format!("u{turn}"),
                    Role::User,
                    &format!("question {turn}"),
                )),
                HistoryEntry::Message(msg(
                    &format!("a{turn}"),
                    Role::Assistant,
                    &format!("{filler} answer {turn}"),
                )),
            ]
        })
        .collect()
}

/// The probe window's height, the viewport bound the ask-card geometry
/// assertions check the footer against.
pub const PROBE_WINDOW_HEIGHT: Pixels = px(780.);

/// Re-pulls the diagnostic ask-card element from the workspace state on EVERY
/// frame — an `AnyElement` is consumed by its first paint, so a stored one
/// would leave later frames empty. The weak handle is deliberately invalid:
/// the build runs while the workspace entity is `update`-held, and the card's
/// render path upgrades the weak to read the custom-input state, which would
/// double-borrow. A geometry probe needs no live custom row.
pub struct AskCardProbe {
    pub ws: Entity<Workspace>,
}

impl Render for AskCardProbe {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A geometry probe needs no live host: the noop host mirrors the
        // old invalid-weak trick (no custom row, controls render inert).
        let card = self.ws.update(cx, |ws, cx| {
            ws.diagnostic_ask_card_element(manox_agent_chat_ui::host::noop_host(), 0, cx)
        });
        card.unwrap_or_else(|| gpui::div().into_any_element())
    }
}

/// A real plan file on disk, as `ProposePlan` leaves one.
pub fn write_plan_file() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit-plan.md");
    std::fs::write(&path, "# Audit\n\nsteps").unwrap();
    let plan_file = path.to_string_lossy().to_string();
    (dir, plan_file)
}

pub fn bash_tool_use_message(id: &str) -> Message {
    Message::assistant(vec![MessageContent::ToolUse(LanguageModelToolUse {
        id: id.to_string(),
        name: "Bash".into(),
        raw_input: r#"{"command":"ls"}"#.to_string(),
        input: serde_json::json!({"command": "ls"}),
        is_input_complete: true,
        thought_signature: None,
    })])
}
