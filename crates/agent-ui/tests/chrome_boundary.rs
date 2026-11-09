//! The chrome/host boundary's regression matrix (#279): every gesture the
//! chrome hands to the host is fired under the SAME lease production fires it
//! in, and its chrome-side effect must land.
//!
//! Why this shape: the 0.3.0 aborts were not wrong arguments — they were
//! `shell.update` executing inside the shell's own update lease, and a
//! `WindowHandle::update` issued mid-event silently dropping the command. A
//! test that drives the host directly (the diagnostics' path) would miss both,
//! because it never holds the chrome entity's lease. `shell.update(|s, cx|
//! s.<gesture>(…))` reproduces exactly what `cx.listener` does, and
//! `update_window` reproduces the event dispatch that takes the window out of
//! the app map.
//!
//! Own test binary: the harness initializes process-global singletons (see
//! `tests/common/mod.rs` — one `#[gpui::test]` per binary).
#![cfg(feature = "test-support")]

mod common;

use gpui::{AppContext as _, Entity, TestAppContext};
use steer_agent_chrome_ui::Shell;

/// A patch the diff tab accepts (the tab renders, so the pane really owns it).
const PATCH: &str = "--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n";

fn click_row(
    cx: &mut TestAppContext,
    shell: &Entity<Shell>,
    window: gpui::AnyWindowHandle,
    id: &str,
) {
    // The sidebar row listener's exact lease: `cx.listener` wraps the handler
    // in `Shell::update`, and the handler calls `Shell::select`.
    cx.update_window(window, move |_, window, cx| {
        shell.update(cx, |shell, cx| shell.select(id, window, cx));
    })
    .expect("the full-wiring window is live");
}

fn fire(
    cx: &mut TestAppContext,
    window: gpui::AnyWindowHandle,
    f: impl FnOnce(&mut gpui::Window, &mut gpui::App),
) {
    cx.update_window(window, move |_, window, cx| f(window, cx))
        .expect("the full-wiring window is live");
}

#[gpui::test]
async fn chrome_gestures_survive_their_real_leases(cx: &mut TestAppContext) {
    let (ws, window, shell) = common::full_wiring_boot_shell(cx);
    let boot_foreground = common::foreground_session(cx, &ws).expect("boot foreground bound");

    // ── a row click: open + attach must not double-lease the shell ─────────
    // The production abort. A landing mirror binds the id synchronously, so
    // the foreground read is a real witness, not a timing accident.
    click_row(cx, &shell, window, "boundary-thread-1");
    cx.run_until_parked();
    assert_eq!(
        common::foreground_session(cx, &ws).as_deref(),
        Some("boundary-thread-1"),
        "the click opened the row"
    );

    // ── the outgoing thread's diff tab is really closed ───────────────────
    // The silent-drop regression: the attach's `close_tool_tabs_of_kind`
    // round-tripped through the window handle, which has no window to take
    // while an event dispatch is in flight — so it returned false and the
    // stale tab survived. With the deferred command channel it lands.
    cx.update(|cx| {
        ws.update(cx, |ws, cx| {
            ws.diagnostic_open_right_diff(Some("a.rs".into()), PATCH.to_string(), cx)
        })
    });
    cx.run_until_parked();
    assert!(
        !cx.update(|cx| agent_ui::chrome_assembly::pane_open_tab_ids(cx))
            .is_empty(),
        "the diff tab is open before the switch"
    );
    click_row(cx, &shell, window, "boundary-thread-2");
    cx.run_until_parked();
    assert_eq!(
        common::foreground_session(cx, &ws).as_deref(),
        Some("boundary-thread-2"),
        "the second click opened the second row"
    );
    assert!(
        cx.update(|cx| agent_ui::chrome_assembly::pane_open_tab_ids(cx))
            .is_empty(),
        "the switch closed the outgoing thread's diff tab (the silent drop is gone)"
    );

    // ── ⌘N / the New button: the launcher landing must not re-enter ───────
    fire(cx, window, |window, cx| {
        shell.update(cx, |shell, cx| shell.new_session(window, cx));
    });
    cx.run_until_parked();
    assert!(
        cx.update(|cx| ws.read(cx).diagnostic_view_mode_is_launcher()),
        "⌘N landed the launcher"
    );

    // ── the sidebar footer's ＋ 新建项目: the dialog mounts ────────────────
    fire(cx, window, |window, cx| {
        shell.update(cx, |shell, cx| shell.new_project(window, cx));
    });
    cx.run_until_parked();
    assert!(
        shell.read_with(cx, |shell, _| shell.has_dialog_surface()),
        "the projection mounted the dialog into the chrome slot"
    );
    cx.update(|cx| ws.update(cx, |ws, cx| ws.close_project_dialog(cx)));
    cx.run_until_parked();
    assert!(
        !shell.read_with(cx, |shell, _| shell.has_dialog_surface()),
        "closing the dialog unmounts the slot"
    );

    // ── the launcher's ⏎ runs under the LauncherState lease ───────────────
    // This is the gesture whose submitted payload must not be re-read by the
    // host (the abort recorded against a stale local build). The submission
    // is deferred, so the composer text is consumed here and the workspace
    // command runs after the lease is gone.
    cx.update_window(window, |_, window, cx| {
        ws.update(cx, |ws, cx| {
            ws.diagnostic_launcher_type("boundary first turn", window, cx);
            ws.diagnostic_launcher_press_enter(window, cx);
        })
    })
    .expect("the full-wiring window is live");
    cx.run_until_parked();
    assert_eq!(
        cx.update(|cx| ws.read(cx).diagnostic_launcher_text(cx)),
        "",
        "⏎ consumed the composer (the gesture ran)"
    );

    // ── the batch archive commit (shell lease -> host teardown) ────────────
    let target = common::foreground_session(cx, &ws).unwrap_or(boot_foreground);
    fire(cx, window, |window, cx| {
        shell.update(cx, |shell, cx| {
            shell.begin_selection(&target, cx);
            shell.commit_selection_archive(window, cx);
        });
    });
    cx.run_until_parked();

    common::full_wiring_teardown(cx);
}
