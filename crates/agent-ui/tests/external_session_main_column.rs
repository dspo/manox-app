//! FULL-WIRING regression for the external-session row click: the REAL
//! chrome assembly (chrome_assembly::mount — the shell, the sidebar, the
//! production select hook) around the REAL workspace. Launching an external
//! session must put its row in the sidebar, and CLICKING the row must bring
//! the session back to the main column (ViewMode::ExternalSession).
//!
//! Own test binary: the harness initializes process-global singletons (see
//! `tests/common/mod.rs` — one `#[gpui::test]` per binary).
#![cfg(feature = "test-support")]

mod common;

use agent_ui::Workspace;
use common::init_harness;
use gpui::{AppContext as _, TestAppContext, VisualTestContext, px, size};
use gpui_component::Root;
use steer_agent_chrome_ui::register_fonts;

/// A PTY source that never starts a process; the test drives its OUTPUT
/// side directly through the shared sender, so pushed bytes travel the
/// real path (terminal pump → vte parser → `Title` events) exactly as a
/// TUI agent's stream would.
#[derive(Clone, Default)]
struct FakePty {
    event_tx: std::sync::Arc<
        std::sync::Mutex<Option<async_channel::Sender<manox_terminal::event::TerminalEvent>>>,
    >,
}

impl FakePty {
    /// Emit raw PTY output bytes (the program's side of the stream).
    fn emit(&self, bytes: &[u8]) {
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

#[gpui::test]
async fn external_row_click_brings_the_session_to_the_main_column(cx: &mut TestAppContext) {
    init_harness(cx);
    // The REAL assembly: the chrome shell around the workspace (the sidebar,
    // the production select hook, the projection merge).
    let (window, shell, ws) = open_assembly(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    // Launch an external session the way the project menu does.
    let pty = FakePty::default();
    let any = window.into();
    cx.update_window(any, |_, window, cx| {
        ws.update(cx, |ws, cx| {
            let terminal = manox_terminal::Terminal::spawn(
                "test-ext-pty".into(),
                std::path::PathBuf::from("/tmp"),
                80,
                24,
                Box::new(pty.clone()),
            )
            .expect("fake terminal spawns");
            let proxy = cx.new(|cx| terminal_ui::terminal_proxy::TerminalProxy::new(terminal, cx));
            let view = terminal_ui::TerminalView::new(proxy, cx);
            ws.spawn_external_session(
                agent_ui::workspace::external_sessions::ExternalSessionLaunch {
                    agent_id: "claude",
                    cx_session_id: "cx-session-1234".into(),
                    label: "Claude Code".into(),
                    svg: "icons/claude.svg",
                    project: None,
                },
                view,
                window,
                cx,
            );
        });
    })
    .unwrap();
    cx.run_until_parked();

    // The session foregrounds immediately (spawn = register + main column).
    assert!(
        ws.read_with(cx, |ws, _| ws.external_session_foreground()),
        "spawning foregrounds the session in the main column"
    );

    // The sidebar carries the row under its namespaced uuid id.
    let row_id = shell.read_with(cx, |s, _| {
        s.sessions
            .iter()
            .find(|r| r.id.starts_with("external:claude:"))
            .map(|r| r.id.clone())
            .expect("the external row merged into the sidebar snapshot")
    });
    let row_selector: &'static str =
        Box::leak(format!("chrome-session-row-{row_id}").into_boxed_str());
    let row = visual
        .debug_bounds(row_selector)
        .expect("the external session's row paints in the sidebar");

    // PARK via the REAL path: opening a thread (the sidebar click's full
    // effect — synchronous leave + the async attach tail).
    let any = window.into();
    cx.update_window(any, |_, window, cx| {
        ws.update(cx, |ws, cx| {
            ws.diagnostic_attach_thread(common::landing_thread("park-target"), true, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    // The REAL sidebar-click path: open_thread parks the external session
    // (its first move) and attaches the thread.
    cx.update_window(any, |_, window, cx| {
        ws.update(cx, |ws, cx| {
            ws.open_thread("park-target".into(), window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        !ws.read_with(cx, |ws, _| ws.external_session_foreground()),
        "thread navigation parks the session"
    );

    // THE USER FLOW: clicking the parked session's sidebar row brings it
    // back to the main column.
    visual.simulate_click(row.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    for _ in 0..3 {
        cx.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }

    assert!(
        ws.read_with(cx, |ws, _| ws.external_session_foreground()),
        "clicking the parked session's row re-foregrounds it in the main column"
    );
    // And the main column actually PAINTS the session card (not just state).
    assert!(
        visual.debug_bounds("external-session-main").is_some(),
        "the external session card paints in the main column after the resume click"
    );
    assert_eq!(
        shell.read_with(cx, |s, _| s.active.clone()),
        Some(row_id.clone()),
        "the shell's highlight follows the clicked external row"
    );
    // The identity link rides the record: the row id is namespaced, the
    // real cx session id is recorded beside it.
    assert!(
        ws.read_with(cx, |ws, _| ws.is_external_session(&row_id)),
        "the workspace routes the namespaced row id"
    );

    // ── the title face ──
    // The row's title lives in the workspace record (the sidebar row AND
    // the main-column titlebar both read it); the terminal's own OSC claim
    // and the row menu's rename both land there.

    // THE TUI'S OWN CLAIM: the program writes an OSC 0 title the way
    // Claude Code does — raw bytes on the PTY — and the row follows.
    pty.emit(b"\x1b]0;\xe2\x9c\xb3 refactoring\x07");
    wait_for_title(cx, &shell, "✳ refactoring");

    // THE USER'S RENAME: the row menu's 重命名 editor commits through the
    // host hook into the same record (last writer wins over the TUI).
    let any = window.into();
    cx.update_window(any, |_, window, cx| {
        shell.update(cx, |s, cx| s.begin_title_edit(row_id.clone(), window, cx));
    })
    .unwrap();
    let (_, input) = shell.read_with(cx, |s, _| {
        s.title_edit_input().expect("the title editor is mounted")
    });
    let any = window.into();
    cx.update_window(any, |_, window, cx| {
        input.update(cx, |state, cx| state.set_value("我的重构会话", window, cx));
    })
    .unwrap();
    let any = window.into();
    cx.update_window(any, |_, window, cx| {
        shell.update(cx, |s, cx| s.commit_title_edit(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        shell.read_with(cx, |s, _| {
            s.sessions
                .iter()
                .any(|r| r.id == row_id && r.title == "我的重构会话")
        }),
        "the rename commit retitled the row through the host hook"
    );

    // THE TUI CLAIMS AGAIN (OSC 2 this time — the window-title code): the
    // row keeps following the program through its lifetime.
    pty.emit(b"\x1b]2;codex plan\x07");
    wait_for_title(cx, &shell, "codex plan");

    manox_agent::thread_store::drop_global_for_test();
    agent_ui::dispatch::clear_globals_for_test();
}

/// Drain until the external row wears `title` — the bytes cross a real
/// PTY-pump thread (tokio) before the gpui proxy pump emits the parsed
/// event, so the arrival is polled, not assumed.
fn wait_for_title(
    cx: &mut TestAppContext,
    shell: &gpui::Entity<steer_agent_chrome_ui::Shell>,
    title: &str,
) {
    for _ in 0..250 {
        cx.run_until_parked();
        let landed = shell.read_with(cx, |s, _| {
            s.sessions
                .iter()
                .any(|r| r.id.starts_with("external:") && r.title == title)
        });
        if landed {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let current: Vec<(String, String)> = shell.read_with(cx, |s, _| {
        s.sessions
            .iter()
            .filter(|r| r.id.starts_with("external:"))
            .map(|r| (r.id.clone(), r.title.clone()))
            .collect()
    });
    panic!("the external row never wore the title {title:?}; current: {current:?}");
}

/// Mount the REAL chrome assembly (the production `chrome_assembly::mount`)
/// and capture the shell + the workspace it created.
fn open_assembly(
    cx: &mut TestAppContext,
) -> (
    gpui::WindowHandle<Root>,
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
        register_fonts(cx);
        let shell = agent_ui::chrome_assembly::mount(window, cx);
        let ws =
            agent_ui::dispatch::workspace_global().expect("the assembly created the workspace");
        *shell_for_build.borrow_mut() = Some(shell.clone());
        *ws_for_build.borrow_mut() = Some(ws);
        Root::new(shell, window, cx)
    });
    cx.run_until_parked();
    let shell = shell_cell.borrow().clone().expect("shell captured");
    let ws = ws_cell.borrow().clone().expect("workspace captured");
    (window, shell, ws)
}
