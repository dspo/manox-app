//! Regression: the AskUserQuestion card synthesized when the rebuilt
//! conversation lacks the top-level `ToolCall` item the interactive drawer
//! renders on. A parked interaction whose underlying ToolUse folded into an
//! activity segment leaves no card on a switch-back rebuild; the workspace
//! must synthesize the gate-created card.
#![cfg(feature = "test-support")]

mod common;

use agent_ui::Workspace;
use agent_ui::conversation::{ApplyCtx, ConversationState};
use common::{bash_tool_use_message, init_harness, open_workspace};
use gpui::{
    AppContext as _, Context, Entity, IntoElement, TestAppContext, VisualTestContext, Window, px,
    size,
};

#[gpui::test]
async fn ask_card_synthesized_when_rebuild_misses_the_tool_item(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let payload = serde_json::json!({
        "questions": [
            {
                "question": "Which one?",
                "header": "Pick",
                "options": [
                    { "label": "A" },
                    { "label": "B" },
                ],
            }
        ]
    });

    // The conversation a switch-back rebuilds: a Bash ToolUse folded into a
    // Thinking segment, no top-level AskUserQuestion card.
    let weak = gpui::WeakEntity::<Workspace>::new_invalid();
    let conversation = cx.new(|cx| {
        ConversationState::rebuild_from_display(
            &[manox_agent::db::HistoryEntry::Message(
                bash_tool_use_message("t1"),
            )],
            &std::collections::HashMap::new(),
            "test-model",
            manox_agent::MessageAuthor::Lead,
            true,
            ApplyCtx {
                weak,
                cwd: None,
                fork_source: None,
            },
            cx,
        )
    });
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_replace_conversation(conversation, cx);
    });
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws
            .diagnostic_tool_call_count("ask1", cx)),
        0,
        "rebuilt conversation must lack the top-level ask card"
    );

    // Re-surfacing the pending authorization: seed the ask and run the
    // synthesis the resurface loop performs per gate entry.
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_seed_ask("ask1", payload.clone(), cx);
        ws.diagnostic_ensure_ask_tool_item("ask1", "summary", payload.clone(), cx);
    });
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_sync_ask_card_snapshots(cx);
    });

    let interactive = workspace.read_with(&visual.cx, |ws, cx| {
        ws.diagnostic_ask_card_interactive("ask1", cx)
    });
    assert!(interactive, "synthesized ask card must be interactive");
    let cards = workspace.read_with(&visual.cx, |ws, cx| {
        ws.diagnostic_tool_call_count("ask1", cx)
    });
    assert_eq!(cards, 1, "exactly one ask card");
    manox_agent::thread_store::drop_global_for_test();
}

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

/// The ask-card layout contract, rendered for real: the plan-review decision
/// card paints the plan scrollport AND a bottom decision row — the approve
/// and discuss actions land BELOW the body's bounds, never above the content
/// they settle. A layout regression that pushes the footer out of the window
/// (or behind the composer overlay) fails here before a human has to squint.
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
    let probe = cx.open_window(size(px(1_120.), px(780.)), {
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
    manox_agent::thread_store::drop_global_for_test();
}

/// Same contract on the generic stepper card: pager, skip and the primary
/// action paint below the scrollable body — a long detail caps the body
/// instead of pushing the footer out of reach.
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
    let probe = cx.open_window(size(px(1_120.), px(780.)), {
        let ws = workspace.clone();
        move |_, _| AskCardProbe { ws }
    });
    let mut visual = VisualTestContext::from_window(probe.into(), cx);
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
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
    manox_agent::thread_store::drop_global_for_test();
}
