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
use manox_agent_chrome_ui::register_fonts;

/// A PTY source that never starts — the test never renders terminal content,
/// only mounts the view and exercises the session state machine.
struct FakePty;

impl manox_terminal::pty_source::PtySource for FakePty {
    fn start(&mut self, _event_tx: async_channel::Sender<manox_terminal::event::TerminalEvent>) {}
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
    ws.update(cx, |ws, cx| {
        let terminal = manox_terminal::Terminal::spawn(
            "test-ext-pty".into(),
            std::path::PathBuf::from("/tmp"),
            80,
            24,
            Box::new(FakePty),
        )
        .expect("fake terminal spawns");
        let proxy = cx.new(|cx| terminal_ui::terminal_proxy::TerminalProxy::new(terminal, cx));
        let view = terminal_ui::TerminalView::new(proxy, cx);
        ws.spawn_external_session("Claude Code".into(), "icons/claude.svg", None, view, cx);
    });
    cx.run_until_parked();

    // The session foregrounds immediately (spawn = register + main column).
    assert!(
        ws.read_with(cx, |ws, _| ws.external_session_foreground()),
        "spawning foregrounds the session in the main column"
    );

    // The sidebar carries the row (brand-mark kind, project group merged).
    let row = visual
        .debug_bounds("chrome-session-row-ext-0001")
        .unwrap_or_else(|| {
            panic!(
                "the external session's row must paint in the sidebar; rows: {:?}",
                shell.read_with(cx, |s, _| s
                    .sessions
                    .iter()
                    .map(|r| (r.id.clone(), r.title.clone()))
                    .collect::<Vec<_>>())
            )
        });

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
        Some("ext-0001".to_string()),
        "the shell's highlight follows the clicked external row"
    );

    manox_agent::thread_store::drop_global_for_test();
    agent_ui::dispatch::clear_globals_for_test();
}

/// Mount the REAL chrome assembly (the production `chrome_assembly::mount`)
/// and capture the shell + the workspace it created.
fn open_assembly(
    cx: &mut TestAppContext,
) -> (
    gpui::WindowHandle<Root>,
    gpui::Entity<manox_agent_chrome_ui::Shell>,
    gpui::Entity<Workspace>,
) {
    let shell_cell: std::rc::Rc<
        std::cell::RefCell<Option<gpui::Entity<manox_agent_chrome_ui::Shell>>>,
    > = std::rc::Rc::new(std::cell::RefCell::new(None));
    let ws_cell: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let (shell_for_build, ws_for_build) = (shell_cell.clone(), ws_cell.clone());

    cx.update(gpui_component::init);
    cx.update(|_cx| {
        manox_i18n::init();
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
