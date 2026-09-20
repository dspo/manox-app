//! Render-level layout contract for the ask-card presentations (drawn with
//! `debug_selector` geometry). Own test binary on purpose: the probes drive
//! `window.draw` under the gpui deterministic scheduler, which asserts no
//! thread strays — while `init_harness` starts a real tokio runtime whose
//! workers register as foreign-thread activity. Sharing a binary with
//! parallel siblings flaked 3/6 at the default concurrency, so the drawing
//! probes live alone here and serialize on a lock (the `workspace_overlap`
//! binary is the same shape: one process, window-drawing only).
#![cfg(feature = "test-support")]

mod common;

use agent_ui::Workspace;
use common::{init_harness, open_workspace};
use gpui::{Context, Entity, IntoElement, TestAppContext, VisualTestContext, Window, px, size};

/// Serializes the two window-drawing probes: the deterministic scheduler's
/// thread assertions are only safe while exactly one test drives windows in
/// this process.
static DRAW_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The probe window's height, the viewport bound the geometry assertions
/// check the footer against.
const PROBE_WINDOW_HEIGHT: gpui::Pixels = px(780.);

/// Minimal host that re-pulls the diagnostic ask-card element from the
/// workspace state on EVERY frame — an `AnyElement` is consumed by its first
/// paint, so a stored one would leave the later frames empty. The weak handle
/// is deliberately invalid: the build runs while the workspace entity is
/// `update`-held, and the card's render path upgrades the weak to read the
/// custom-input state, which would double-borrow. A geometry probe needs no
/// live custom row.
struct AskCardProbe {
    ws: Entity<Workspace>,
}

impl gpui::Render for AskCardProbe {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let invalid = gpui::WeakEntity::<Workspace>::new_invalid();
        let card = self
            .ws
            .update(cx, |ws, cx| ws.diagnostic_ask_card_element(invalid, 0, cx));
        card.unwrap_or_else(|| gpui::div().into_any_element())
    }
}

/// The plan-review decision card paints the plan scrollport AND a bottom
/// decision row: the row lands at-or-below the body's bottom edge AND inside
/// the viewport — a layout regression that stacks the footer over the plan
/// or pushes it out of the window fails here before a human has to squint.
#[gpui::test]
async fn plan_review_decision_row_renders_below_the_plan_body(cx: &mut TestAppContext) {
    let _draw = DRAW_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

/// Same contract on the generic stepper card: the pager, skip and the primary
/// action land at-or-below the scrollable body and inside the viewport — a
/// long detail caps the body instead of pushing the footer out of reach.
#[gpui::test]
async fn generic_ask_footer_renders_below_the_body(cx: &mut TestAppContext) {
    let _draw = DRAW_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
