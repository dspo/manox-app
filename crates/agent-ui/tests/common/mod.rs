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
use gpui::{AppContext as _, Entity, TestAppContext, px, size};
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
                    "../../../steer-app/assets/fonts/lilex/Lilex-Light.ttf"
                )),
                Cow::Borrowed(include_bytes!(
                    "../../../steer-app/assets/fonts/lilex/Lilex-Medium.ttf"
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
    open_workspace_at(cx, 1_120.)
}

/// The same shell at an explicit width — the narrow-card layout probes need
/// to drive the strip at a width the default window never reaches.
pub fn open_workspace_at(
    cx: &mut TestAppContext,
    width: f32,
) -> (gpui::WindowHandle<gpui_component::Root>, Entity<Workspace>) {
    let cell: std::rc::Rc<std::cell::RefCell<Option<Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let capture = cell.clone();
    let window = cx.open_window(size(px(width), px(780.)), move |window, cx| {
        let workspace = cx.new(|cx| Workspace::new(window, cx));
        *capture.borrow_mut() = Some(workspace.clone());
        gpui_component::Root::new(workspace, window, cx)
    });
    cx.run_until_parked();
    (window, cell.borrow().clone().expect("workspace captured"))
}

// ── full-wiring harness (real in-proc host, production chrome assembly) ──

/// Poll budget for the real-host round trips: the create/attach lands on
/// the tokio runtime and only then wakes the gpui side, so consumers poll
/// (`run_until_parked` plus a real sleep) instead of parking once.
pub const PARK_ROUNDS: usize = 100;
pub const PARK_STEP: std::time::Duration = std::time::Duration::from_millis(50);

/// Boot the full-wiring shape: a hermetic state root, every process-global
/// the app installs at startup, the production `chrome_assembly` mount, and
/// the completed AHP handshake. One `#[gpui::test]` per binary (module
/// header) — the singletons are process-wide.
pub fn full_wiring_boot(cx: &mut TestAppContext) -> (Entity<Workspace>, gpui::AnyWindowHandle) {
    let (ws, window, _shell) = full_wiring_boot_shell(cx);
    (ws, window)
}

/// The same boot, returning the mounted `Shell` entity too — the
/// pane/dock-switch regressions drive the shell face directly.
pub fn full_wiring_boot_shell(
    cx: &mut TestAppContext,
) -> (
    Entity<Workspace>,
    gpui::AnyWindowHandle,
    gpui::Entity<steer_agent_chrome_ui::Shell>,
) {
    cx.update(|_cx| {
        manox_agent::runtime::init_hermetic_for_test();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init();
        manox_agent::object_store::init();
    });
    cx.background_executor.allow_parking();
    let cell: std::rc::Rc<std::cell::RefCell<Option<Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let shell_cell: std::rc::Rc<
        std::cell::RefCell<Option<gpui::Entity<steer_agent_chrome_ui::Shell>>>,
    > = std::rc::Rc::new(std::cell::RefCell::new(None));
    let capture = cell.clone();
    let shell_capture = shell_cell.clone();
    cx.update(gpui_component::init);
    cx.update(|cx| {
        steer_i18n::init();
        agent_ui::slash_command::init(cx);
    });
    let window = cx.open_window(size(px(1_280.), px(820.)), move |window, cx| {
        steer_agent_chrome_ui::register_fonts(cx);
        let shell = agent_ui::chrome_assembly::mount(window, cx);
        let ws = agent_ui::dispatch::workspace_global().expect("assembly workspace");
        *capture.borrow_mut() = Some(ws);
        *shell_capture.borrow_mut() = Some(shell.clone());
        gpui_component::Root::new(shell, window, cx)
    });
    // The production main installs this right after the window opens
    // (`steer-app/src/main.rs`); the pane-opening paths route through it
    // (the sub-agent panels, the edit cards' diff tab), so the harness
    // installs it too.
    agent_ui::dispatch::set_window(window);
    cx.run_until_parked();
    let ws = cell.borrow().clone().expect("workspace captured");
    let shell = shell_cell.borrow().clone().expect("shell captured");
    for _ in 0..PARK_ROUNDS {
        cx.run_until_parked();
        if cx.update(|cx| ws.read(cx).diagnostic_ahp_connected(cx)) {
            break;
        }
        std::thread::sleep(PARK_STEP);
    }
    cx.run_until_parked();
    (ws, window.into(), shell)
}

/// The Steer row's own gesture: 新建会话 on the workspace, returning the
/// minted (or refilled) session id.
pub fn new_container(
    cx: &mut TestAppContext,
    ws: &Entity<Workspace>,
    window: gpui::AnyWindowHandle,
    project: Option<std::path::PathBuf>,
) -> String {
    let ws = ws.clone();
    cx.update_window(window, move |_, window, cx| {
        ws.update(cx, |ws, cx| ws.new_container(project.clone(), window, cx))
    })
    .expect("the full-wiring window is live")
}

/// Tear down the process globals `full_wiring_boot` installed.
/// Drain whatever the last command scheduled (the landing's refresh walks
/// and event pumps park on the real runtime and only wake between polls),
/// then tear down the process globals `full_wiring_boot` installed. Without
/// the drain the gpui leak detector can catch a task still holding the
/// workspace at drop time.
pub fn full_wiring_teardown(cx: &mut TestAppContext) {
    cx.run_until_parked();
    std::thread::sleep(PARK_STEP);
    cx.run_until_parked();
    manox_agent::thread_store::drop_global_for_test();
    agent_ui::dispatch::clear_globals_for_test();
}

/// The foreground session id the workspace is bound to.
pub fn foreground_session(cx: &mut TestAppContext, ws: &Entity<Workspace>) -> Option<String> {
    cx.update(|cx| ws.read(cx).diagnostic_foreground_session(cx))
}

/// Dispatch `line` through the live slash registry and poll until the
/// foreground conversation moves off its current id — the shape every
/// create/exit regression shares. `None` when the command silently no-oped.
pub fn run_slash_until_foreground_moves(
    cx: &mut TestAppContext,
    ws: &Entity<Workspace>,
    window: gpui::AnyWindowHandle,
    line: &str,
) -> Option<String> {
    let parsed = agent_ui::slash_command::parse(line).unwrap_or_else(|| panic!("{line} parses"));
    let before = foreground_session(cx, ws).expect("foreground bound");
    cx.update_window(window, |_, window, cx| {
        let ws = ws.clone();
        ws.update(cx, |ws, cx| {
            agent_ui::slash_command::dispatch(&parsed, ws, window, cx);
        });
    })
    .ok()?;
    wait_foreground_moved(cx, ws, &before)
}

/// Poll until the foreground session is not `before` (create, attach and
/// the `/exit` teardown beats all land asynchronously).
pub fn wait_foreground_moved(
    cx: &mut TestAppContext,
    ws: &Entity<Workspace>,
    before: &str,
) -> Option<String> {
    for _ in 0..PARK_ROUNDS {
        cx.run_until_parked();
        let now = foreground_session(cx, ws);
        if now.is_some() && now.as_deref() != Some(before) {
            return now;
        }
        std::thread::sleep(PARK_STEP);
    }
    None
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

/// `turns` rounds of (question, long answer) as a chat fold — the projection
/// counterpart of [`tall_history`].
pub fn tall_chat(turns: usize) -> ahp_types::state::ChatState {
    let filler = "lorem ipsum ".repeat(40);
    let turns_json: Vec<serde_json::Value> = (0..turns)
        .map(|turn| {
            serde_json::json!({
                "id": format!("t{turn}"),
                "startedAt": "2026-01-01T00:00:00Z",
                "message": { "text": format!("question {turn}"), "origin": { "kind": "user" } },
                "responseParts": [
                    { "kind": "markdown", "id": format!("p{turn}"),
                      "content": format!("{filler} answer {turn}") },
                ],
                "state": "complete",
            })
        })
        .collect();
    serde_json::from_value(serde_json::json!({
        "resource": "ahp-chat:/c-fixture",
        "title": "fixture",
        "status": 0,
        "modifiedAt": "2026-01-01T00:00:00Z",
        "turns": turns_json,
    }))
    .expect("the tall chat fixture parses")
}

/// Lower a display sequence into a chat fold: a user message opens a turn,
/// and everything after it until the next user message becomes that turn's
/// response parts (text → markdown, thinking → reasoning, tool use →
/// toolCall with its result). The geometry-fixture counterpart of the
/// retired `synth_display`, run in reverse.
pub fn chat_from_display(display: &[HistoryEntry]) -> ahp_types::state::ChatState {
    use manox_agent::language_model::MessageContent as C;
    let mut turns: Vec<serde_json::Value> = Vec::new();
    let mut part_id = 0usize;
    for entry in display {
        let manox_agent::db::HistoryEntry::Message(message) = entry else {
            continue;
        };
        let is_user = message.role == Role::User
            && message
                .content
                .iter()
                .any(|c| !matches!(c, C::ToolResult(_)));
        if is_user {
            let text = message
                .content
                .iter()
                .filter_map(|c| match c {
                    C::Text(t) => Some(t.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            turns.push(serde_json::json!({
                "id": format!("t{}", turns.len()),
                "startedAt": "2026-01-01T00:00:00Z",
                "message": { "text": text, "origin": { "kind": "user" } },
                "responseParts": [],
                "state": "complete",
            }));
            continue;
        }
        let Some(turn) = turns.last_mut() else {
            continue;
        };
        let parts = turn
            .as_object_mut()
            .expect("object")
            .get_mut("responseParts")
            .and_then(|p| p.as_array_mut())
            .expect("parts array");
        for block in &message.content {
            match block {
                C::Text(t) => {
                    parts.push(serde_json::json!({
                        "kind": "markdown", "id": format!("p{part_id}"), "content": t,
                    }));
                    part_id += 1;
                }
                C::Thinking { text, .. } => {
                    parts.push(serde_json::json!({
                        "kind": "reasoning", "id": format!("p{part_id}"), "content": text,
                    }));
                    part_id += 1;
                }
                C::ToolUse(use_block) => {
                    parts.push(serde_json::json!({
                        "kind": "toolCall",
                        "toolCall": {
                            "status": "completed",
                            "toolCallId": use_block.id,
                            "toolName": use_block.name,
                            "displayName": use_block.name,
                            "invocationMessage": use_block.name,
                            "toolInput": use_block.raw_input,
                            "success": true,
                            "pastTenseMessage": use_block.name,
                            "confirmed": "not-needed",
                        },
                    }));
                }
                _ => {}
            }
        }
    }
    if turns.is_empty() {
        turns.push(serde_json::json!({
            "id": "t0",
            "startedAt": "2026-01-01T00:00:00Z",
            "message": { "text": "fixture", "origin": { "kind": "user" } },
            "responseParts": [],
            "state": "complete",
        }));
    }
    serde_json::from_value(serde_json::json!({
        "resource": "ahp-chat:/c-fixture",
        "title": "fixture",
        "status": 0,
        "modifiedAt": "2026-01-01T00:00:00Z",
        "turns": turns,
    }))
    .expect("the display-lowered chat parses")
}

/// Project a display sequence — the rebuilt-transcript fixture form.
pub fn conversation_from_display(
    cx: &mut gpui::App,
    display: &[HistoryEntry],
) -> gpui::Entity<agent_ui::conversation::ConversationState> {
    let chat = chat_from_display(display);
    conversation_from_chat(cx, &chat)
}

/// Project a chat fold into a fresh `ConversationState` — the fixture form
/// of the sync every test shares.
pub fn conversation_from_chat(
    cx: &mut gpui::App,
    chat: &ahp_types::state::ChatState,
) -> gpui::Entity<agent_ui::conversation::ConversationState> {
    let recipient = manox_agent::MessageAuthor::Lead;
    let host = steer_agent_chat_ui::host::noop_host();
    let env = steer_agent_chat_ui::transcript::ProjectEnv {
        role: "test-model",
        recipient: &recipient,
        host: &host,
        cwd: None,
        fork_source: false,
        promoted_confirmation: None,
        entering_turns: &[],
        steered_turns: steer_agent_chat_ui::transcript::no_steered_turns(),
    };
    cx.new(|cx| {
        let mut conversation = agent_ui::conversation::ConversationState::new(recipient.clone());
        let nodes = conversation.stage(Some(chat), &env);
        conversation.reconcile(nodes, &env, cx);
        conversation
    })
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

/// A PTY source that never starts a process; the test drives its OUTPUT side
/// directly through the shared sender (see the external-session regression
/// binary for the long form).
#[derive(Clone, Default)]
pub struct FakePty {
    pub event_tx: std::sync::Arc<
        std::sync::Mutex<Option<async_channel::Sender<manox_terminal::event::TerminalEvent>>>,
    >,
}

impl FakePty {
    pub fn emit(&self, bytes: &[u8]) {
        self.event_tx
            .lock()
            .unwrap()
            .as_ref()
            .expect("pty started")
            .send_blocking(manox_terminal::event::TerminalEvent::PtyOutput(
                bytes.to_vec(),
            ))
            .expect("event channel open");
    }
}

impl manox_terminal::pty_source::PtySource for FakePty {
    fn start(&mut self, event_tx: async_channel::Sender<manox_terminal::event::TerminalEvent>) {
        *self.event_tx.lock().unwrap() = Some(event_tx);
    }
    fn write(&self, _bytes: &[u8]) -> std::io::Result<()> {
        Ok(())
    }
    fn resize(&self, _cols: u16, _rows: u16) -> std::io::Result<()> {
        Ok(())
    }
}

/// The REAL chrome assembly (shell + sidebar + production select hook)
/// around the REAL workspace; returns both entities.
pub fn open_assembly(
    cx: &mut TestAppContext,
) -> (
    gpui::WindowHandle<gpui_component::Root>,
    gpui::Entity<steer_agent_chrome_ui::Shell>,
    gpui::Entity<Workspace>,
) {
    let shell_cell: std::rc::Rc<
        std::cell::RefCell<Option<gpui::Entity<steer_agent_chrome_ui::Shell>>>,
    > = std::rc::Rc::new(std::cell::RefCell::new(None));
    let ws_cell: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let (shell_for_build, ws_for_build) = (shell_cell.clone(), ws_cell.clone());

    cx.update(gpui_component::init);
    cx.update(|_cx| {
        steer_i18n::init();
    });
    let window = cx.open_window(size(px(1_280.), px(820.)), move |window, cx| {
        steer_agent_chrome_ui::register_fonts(cx);
        let shell = agent_ui::chrome_assembly::mount(window, cx);
        let ws =
            agent_ui::dispatch::workspace_global().expect("the assembly created the workspace");
        *shell_for_build.borrow_mut() = Some(shell.clone());
        *ws_for_build.borrow_mut() = Some(ws);
        gpui_component::Root::new(shell, window, cx)
    });
    cx.run_until_parked();
    let shell = shell_cell.borrow().clone().expect("shell captured");
    let ws = ws_cell.borrow().clone().expect("workspace captured");
    (window, shell, ws)
}
