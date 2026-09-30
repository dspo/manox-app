//! Regression: an ask the user dismissed came right back when the engine's
//! restore re-parked it (upstream #840). The decline dispatched on close
//! raced the restore and reduced to NoOp host-side, the fold re-opened the
//! same request, and the live-ask edge re-seeded the card the user had just
//! closed — with the turn still open, the composer ping-ponged between ask
//! and stop until a manual cancel. Inside the re-decline window the edge now
//! re-issues the decline for the re-parked request instead of re-seeding.
//!
//! Own test binary: the shared harness initializes process-global singletons
//! (see `tests/common/mod.rs`).
#![cfg(feature = "test-support")]

mod common;

use ahp_types::actions::{ChatInputRequestedAction, ChatTurnStartedAction, StateAction};
use ahp_types::state::{
    ChatInputOption, ChatInputQuestion, ChatInputRequest, ChatInputSingleSelectQuestion,
};
use common::{init_harness, open_workspace};
use gpui::{AppContext as _, TestAppContext, VisualTestContext};
use manox_agent_chat_ui::ahp_store::{AhpStore, chat_uri};

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
async fn a_dismissed_ask_redeclines_instead_of_reseeding_when_reparked(cx: &mut TestAppContext) {
    init_harness(cx);
    let (_window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(_window.into(), cx);

    let store = cx.update(|cx| cx.new(|_| AhpStore::detached()));
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store.clone(), "sess-1", cx);
    });

    // The fold holds the previous run's unsettled question (the re-park).
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
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_sync_live_ask(cx);
    });
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_pending_ask_id(cx)),
        Some("req-1".to_string()),
        "the stale ask seeds its card"
    );

    // The user closes it (the stop button's dismiss leg): the decline
    // dispatches, but a detached store has no echo — the fold still holds
    // the request, the re-parked shape after a restore race.
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_dismiss_ask(cx);
    });
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_pending_ask_id(cx)),
        None,
        "the dismissal clears the card"
    );

    // The re-park: the fold re-opens the SAME request. The edge must re-issue
    // the decline instead of re-seeding the card the user just closed.
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_sync_live_ask(cx);
    });
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_pending_ask_id(cx)),
        None,
        "a re-parked ask inside the decline window does not re-seed"
    );
    // And the fold still routes the id (nothing settled it), so a later edge
    // keeps the decline posture rather than stranding a ghost card.
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_sync_live_ask(cx);
    });
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_pending_ask_id(cx)),
        None,
        "the re-decline posture persists while the fold holds the request"
    );
    manox_agent::thread_store::drop_global_for_test();
}
