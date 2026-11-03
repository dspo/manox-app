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

use common::{FakePty, init_harness, open_assembly};
use gpui::{AppContext as _, TestAppContext, VisualTestContext};

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
                    // The fake PTY has no cx session behind it.
                    handle: None,
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
    // The REAL sidebar-click path: open_container parks the external session
    // (its first move) and attaches the thread.
    cx.update_window(any, |_, window, cx| {
        ws.update(cx, |ws, cx| {
            ws.open_container("park-target".into(), window, cx)
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
