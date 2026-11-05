//! FULL-WIRING regression for `/exit`'s scope (container model D6/D8):
//! `/exit` is the OUTGOING container's death only. Its own resources — the
//! right-pane CLI agent tabs' children at their registered kill handles —
//! die with it; background EXTERNAL containers are INDEPENDENT containers
//! with their own 0 号 processes and survive it (row, record and process
//! intact). The app process quitting is the only thing that ends the
//! survivors, by design.
//!
//! Own test binary: the harness initializes process-global singletons (see
//! `tests/common/mod.rs` — one `#[gpui::test]` per binary).
#![cfg(feature = "test-support")]

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use agent_ui::workspace::external_sessions::ExternalSessionLaunch;
use gpui::{AppContext as _, TestAppContext};
use steer_ext_agents::SessionHandle;

/// A REAL child (`/bin/cat` — blocks forever with no input) behind a session
/// handle, so death and survival are both observable: `wait` returns only
/// once the child actually died, and silence means it lived on.
fn spawn_cat() -> (tempfile::TempDir, Arc<SessionHandle>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("cat.sock");
    let spec = steer_ext_agents::LaunchSpec {
        program: PathBuf::from("/bin/cat"),
        args: vec![],
        env: BTreeMap::new(),
        summary: String::new(),
        detach: false,
        env_remove: vec![],
        agent_id: "test".into(),
        provider_name: "test".into(),
        model_id: None,
        pty: true,
        socket: Some(socket.to_string_lossy().into_owned()),
        cwd: None,
    };
    let handle = Arc::new(SessionHandle::spawn(&spec, None).expect("spawn /bin/cat"));
    (dir, handle)
}

/// Poll until the child behind `handle` is dead — `wait` reaps it and
/// reports a signal termination only when OUR kill ended it.
fn wait_child_dead(cx: &mut TestAppContext, handle: Arc<SessionHandle>) -> bool {
    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    std::thread::spawn(move || {
        let killed = handle
            .wait()
            .ok()
            .is_some_and(|result| result.termination.is_some());
        let _ = tx.send(killed);
    });
    for _ in 0..250 {
        cx.run_until_parked();
        if let Ok(killed) = rx.try_recv() {
            return killed;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    false
}

/// Register an external container the way the project menu does (the record
/// lands through the unified container entry) and return its row id.
fn spawn_external(
    cx: &mut TestAppContext,
    ws: &gpui::Entity<agent_ui::Workspace>,
    window: gpui::AnyWindowHandle,
    handle: Arc<SessionHandle>,
) -> String {
    let fixture = common::ghostty_fixture(cx);
    let ws = ws.clone();
    cx.update_window(window, |_, window, cx| {
        ws.update(cx, |ws, cx| {
            ws.spawn_external_session(
                ExternalSessionLaunch {
                    agent_id: "claude",
                    cx_session_id: String::new(),
                    label: "Claude Code".into(),
                    svg: "icons/claude.svg",
                    project: None,
                    cwd: std::path::PathBuf::from("/tmp"),
                    handle: Some(handle),
                },
                fixture.view.clone(),
                window,
                cx,
            );
        });
    })
    .expect("spawn the external container");
    cx.run_until_parked();
    ws.read_with(cx, |ws, _| {
        ws.diagnostic_external_session_ids()
            .into_iter()
            .find(|id| id.starts_with("external:claude:"))
            .expect("the external container registered")
    })
}

#[gpui::test]
async fn exit_kills_its_own_resources_and_spares_background_externals(cx: &mut TestAppContext) {
    let (ws, window, _shell) = common::full_wiring_boot_shell(cx);
    let native = common::foreground_session(cx, &ws).expect("the landing session is bound");

    // (1) The outgoing NATIVE container's own right-pane CLI agent tab: its
    // kill handle wraps a REAL child (the production path registers this at
    // spawn; the death leg is the registry kill, not the view drop). The
    // waiter fires only when OUR kill ended the child.
    let (_dir, tab_child) = spawn_cat();
    ws.update(cx, |ws, _| {
        ws.diagnostic_register_kill_handle(&native, "claude-tab-1", tab_child.clone());
    });
    assert_eq!(
        ws.read_with(cx, |ws, _| ws.diagnostic_kill_handles()),
        vec![(native.clone(), "claude-tab-1".to_string())],
        "the CLI tab's child is a resource of the native container"
    );

    // (2) A background EXTERNAL container (sidebar-launched, its own real
    // child) — an independent container nothing in this test touches.
    let (_dir2, ext_child) = spawn_cat();
    let ext_row = spawn_external(cx, &ws, window, ext_child.clone());
    let (alive, _cwd) = ws
        .read_with(cx, |ws, _| ws.diagnostic_external_container(&ext_row))
        .expect("the external record is live");
    assert!(alive);

    // Another container runs `/exit` — the full waterfall (subagent beat,
    // turn beat, terminal beat, browser beat, teardown, launcher landing).
    common::run_slash(cx, &ws, window, "/exit");
    assert!(common::wait_launcher(cx, &ws), "/exit lands the launcher");

    // The native container's resource died with it: the archive leg's
    // container teardown killed the CLI tab's child.
    assert!(
        wait_child_dead(cx, tab_child),
        "/exit kills the outgoing container's own right-pane CLI tab child"
    );

    // The background external container SURVIVED: its record (the sidebar
    // row with it) is still there, its alive bit never flipped, and its
    // child process was never killed.
    assert!(
        ws.read_with(cx, |ws, _| ws.is_external_session(&ext_row)),
        "the background external container's row survives another container's /exit"
    );
    let (alive, _cwd) = ws
        .read_with(cx, |ws, _| ws.diagnostic_external_container(&ext_row))
        .expect("the external record survives");
    assert!(alive, "the external container's 0 号 never exited");
    // The child outlived the whole sequence: give the /exit waterfall's
    // deferred reclaims every chance to (wrongly) kill it, then assert the
    // death never came.
    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    std::thread::spawn(move || {
        let killed = ext_child
            .wait()
            .ok()
            .is_some_and(|result| result.termination.is_some());
        let _ = tx.send(killed);
    });
    for _ in 0..50 {
        cx.run_until_parked();
        assert!(
            rx.try_recv().is_err(),
            "/exit must not kill a background external container's process"
        );
        std::thread::sleep(std::time::Duration::from_millis(4));
    }

    common::full_wiring_teardown(cx);
}
