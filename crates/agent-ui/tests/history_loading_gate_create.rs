//! Regression: the create path (an attach that expects no history) must never
//! arm the history-loading gate.
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
