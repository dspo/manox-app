//! Regression: a history-loading gate that outlives its timeout must degrade
//! the page in the SAME frame — the render pass that observes an expired gate
//! prunes it before deriving the body facts, so the loading page never builds
//! past its timeout and the hero screen needs no extra notify cycle. A failed
//! reopen must not pin the loading page forever.
//!
//! Lives in its own test binary: the shared harness initializes
//! process-global singletons (`manox_agent::runtime`, `pi_providers`,
//! `thread_store`) whose global store entity is app-affine, so each binary
//! holds exactly ONE `#[gpui::test]` (see `tests/common/mod.rs` and the
//! `workspace_overlap` binary for the same constraint).
#![cfg(feature = "test-support")]

mod common;

use agent_ui::views::history_loading::take_history_loading_page_builds;
use common::{init_harness, landing_thread, open_workspace};
use gpui::{AppContext as _, TestAppContext, VisualTestContext};

#[gpui::test]
async fn gate_past_its_timeout_degrades_in_the_same_frame(cx: &mut TestAppContext) {
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

    // The snapshot never lands (no host in the test), so the gate holds and
    // its loading page is what the window paints. The pre-timeout facts make
    // the post-timeout assertions meaningful: the page was being built, and
    // the selector records it.
    assert!(
        visual.debug_bounds("history-loading").is_some(),
        "an armed gate paints the loading page"
    );
    assert!(
        take_history_loading_page_builds() > 0,
        "the armed gate builds the loading page into every frame"
    );

    // Backdate past the timeout. Every frame drawn from here on — including
    // the one the update's effect flush paints — observes an expired gate,
    // and each of those render passes must prune the gate BEFORE deriving the
    // body facts: the loading page must not be built again, and one pass must
    // be enough to reach the degraded page (no extra notify cycle).
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_backdate_history_gate(cx);
    });
    cx.run_until_parked();

    let pruned = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_awaiting_history(cx));
    assert!(
        pruned.is_none(),
        "a failed reopen must exit to the hero after the timeout"
    );
    assert!(
        take_history_loading_page_builds() == 0,
        "an expired gate must degrade the page in the same render pass — \
         the loading page must not be built after the expiry"
    );
    assert!(
        visual.debug_bounds("history-loading").is_none(),
        "the frame after the expiry renders the degraded page, not the loading page"
    );
    manox_agent::thread_store::drop_global_for_test();
}
