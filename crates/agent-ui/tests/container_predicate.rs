//! FULL-WIRING regression for the container object's death predicate (plan
//! §2.3): both homes flip `alive` through the ONE interface and raise the
//! death event — the external container dies when its PTY child exits (D8),
//! the native container dies when its session archives (D7). The creation
//! paths land through the unified `create_container` entry (the external
//! spawn here, the native `/exit` re-mint below); the deep draft-refill and
//! already-exists equivalences stay with their own binaries
//! (`new_thread_project_inheritance`, `zombie_draft_exit`).
//!
//! Own test binary: the harness initializes process-global singletons (see
//! `tests/common/mod.rs` — one `#[gpui::test]` per binary).
#![cfg(feature = "test-support")]

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use agent_ui::workspace::container::{ContainerEvent, ContainerKind};
use common::FakePty;
use gpui::{AppContext as _, TestAppContext};

#[gpui::test]
async fn container_death_flips_on_both_homes(cx: &mut TestAppContext) {
    let (ws, window) = common::full_wiring_boot(cx);

    // The death-event face: the workspace raises `ContainerEvent::Died` for
    // either home's flip. The subscription leaks — the observer lives for the
    // whole binary, the way the app's own observers do.
    let died: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = died.clone();
    let observed = ws.clone();
    cx.update_window(window, |_, _window, cx| {
        ws.update(cx, |_, cx| {
            let sub = cx.subscribe(
                &observed,
                move |_this, _emitter, ev: &ContainerEvent, _cx| match ev {
                    ContainerEvent::Died(id) => sink.borrow_mut().push(id.clone()),
                },
            );
            std::mem::forget(sub);
        });
    })
    .expect("subscribe on the live window");

    // ── native: archive is death (D7) ──
    // The boot landing session — a never-spoken container — reads alive.
    let landing = common::foreground_session(cx, &ws).expect("the landing session is bound");
    assert!(
        cx.update(|cx| ws.read(cx).container_alive(&landing, cx)),
        "a never-archived session reads alive"
    );
    // `/exit` archives the landing and mints a fresh container through the
    // unified entry; the archived session's predicate flips.
    let after_exit = common::run_slash_until_foreground_moves(cx, &ws, window, "/exit")
        .expect("/exit lands a fresh conversation");
    assert_ne!(
        after_exit, landing,
        "/exit must not reopen the archived one"
    );
    cx.run_until_parked();
    assert!(
        !cx.update(|cx| ws.read(cx).container_alive(&landing, cx)),
        "the archived session reads dead"
    );
    assert!(
        cx.update(|cx| ws.read(cx).container_alive(&after_exit, cx)),
        "the fresh container reads alive"
    );
    assert!(
        died.borrow().iter().any(|id| id == &landing),
        "the native flip raised the death event"
    );

    // ── external: the process exit is death (D8) ──
    // Launch an external session the way the project menu does (the record
    // lands through the unified container entry).
    let pty = FakePty::default();
    cx.update_window(window, |_, window, cx| {
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
    .expect("spawn the external container");
    cx.run_until_parked();

    let row_id = ws
        .read_with(cx, |ws, _| {
            ws.diagnostic_external_session_ids()
                .into_iter()
                .find(|id| id.starts_with("external:claude:"))
        })
        .expect("the external container registered");
    // The record's container face: alive, the 0 号's kind, the spawn cwd
    // frozen into the attrs snapshot.
    let (alive, kind, cwd) = ws
        .read_with(cx, |ws, _| ws.diagnostic_external_container(&row_id))
        .expect("the record carries the container face");
    assert!(alive, "a fresh external container reads alive");
    assert_eq!(kind, Some(ContainerKind::Claude));
    assert_eq!(cwd, std::path::PathBuf::from("/tmp"));
    assert!(
        cx.update(|cx| ws.read(cx).container_alive(&row_id, cx)),
        "the ONE predicate reads the external home"
    );
    assert_eq!(
        ws.read_with(cx, |ws, _| ws.container_kind(&row_id)),
        Some(ContainerKind::Claude),
        "the kind read resolves through the record"
    );

    // The 0 号 exits: the PTY's child-exit event flips the predicate. The
    // bytes cross the kernel pump before the gpui proxy emits, so the flip
    // is polled, not assumed.
    pty.event_tx
        .lock()
        .unwrap()
        .as_ref()
        .expect("pty started")
        .send_blocking(manox_terminal::event::TerminalEvent::ChildExit(0))
        .expect("event channel open");
    let mut flipped = false;
    for _ in 0..250 {
        cx.run_until_parked();
        if !cx.update(|cx| ws.read(cx).container_alive(&row_id, cx)) {
            flipped = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(
        flipped,
        "the child exit flips the external container's alive bit"
    );
    assert!(
        died.borrow().iter().any(|id| id == &row_id),
        "the external flip raised the death event"
    );
    // Since the teardown PR (A2): the death ALSO drops the record — the
    // sidebar row goes with it — and reclaims the container's resources
    // (the workspace's own subscription runs the ONE teardown).
    let mut row_gone = false;
    for _ in 0..250 {
        cx.run_until_parked();
        if !ws.read_with(cx, |ws, _| {
            ws.diagnostic_external_session_ids()
                .iter()
                .any(|id| id == &row_id)
        }) {
            row_gone = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(
        row_gone,
        "the external container's death drops its record (the row with it)"
    );
    assert!(
        !cx.update(|cx| ws.read(cx).container_alive(&row_id, cx)),
        "a closed external id reads dead (the record is gone)"
    );

    common::full_wiring_teardown(cx);
}
