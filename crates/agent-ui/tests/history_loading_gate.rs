//! Regression: the history-loading gate. A reopened thread whose chat
//! snapshot is still in flight must arm the gate (the loading page replaces
//! the hero), an empty snapshot must clear it (the hero returns), and a gate
//! that outlives the timeout must prune itself (a failed reopen cannot pin
//! the page forever).
#![cfg(feature = "test-support")]

mod common;

use common::{init_harness, open_workspace};
use gpui::{AppContext as _, TestAppContext, VisualTestContext};

fn landing_thread(id: &str) -> manox_agent::thread::ThreadHandle {
    manox_agent::thread::Thread::landing_with_id(
        manox_agent::ThreadId(id.to_string()),
        std::path::PathBuf::from("/tmp"),
    )
}

#[gpui::test]
async fn reopen_without_chat_snapshot_arms_the_gate(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let visual = VisualTestContext::from_window(window.into(), cx);

    let before = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_awaiting_history(cx));
    assert!(
        before.is_none(),
        "fresh workspace does not wait for history"
    );

    let any = window.into();
    cx.update_window(any, |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            ws.diagnostic_attach_thread(landing_thread("gate-a"), true, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();

    let armed = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_awaiting_history(cx));
    assert!(
        armed.is_some(),
        "reopen with no chat channel in the fold arms the gate"
    );
    manox_agent::thread_store::drop_global_for_test();
}

#[gpui::test]
async fn create_path_never_arms_the_gate(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let visual = VisualTestContext::from_window(window.into(), cx);

    let any = window.into();
    cx.update_window(any, |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            ws.diagnostic_attach_thread(landing_thread("gate-b"), false, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();

    let armed = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_awaiting_history(cx));
    assert!(
        armed.is_none(),
        "an attach that expects no history never arms the gate"
    );
    manox_agent::thread_store::drop_global_for_test();
}

#[gpui::test]
async fn empty_chat_snapshot_clears_the_gate(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let any = window.into();
    cx.update_window(any, |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            ws.diagnostic_attach_thread(landing_thread("gate-c"), true, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let armed = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_awaiting_history(cx));
    assert!(armed.is_some(), "precondition: gate armed by the reopen");

    // The chat snapshot lands and holds no turns: the genuinely-empty
    // session branch of the store observe must drop the gate.
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_apply_empty_chat_snapshot(cx);
    });
    cx.run_until_parked();

    let cleared = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_awaiting_history(cx));
    assert!(
        cleared.is_none(),
        "an empty history must return the hero, not pin the loading page"
    );
    manox_agent::thread_store::drop_global_for_test();
}

#[gpui::test]
async fn gate_past_its_timeout_prunes(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let any = window.into();
    cx.update_window(any, |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            ws.diagnostic_attach_thread(landing_thread("gate-d"), true, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();

    // The snapshot never lands (no host in the test): backdate past the
    // timeout and run the same prune the render loop drives.
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_backdate_history_gate(cx);
    });
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_prune_history_gate(cx);
    });

    let pruned = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_awaiting_history(cx));
    assert!(
        pruned.is_none(),
        "a failed reopen must exit to the hero after the timeout"
    );
    manox_agent::thread_store::drop_global_for_test();
}
