//! Regression: switching back to a thread that streamed while parked must
//! not replay its retained display events onto the rebuilt transcript.
//!
//! The bucket is a delta log against the conversation's build state. A
//! rebuild synthesizes the conversation from the book — which already folded
//! every event the bucket holds — so the log must be consumed AT the rebuild.
//! Without that, the next notify takes the live leg (the conversation is no
//! longer empty, so the guard does not rebuild again) and replays the whole
//! retained bucket onto the rebuilt list: every parked delta renders twice.
//!
//! Own test binary: the shared harness initializes process-global singletons
//! (see `tests/common/mod.rs` — one `#[gpui::test]` per binary).
#![cfg(feature = "test-support")]

mod common;

use ahp_types::actions::{
    ActionEnvelope, ChatDeltaAction, ChatResponsePartAction, ChatTurnStartedAction, StateAction,
};
use ahp_types::state::{MarkdownResponsePart, ResponsePart};
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

/// Open the answer part `part`, then append `text` to it — the two actions
/// that give the parked turn a markdown body the transcript lowers.
fn open_part(turn: &str, part: &str) -> StateAction {
    StateAction::ChatResponsePart(ChatResponsePartAction {
        turn_id: turn.to_string(),
        part: ResponsePart::Markdown(MarkdownResponsePart {
            id: part.to_string(),
            content: String::new(),
        }),
        meta: None,
    })
}

fn append_text(turn: &str, part: &str, text: &str) -> StateAction {
    StateAction::ChatDelta(ChatDeltaAction {
        turn_id: turn.to_string(),
        part_id: part.to_string(),
        content: text.to_string(),
        meta: None,
    })
}

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

fn transcript_texts(
    workspace: &gpui::Entity<agent_ui::Workspace>,
    cx: &TestAppContext,
) -> Vec<String> {
    workspace.read_with(cx, |ws, cx| ws.diagnostic_transcript_texts(cx))
}

#[gpui::test]
async fn switching_back_does_not_replay_the_parked_bucket(cx: &mut TestAppContext) {
    init_harness(cx);
    let (_window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(_window.into(), cx);

    let store = cx.update(|cx| cx.new(|_| AhpStore::detached()));
    seed_session(&store, "s-1", "c-1", &mut visual.cx);
    seed_session(&store, "s-2", "c-2", &mut visual.cx);

    // s-2 is foreground; s-1 is PARKED and keeps streaming (its channel stays
    // subscribed). Every derived event lands in s-1's bucket, unconsumed.
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store.clone(), "s-2", cx);
    });
    store.update(&mut visual.cx, |s, cx| {
        s.diagnostic_route_event(envelope(chat_uri("c-1"), turn_started("turn-1")), cx);
        s.diagnostic_route_event(envelope(chat_uri("c-1"), open_part("turn-1", "p1")), cx);
        s.diagnostic_route_event(
            envelope(chat_uri("c-1"), append_text("turn-1", "p1", "parked text")),
            cx,
        );
    });
    visual.cx.run_until_parked();
    assert!(
        transcript_texts(&workspace, &visual.cx).is_empty(),
        "a parked thread's stream paints nothing of the bound transcript"
    );

    // The user switches BACK to s-1: the attach rebuilds the conversation
    // from the book — which already holds "parked text".
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store.clone(), "s-1", cx);
    });
    // A store notify drives the observer's first pass (the same notify the
    // switch-back's stream activity would produce).
    store.update(&mut visual.cx, |_, cx| cx.notify());
    visual.cx.run_until_parked();
    let after_rebuild = transcript_texts(&workspace, &visual.cx);
    assert!(
        after_rebuild.iter().any(|t| t.contains("parked text")),
        "the switch-back rebuilds the transcript from the parked content: {after_rebuild:?}"
    );

    // Any later event (here: the parked turn settling) drives the live leg.
    // The threshold: the rebuilt transcript must not grow by a replay of the
    // already-folded deltas.
    store.update(&mut visual.cx, |_, cx| cx.notify());
    visual.cx.run_until_parked();
    let after_live = transcript_texts(&workspace, &visual.cx);
    assert_eq!(
        after_live, after_rebuild,
        "the retained bucket must be consumed at the rebuild, not replayed after it"
    );
}
