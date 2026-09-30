//! Regression: a history-loading gate that outlives its timeout must prune
//! itself (a failed reopen cannot pin the loading page forever).
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
