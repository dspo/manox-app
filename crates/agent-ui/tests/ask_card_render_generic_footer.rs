//! Render-level layout contract for the generic stepper ask card: the pager,
//! skip and the primary action land at-or-below the scrollable body and
//! inside the viewport — a long detail caps the body instead of pushing the
//! footer out of reach.
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
async fn generic_ask_footer_renders_below_the_body(cx: &mut TestAppContext) {
    init_harness(cx);
    let (_window, workspace) = open_workspace(cx);
    let payload = serde_json::json!({
        "questions": [
            {
                "question": "Which one?",
                "header": "Pick",
                "detail": "some supporting text",
                "options": [
                    { "label": "A" },
                    { "label": "B" },
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
        .debug_bounds("ask-card-body-0-0")
        .expect("the ask body renders");
    let footer = visual
        .debug_bounds("ask-card-footer-0-0")
        .expect("the bottom footer renders");
    assert!(
        footer.top() >= body.bottom(),
        "pager + skip + submit paint below the body as the bottom footer"
    );
    assert!(
        footer.bottom() <= PROBE_WINDOW_HEIGHT,
        "the footer stays inside the viewport"
    );
    manox_agent::thread_store::drop_global_for_test();
}
