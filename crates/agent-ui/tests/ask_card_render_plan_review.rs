//! Render-level layout contract for the plan-review decision card: the card
//! paints the plan scrollport AND a bottom decision row — the row lands
//! at-or-below the body's bottom edge AND inside the viewport; a layout
//! regression that stacks the footer over the plan or pushes it out of the
//! window fails here before a human has to squint.
//!
//! Own test binary on purpose, exactly one `#[gpui::test]` per binary: the
//! probes drive `window.draw` under the gpui deterministic scheduler, which
//! asserts no thread strays — while `init_harness` starts a real tokio
//! runtime whose workers register as foreign-thread activity, and the
//! harness's process-global singletons (`tests/common/mod.rs`) make two
//! tests in one process overwrite each other's store (the `workspace_overlap`
//! binary is the same shape: one process, window-drawing only).
#![cfg(feature = "test-support")]

mod common;

use common::{AskCardProbe, PROBE_WINDOW_HEIGHT, init_harness, open_workspace};
use gpui::{AppContext as _, TestAppContext, VisualTestContext, px, size};

#[gpui::test]
async fn plan_review_decision_row_renders_below_the_plan_body(cx: &mut TestAppContext) {
    init_harness(cx);
    let (_window, workspace) = open_workspace(cx);
    let payload = serde_json::json!({
        "questions": [
            {
                "question": "Review the plan.",
                "header": "Plan",
                "detail": "# Plan\n\n- step one\n- step two",
                "intent": { "kind": "plan-review", "approve": "Approve" },
                "options": [
                    { "label": "Approve", "description": "start executing" },
                    { "label": "Approve & compact" },
                    { "label": "Request changes" }
                ],
            }
        ]
    });
    cx.update(|cx| {
        workspace.update(cx, |ws, cx| ws.diagnostic_seed_ask("ask1", payload, cx));
    });
    let probe = cx.open_window(size(px(1_120.), PROBE_WINDOW_HEIGHT), {
        let ws = workspace.clone();
        move |_, _| AskCardProbe { ws }
    });
    let mut visual = VisualTestContext::from_window(probe.into(), cx);
    for _ in 0..3 {
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }
    let body = visual
        .debug_bounds("plan-review-body-0")
        .expect("the plan scrollport renders");
    let footer = visual
        .debug_bounds("plan-review-footer-0")
        .expect("the decision row renders");
    assert!(
        footer.top() >= body.bottom(),
        "the decision row paints below the plan body, never above the content it settles"
    );
    assert!(
        footer.bottom() <= PROBE_WINDOW_HEIGHT,
        "the decision row stays inside the viewport on a long plan"
    );
    manox_agent::thread_store::drop_global_for_test();
}
