//! Regression: reopening a thread whose chat snapshot is still in flight must
//! arm the history-loading gate (the loading page replaces the hero).
//!
//! Lives in its own test binary: the shared harness initializes
//! process-global singletons (`manox_agent::runtime`, `pi_providers`,
//! `thread_store`) whose global store entity is app-affine, so each binary
//! holds exactly ONE `#[gpui::test]` (see `tests/common/mod.rs` and the
//! `workspace_overlap` binary for the same constraint).
#![cfg(feature = "test-support")]

mod common;

use common::{init_harness, landing_thread, open_workspace};
use gpui::{AppContext as _, TestAppContext, VisualTestContext};

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
