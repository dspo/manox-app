//! The ask card is the projection's: the fold's open elicitation (an `ir:`
//! part) renders as the interactive card, and the workspace's pending-ask
//! state attaches its snapshot to that one row. This pins the two faces that
//! used to need a synthesis leg — a transcript whose tool call folded into a
//! segment carries no ask card until the fold holds the elicitation, and the
//! elicitation yields exactly one interactive card.
#![cfg(feature = "test-support")]

mod common;

use common::{init_harness, open_workspace};
use gpui::{AppContext as _, TestAppContext, VisualTestContext};
use steer_agent_chat_ui::ahp_store::{AhpStore, chat_uri};

fn user_message() -> ahp_types::state::Message {
    serde_json::from_value(serde_json::json!({
        "text": "run the build",
        "origin": { "kind": "user" },
    }))
    .expect("a user message parses")
}

fn select_request(id: &str) -> ahp_types::state::ChatInputRequest {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "questions": [{
            "kind": "single-select", "id": "q", "title": "Pick",
            "message": "Which one?",
            "options": [
                { "id": "a", "label": "A" },
                { "id": "b", "label": "B" },
            ],
        }],
    }))
    .expect("the select request parses")
}

#[gpui::test]
async fn the_folds_elicitation_is_the_one_interactive_ask_card(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let store = cx.update(|cx| cx.new(|_| AhpStore::detached()));
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store.clone(), "sess-1", cx);
    });

    // A settled turn whose Bash call folded into an activity segment: no
    // top-level ask card exists.
    store.update(&mut visual.cx, |s, _| {
        s.book.apply(
            &chat_uri("sess-1"),
            &ahp_types::actions::StateAction::ChatTurnStarted(
                ahp_types::actions::ChatTurnStartedAction {
                    turn_id: "turn-1".to_string(),
                    started_at: "2026-09-30T00:00:00Z".to_string(),
                    message: user_message(),
                    queued_message_id: None,
                    meta: None,
                },
            ),
        );
        s.book.apply(
            &chat_uri("sess-1"),
            &ahp_types::actions::StateAction::ChatToolCallStart(
                ahp_types::actions::ChatToolCallStartAction {
                    turn_id: "turn-1".to_string(),
                    tool_call_id: "tc-1".to_string(),
                    meta: None,
                    tool_name: "Bash".to_string(),
                    display_name: "Bash".to_string(),
                    intention: Some("Run the build".to_string()),
                    contributor: None,
                },
            ),
        );
    });
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_sync_conversation(cx);
    });
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws
            .diagnostic_tool_call_count("ask1", cx)),
        0,
        "a transcript without an elicitation carries no ask card"
    );

    // The elicitation arrives the production way — an `ir:` part on the
    // running turn — and the pending-ask state arms from the same fold.
    store.update(&mut visual.cx, |s, _| {
        s.book.apply(
            &chat_uri("sess-1"),
            &ahp_types::actions::StateAction::ChatInputRequested(
                ahp_types::actions::ChatInputRequestedAction {
                    request: select_request("ask1"),
                },
            ),
        );
    });
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_sync_conversation(cx);
        ws.diagnostic_sync_live_ask(cx);
    });
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_sync_ask_card_snapshots(cx);
    });

    let interactive = workspace.read_with(&visual.cx, |ws, cx| {
        ws.diagnostic_ask_card_interactive("ask1", cx)
    });
    assert!(interactive, "the projected ask card must be interactive");
    let cards = workspace.read_with(&visual.cx, |ws, cx| {
        ws.diagnostic_tool_call_count("ask1", cx)
    });
    assert_eq!(cards, 1, "exactly one ask card");
    manox_agent::thread_store::drop_global_for_test();
}
