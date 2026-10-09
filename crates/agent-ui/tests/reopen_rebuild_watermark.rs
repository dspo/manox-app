//! Regression: a conversation rebuilt from the pre-snapshot replay fold (the
//! first journal deltas beat the subscribe answer) froze at that partial
//! shape — the one-shot `is_empty` rebuild guard could never fire again, so
//! the transcript stayed truncated (empty but for the stale active turn) and
//! the composer stuck in the stop/running form. The rebuild watermark heals
//! the build once the authoritative snapshot lands with more settled turns.
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
async fn pre_snapshot_rebuild_heals_when_the_snapshot_lands(cx: &mut TestAppContext) {
    init_harness(cx);
    let (_window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(_window.into(), cx);

    let store = cx.update(|cx| cx.new(|_| AhpStore::detached()));
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store.clone(), "sess-1", cx);
    });

    // The race, staged: the first replay delta (the stale turn's turnStarted)
    // folds BEFORE the subscribe snapshot answers. The fold now holds a
    // displayable chat (an active turn, zero settled turns) — exactly the
    // shape that fired the old one-shot guard into a premature build.
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
    });
    let (rebuilt, _) =
        workspace.update(&mut visual.cx, |ws, cx| ws.diagnostic_run_rebuild_guard(cx));
    assert!(
        rebuilt,
        "the first displayable fold builds the conversation"
    );
    let (pre_snapshot, built_turns, items) =
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_rebuild_watermark(cx));
    assert!(pre_snapshot, "the build ran before the snapshot landed");
    assert_eq!(built_turns, 0, "no settled turns existed at build time");
    assert!(items > 0, "the active turn lowers to transcript rows");

    // The snapshot lands with the full replayed history: the watermark must
    // heal the premature build instead of starving on the non-empty skeleton.
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
    let (rebuilt, _) =
        workspace.update(&mut visual.cx, |ws, cx| ws.diagnostic_run_rebuild_guard(cx));
    assert!(
        rebuilt,
        "the snapshot's larger settled-turn count heals the premature build"
    );
    let (pre_snapshot, built_turns, items_after) =
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_rebuild_watermark(cx));
    assert!(
        !pre_snapshot,
        "the healing build ran on the landed snapshot"
    );
    assert_eq!(built_turns, 2, "the watermark advanced to the snapshot");
    assert!(
        items_after > items,
        "the healed transcript holds more than the stale active turn"
    );

    // Settled: no further rebuild churns the conversation once the watermark
    // matches the fold.
    let (rebuilt, _) =
        workspace.update(&mut visual.cx, |ws, cx| ws.diagnostic_run_rebuild_guard(cx));
    assert!(!rebuilt, "a watermark-matched fold does not rebuild again");
    manox_agent::thread_store::drop_global_for_test();
}
