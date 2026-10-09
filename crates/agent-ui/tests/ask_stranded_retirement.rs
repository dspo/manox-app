//! Regression (manox-app#88, composer half): the live-ask edge must retire a
//! card whose request the fold can no longer route. A card the fold lost
//! parks the composer in ask-supplement mode against a request
//! `resolve_ask` cannot answer — submit becomes a silent no-op until the
//! card is gone. The fold is the only answer route, so a fold without the
//! id retires; a later fold that regains the request re-seeds (self-healing).
#![cfg(feature = "test-support")]

mod common;

use ahp_types::actions::{
    ChatInputCompletedAction, ChatInputRequestedAction, ChatTurnStartedAction, StateAction,
};
use ahp_types::state::{
    ChatInputOption, ChatInputQuestion, ChatInputRequest, ChatInputSingleSelectQuestion,
};
use common::{init_harness, open_workspace};
use gpui::{AppContext as _, TestAppContext, VisualTestContext};
use steer_agent_chat_ui::ahp_store::{AhpStore, chat_uri};

fn select_request(id: &str) -> ChatInputRequest {
    ChatInputRequest {
        id: id.to_string(),
        message: Some("the agent is asking a question".to_string()),
        url: None,
        questions: Some(vec![ChatInputQuestion::SingleSelect(
            ChatInputSingleSelectQuestion {
                id: "q1".to_string(),
                title: Some("Pick".to_string()),
                message: "Which one?".to_string(),
                required: None,
                options: vec![
                    ChatInputOption {
                        id: "a".to_string(),
                        label: "A".to_string(),
                        description: None,
                        recommended: None,
                    },
                    ChatInputOption {
                        id: "b".to_string(),
                        label: "B".to_string(),
                        description: None,
                        recommended: None,
                    },
                ],
                allow_freeform_input: None,
            },
        )]),
        answers: None,
    }
}

fn user_message() -> ahp_types::state::Message {
    serde_json::from_value(serde_json::json!({
        "text": "run it",
        "origin": { "kind": "user" },
    }))
    .expect("a user message parses")
}

#[gpui::test]
async fn live_ask_edge_retires_a_card_the_fold_cannot_route(cx: &mut TestAppContext) {
    init_harness(cx);
    let (_window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(_window.into(), cx);

    let store = cx.update(|cx| cx.new(|_| AhpStore::detached()));
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store.clone(), "sess-1", cx);
    });

    // The fold gains the request the production way — through the AHP chat
    // reducer (turn first: the reducer parks input requests on it).
    store.update(&mut visual.cx, |s, _| {
        s.book.apply(
            &chat_uri("sess-1"),
            &StateAction::ChatTurnStarted(ChatTurnStartedAction {
                turn_id: "turn-1".to_string(),
                started_at: "2026-09-30T00:00:00Z".to_string(),
                message: user_message(),
                queued_message_id: None,
                meta: None,
            }),
        );
        s.book.apply(
            &chat_uri("sess-1"),
            &StateAction::ChatInputRequested(ChatInputRequestedAction {
                request: select_request("req-1"),
            }),
        );
    });

    // The edge seeds the card from the fold — the attach re-seed path runs
    // exactly this.
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_sync_live_ask(cx);
    });
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_pending_ask_id(cx)),
        Some("req-1".to_string()),
        "an open elicitation in the fold seeds the interactive card"
    );

    // The request settles (an answer rode the wire): its part carries a
    // response, the fold routes nothing, and the live-seeded card retires.
    store.update(&mut visual.cx, |s, _| {
        s.book.apply(
            &chat_uri("sess-1"),
            &StateAction::ChatInputCompleted(ChatInputCompletedAction {
                request_id: "req-1".to_string(),
                response: ahp_types::state::ChatInputResponseKind::Accept,
                answers: None,
            }),
        );
    });
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_sync_live_ask(cx);
    });
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_pending_ask_id(cx)),
        None,
        "a settled elicitation retires the live-seeded card"
    );

    // The stranded shape: a card whose id the fold never carried (the forward
    // was swallowed mid-flood, or the seed predates the fold loss). The edge
    // must retire it — leaving it up parks the composer in ask-supplement
    // mode against an unroutable request, the #88 dead-lock.
    let payload = serde_json::json!({
        "questions": [
            {"question": "Which one?", "header": "Pick", "multiSelect": false,
             "options": [{"label": "A", "description": ""}, {"label": "B", "description": ""}]}
        ]
    });
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_seed_ask("req-2", payload, cx);
    });
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_pending_ask_id(cx)),
        Some("req-2".to_string()),
        "the stranded card is up before the edge runs"
    );
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_sync_live_ask(cx);
    });
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_pending_ask_id(cx)),
        None,
        "a card the fold cannot route retires instead of stranding the composer"
    );

    // The plan-review zombie (the #88 device repro): the review's turn
    // finishes before the verdict lands, so the chat-level part can never
    // fold answered — the reducer only settles active-turn parts. The
    // verdict on the plan channel is the retirement signal.
    let plan_channel = format!("{}sess-1", manox_ahp::ext::channels::PLAN);
    store.update(&mut visual.cx, |s, _| {
        s.book.apply(
            &chat_uri("sess-1"),
            &StateAction::ChatInputRequested(ChatInputRequestedAction {
                request: select_request("plan-review:e-3"),
            }),
        );
        // The plan channel must exist before its deltas fold (the baseline
        // creates the ext entry).
        s.book.apply(
            &plan_channel,
            &StateAction::Unknown(serde_json::json!({
                "type": manox_ahp::ext::actions::BASELINE,
                "state": {},
            })),
        );
        s.book.apply(
            &plan_channel,
            &StateAction::Unknown(serde_json::json!({
                "type": manox_ahp::ext::actions::PLAN_VERDICT_REQUESTED,
                "requestId": "plan-review:e-3",
                "content": "# the plan",
            })),
        );
    });
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_sync_live_ask(cx);
    });
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_pending_ask_id(cx)),
        Some("plan-review:e-3".to_string()),
        "an open plan review seeds its card"
    );

    // The verdict lands: the card must retire even though the chat-level
    // part stays unanswered in its archived turn.
    store.update(&mut visual.cx, |s, _| {
        s.book.apply(
            &plan_channel,
            &StateAction::Unknown(serde_json::json!({
                "type": manox_ahp::ext::actions::PLAN_REVIEW_SETTLED,
                "requestId": "plan-review:e-3",
            })),
        );
    });
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_sync_live_ask(cx);
    });
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_pending_ask_id(cx)),
        None,
        "a plan review settled on the plan channel retires the card"
    );
    manox_agent::thread_store::drop_global_for_test();
}
