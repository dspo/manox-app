//! Regression (inverted): a conversation built from the pre-snapshot replay
//! fold (the first journal deltas beat the subscribe answer) used to freeze
//! at that partial shape — the one-shot rebuild guard could never fire again,
//! so the transcript stayed truncated. Under the keyed projection there is
//! no rebuild at all: the snapshot's turns carry the same part ids the
//! replay minted, so the sync that follows the snapshot converges the same
//! rows to the full shape — and a later sync changes nothing.
//!
//! Own test binary: the shared harness initializes process-global singletons
//! (see `tests/common/mod.rs`).
#![cfg(feature = "test-support")]

mod common;

use ahp_types::actions::{ChatTurnCompleteAction, ChatTurnStartedAction, StateAction};
use common::{init_harness, open_workspace};
use gpui::{AppContext as _, TestAppContext, VisualTestContext};
use steer_agent_chat_ui::ahp_store::{AhpStore, chat_uri};

fn user_message() -> ahp_types::state::Message {
    serde_json::from_value(serde_json::json!({
        "text": "run it",
        "origin": { "kind": "user" },
    }))
    .expect("a user message parses")
}

#[gpui::test]
async fn a_pre_snapshot_build_converges_when_the_snapshot_lands(cx: &mut TestAppContext) {
    init_harness(cx);
    let (_window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(_window.into(), cx);

    let store = cx.update(|cx| cx.new(|_| AhpStore::detached()));
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store.clone(), "sess-1", cx);
    });

    // The race, staged: the first replay delta (the stale turn's turnStarted)
    // folds BEFORE the subscribe snapshot answers. The fold holds a
    // displayable chat (an active turn, zero settled turns).
    let delta = |turn: &str, part: &str, text: &str| {
        StateAction::ChatDelta(ahp_types::actions::ChatDeltaAction {
            turn_id: turn.to_string(),
            part_id: part.to_string(),
            content: text.to_string(),
            meta: None,
        })
    };
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
        // The replay's first delta opens its own part in place.
        s.book.apply(
            &chat_uri("sess-1"),
            &StateAction::ChatResponsePart(ahp_types::actions::ChatResponsePartAction {
                turn_id: "turn-1".to_string(),
                part: ahp_types::state::ResponsePart::Markdown(
                    ahp_types::state::MarkdownResponsePart {
                        id: "p-11".to_string(),
                        content: String::new(),
                    },
                ),
                meta: None,
            }),
        );
        s.book.apply(
            &chat_uri("sess-1"),
            &delta("turn-1", "p-11", "first answer"),
        );
    });
    workspace.update(&mut visual.cx, |ws, cx| ws.diagnostic_sync_conversation(cx));
    let early_texts = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_transcript_texts(cx));
    assert_eq!(early_texts.len(), 1, "the active turn lowered to a row");

    // The snapshot lands with the full replayed history: the same sync —
    // not a rebuild — converges the transcript to the snapshot's shape.
    store.update(&mut visual.cx, |s, _| {
        s.book.apply(
            &chat_uri("sess-1"),
            &StateAction::ChatTurnComplete(ChatTurnCompleteAction {
                turn_id: "turn-1".to_string(),
                duration: 0,
                meta: None,
            }),
        );
        s.book.apply(
            &chat_uri("sess-1"),
            &StateAction::ChatTurnStarted(ChatTurnStartedAction {
                turn_id: "turn-2".to_string(),
                started_at: "2026-09-30T00:01:00Z".to_string(),
                message: user_message(),
                queued_message_id: None,
                meta: None,
            }),
        );
        s.book.apply(
            &chat_uri("sess-1"),
            &StateAction::ChatResponsePart(ahp_types::actions::ChatResponsePartAction {
                turn_id: "turn-2".to_string(),
                part: ahp_types::state::ResponsePart::Markdown(
                    ahp_types::state::MarkdownResponsePart {
                        id: "p-21".to_string(),
                        content: String::new(),
                    },
                ),
                meta: None,
            }),
        );
        s.book.apply(
            &chat_uri("sess-1"),
            &delta("turn-2", "p-21", "second answer"),
        );
        s.book.apply(
            &chat_uri("sess-1"),
            &StateAction::ChatTurnComplete(ChatTurnCompleteAction {
                turn_id: "turn-2".to_string(),
                duration: 0,
                meta: None,
            }),
        );
        assert!(
            s.diagnostic_apply_folded_chat_snapshot("sess-1"),
            "the folded chat re-applies as its snapshot"
        );
    });
    workspace.update(&mut visual.cx, |ws, cx| ws.diagnostic_sync_conversation(cx));
    let settled_texts =
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_transcript_texts(cx));
    assert!(
        settled_texts.len() > early_texts.len(),
        "the snapshot's settled turns land on top of the pre-snapshot rows: {settled_texts:?}"
    );
    assert!(
        settled_texts.iter().any(|t| t.contains("first answer")),
        "the pre-snapshot row survived the convergence (same key, updated in place)"
    );

    // Settled: a further sync churns nothing — the projection is idempotent
    // over an unchanged fold.
    workspace.update(&mut visual.cx, |ws, cx| ws.diagnostic_sync_conversation(cx));
    let again = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_transcript_texts(cx));
    assert_eq!(
        again, settled_texts,
        "an unchanged fold re-projects identically"
    );
    manox_agent::thread_store::drop_global_for_test();
}
