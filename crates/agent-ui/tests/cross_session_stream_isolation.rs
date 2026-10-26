//! Regression: derived display events must keep the envelope's channel
//! identity. AHP stamps every action envelope with its channel while chat
//! payloads carry no URI of their own, so the store buckets every derived
//! event under the envelope's owning session and the workspace drains only
//! the bucket of the session it binds — a parked session streaming in the
//! background must never paint the foreground transcript (the cross-thread
//! stream bleed: one thread's turn streamed live into another thread's
//! view).
//!
//! Own test binary: the shared harness initializes process-global singletons
//! (see `tests/common/mod.rs` — one `#[gpui::test]` per binary).
#![cfg(feature = "test-support")]

mod common;

use ahp_types::actions::{
    ActionEnvelope, ChatTurnCompleteAction, ChatTurnStartedAction, StateAction,
};
use common::{init_harness, open_workspace};
use gpui::{AppContext as _, TestAppContext, VisualTestContext};
use steer_agent_chat_ui::ahp_store::{AhpStore, chat_uri, session_uri};

fn user_message() -> ahp_types::state::Message {
    serde_json::from_value(serde_json::json!({
        "text": "run it",
        "origin": { "kind": "user" },
    }))
    .expect("a user message parses")
}

/// One wire-shaped event on `channel`, exactly as the pump receives it.
fn envelope(channel: String, action: StateAction) -> ahp::ClientEvent {
    ahp::ClientEvent {
        channel: channel.clone(),
        event: ahp::SubscriptionEvent::Action(ActionEnvelope {
            channel,
            action,
            server_seq: 1,
            origin: None,
            rejection_reason: None,
        }),
    }
}

fn turn_started(turn: &str) -> StateAction {
    StateAction::ChatTurnStarted(ChatTurnStartedAction {
        turn_id: turn.to_string(),
        started_at: "2026-10-14T00:00:00Z".to_string(),
        message: user_message(),
        queued_message_id: None,
        meta: None,
    })
}

fn turn_complete(turn: &str) -> StateAction {
    StateAction::ChatTurnComplete(ChatTurnCompleteAction {
        turn_id: turn.to_string(),
        duration: 0,
        meta: None,
    })
}

/// Seed a session whose default chat is `cid`, so a chat-channel envelope
/// resolves its owning session the way a host snapshot seats it.
fn seed_session(store: &gpui::Entity<AhpStore>, sid: &str, cid: &str, cx: &mut TestAppContext) {
    store.update(cx, |s, _| {
        let state = serde_json::from_value(serde_json::json!({
            "resource": session_uri(sid),
            "provider": "test",
            "title": sid,
            "status": 1,
            "modifiedAt": "2026-10-14T00:00:00.000Z",
            "lifecycle": "ready",
            "activeClients": [],
            "chats": [],
            "defaultChat": chat_uri(cid),
        }))
        .expect("the session snapshot parses");
        s.book.apply_snapshot(&session_uri(sid), state);
    });
}

fn transcript_items(workspace: &gpui::Entity<agent_ui::Workspace>, cx: &TestAppContext) -> usize {
    workspace.read_with(cx, |ws, cx| ws.diagnostic_list_state(cx).item_count())
}

#[gpui::test]
async fn a_parked_sessions_stream_never_paints_the_bound_transcript(cx: &mut TestAppContext) {
    init_harness(cx);
    let (_window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(_window.into(), cx);

    let store = cx.update(|cx| cx.new(|_| AhpStore::detached()));
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store.clone(), "s-1", cx);
    });
    seed_session(&store, "s-1", "c-1", &mut visual.cx);
    seed_session(&store, "s-2", "c-2", &mut visual.cx);

    // The parked session (s-2) streams a turn while s-1 is the bound
    // foreground. The turn must land in ITS chat's fold, and the bound
    // transcript must not move.
    store.update(&mut visual.cx, |s, cx| {
        s.diagnostic_route_event(envelope(chat_uri("c-2"), turn_started("turn-2")), cx);
    });
    visual.cx.run_until_parked();
    assert_eq!(
        transcript_items(&workspace, &visual.cx),
        0,
        "a parked session's turn must not paint the bound transcript"
    );
    assert!(
        store.update(&mut visual.cx, |s, _| {
            s.book
                .chats
                .get("c-2")
                .map(|chat| chat.turns.len() + usize::from(chat.active_turn.is_some()) > 0)
                .unwrap_or(false)
        }),
        "the parked turn routes to its own session's fold"
    );
    assert!(
        store.update(&mut visual.cx, |s, _| s.book.chats.get("c-1").is_none()),
        "the bound session's fold holds nothing of the parked stream"
    );

    // The bound session's own first displayable fold builds the transcript
    // (the snapshot → transcript transition on the store notify).
    store.update(&mut visual.cx, |s, cx| {
        s.diagnostic_route_event(envelope(chat_uri("c-1"), turn_started("turn-1")), cx);
    });
    visual.cx.run_until_parked();
    let items = transcript_items(&workspace, &visual.cx);
    assert!(items > 0, "the bound session's content lands");

    // The live leg: the bound session's settlement advances its own fold
    // onto the transcript; the parked session's stays in its own fold.
    store.update(&mut visual.cx, |s, cx| {
        s.diagnostic_route_event(envelope(chat_uri("c-1"), turn_complete("turn-1")), cx);
        s.diagnostic_route_event(envelope(chat_uri("c-2"), turn_complete("turn-2")), cx);
    });
    visual.cx.run_until_parked();
    assert_eq!(
        transcript_items(&workspace, &visual.cx),
        items,
        "a parked settlement must not add transcript rows"
    );
    assert!(
        store.update(&mut visual.cx, |s, _| {
            s.book
                .chats
                .get("c-1")
                .map(|chat| {
                    chat.turns
                        .first()
                        .is_some_and(|t| t.state == ahp_types::state::TurnState::Complete)
                })
                .unwrap_or(false)
        }),
        "the bound session's fold carries its own settlement"
    );
}
