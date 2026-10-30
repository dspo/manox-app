//! FULL-WIRING regression for the attach tail's mode guard. A stale async
//! attach must not steal the main column back from a mode the user entered
//! while it was in flight — Settings (the injected nav's only exit is the
//! sidebar; flipping past it strands the nav), an external session and the
//! sub-agent page alike (the four-mode predicate is pinned by `attach.rs`'s
//! own unit test; this binary drives the Settings and external-session legs
//! through the real assembly).
//!
//! Own test binary: the harness initializes process-global singletons (see
//! `tests/common/mod.rs` — one `#[gpui::test]` per binary).
#![cfg(feature = "test-support")]

mod common;

use common::{FakePty, init_harness, landing_thread, open_assembly};
use gpui::{AppContext as _, TestAppContext};

#[gpui::test]
async fn a_stale_attach_never_flips_past_settings_or_an_external_session(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, shell, ws) = open_assembly(cx);

    // The user keeps the sidebar collapsed; Settings must not cost them that
    // preference — the pin is a loan, not an override.
    cx.update(|cx| shell.update(cx, |shell, _| shell.show_sidebar = false));

    // Settings owns the column and the injected nav owns the sidebar. The
    // pin forces the sidebar visible for the duration.
    cx.update_window(window.into(), |_, window, cx| {
        ws.update(cx, |ws, cx| ws.enter_settings(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        ws.read_with(cx, |ws, _| ws.settings_foreground()),
        "enter_settings owns the main column"
    );
    assert!(
        shell.read_with(cx, |s, _| s.sidebar_surface.is_some()),
        "the settings nav owns the sidebar slot"
    );
    assert!(
        shell.read_with(cx, |s, _| s.show_sidebar),
        "the pin forces the sidebar up while the nav is mounted"
    );

    // ⌘, inside Settings re-enters (a second mount while pinned): it must not
    // overwrite the remembered preference the first mount recorded.
    cx.update_window(window.into(), |_, window, cx| {
        ws.update(cx, |ws, cx| ws.enter_settings(window, cx));
    })
    .unwrap();
    cx.run_until_parked();

    // The stale attach tail lands: Settings must survive it (the pre-fix
    // regression flipped the column back and stranded the nav).
    cx.update_window(window.into(), |_, window, cx| {
        ws.update(cx, |ws, cx| {
            ws.diagnostic_attach_thread(landing_thread("late-attach"), true, window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        ws.read_with(cx, |ws, _| ws.settings_foreground()),
        "a stale attach must not flip past Settings"
    );
    assert!(
        shell.read_with(cx, |s, _| s.sidebar_surface.is_some()),
        "the settings nav keeps the sidebar slot"
    );

    // Leave Settings; the pin releases, the session list is back, and the
    // collapsed preference is restored.
    cx.update_window(window.into(), |_, window, cx| {
        ws.update(cx, |ws, cx| ws.leave_settings(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        !ws.read_with(cx, |ws, _| ws.settings_foreground()),
        "leave_settings returns to the conversation"
    );
    assert!(
        shell.read_with(cx, |s, _| s.sidebar_surface.is_none()),
        "the session list owns the sidebar slot again"
    );
    assert!(
        !shell.read_with(cx, |s, _| s.show_sidebar),
        "the pin restores the user's collapsed sidebar"
    );

    // Same guard for an external session: foregrounded, then a stale attach
    // lands — the session must keep the column (the exact regression the
    // round-2 review caught).
    let pty = FakePty::default();
    cx.update_window(window.into(), |_, window, cx| {
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
    assert!(
        ws.read_with(cx, |ws, _| ws.external_session_foreground()),
        "spawning foregrounds the session"
    );
    cx.update_window(window.into(), |_, window, cx| {
        ws.update(cx, |ws, cx| {
            ws.diagnostic_attach_thread(landing_thread("late-attach-2"), true, window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        ws.read_with(cx, |ws, _| ws.external_session_foreground()),
        "a stale attach must not flip past an external session"
    );
}
