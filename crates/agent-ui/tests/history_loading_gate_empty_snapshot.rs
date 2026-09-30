//! Regression: a chat snapshot that lands empty must clear the history-loading
//! gate (the hero returns instead of the loading page staying pinned).
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
