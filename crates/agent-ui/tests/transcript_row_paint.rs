//! Regression: the transcript painted NOTHING on every conversation — the
//! turn-rail band (#92) wraps the message list in `h_flex`, whose preset
//! `items_center` centered the list vertically and collapsed its `h_full` to
//! zero (the band gave it height, the centering took it away). Rows were
//! built and counted but never painted; the composer and every other surface
//! were fine. The band now stretches its children, and the list wrapper must
//! always carry real height through the production render path.
//!
//! Own test binary: shared harness singletons (see `tests/common/mod.rs`).
//! Note `debug_bounds` cannot see INTO `gpui::list` virtualized rows (the
//! scroll_list probes record via `on_prepaint` for the same reason), so the
//! assertion targets the list wrapper itself.
#![cfg(feature = "test-support")]

mod common;

use common::{init_harness, open_workspace};
use gpui::{AppContext as _, TestAppContext, VisualTestContext, px};
use manox_agent_chat_ui::ahp_store::{AhpStore, chat_uri};

fn rich_chat_snapshot() -> serde_json::Value {
    let turn = |id: &str, q: &str, a: &str| {
        serde_json::json!({
            "id": id,
            "message": { "text": q, "origin": { "kind": "user" } },
            "responseParts": [
                { "kind": "markdown", "id": format!("{id}-p1"), "content": a },
            ],
            "state": "complete",
        })
    };
    serde_json::json!({
        "resource": chat_uri("sess-1"),
        "title": "paint probe",
        "status": 0,
        "modifiedAt": "2026-09-30T00:00:00Z",
        "turns": [
            turn("t-1", "first question", "first answer with a fairly long body so the row needs real height."),
            turn("t-2", "second question", "second answer, likewise long enough to measure."),
            turn("t-3", "third question", "third answer."),
        ],
        "activeTurn": {
            "id": "t-4",
            "startedAt": "2026-09-30T00:05:00Z",
            "message": { "text": "running question", "origin": { "kind": "user" } },
            "responseParts": [
                { "kind": "markdown", "id": "t-4-p1", "content": "partial answer streaming" },
            ],
        },
    })
}

#[gpui::test]
async fn the_transcript_band_gives_the_message_list_real_height(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let store = cx.update(|cx| cx.new(|_| AhpStore::detached()));
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store.clone(), "sess-1", cx);
    });

    // The authoritative snapshot lands (three settled turns + one active),
    // then the rebuild guard builds the conversation.
    store.update(&mut visual.cx, |s, _| {
        let chat: ahp_types::state::ChatState =
            serde_json::from_value(rich_chat_snapshot()).expect("snapshot parses");
        assert!(s.book.apply_snapshot(
            &chat_uri("sess-1"),
            ahp_types::state::SnapshotState::Chat(Box::new(chat)),
        ));
    });
    let (rebuilt, _) =
        workspace.update(&mut visual.cx, |ws, cx| ws.diagnostic_run_rebuild_guard(cx));
    assert!(rebuilt, "the snapshot build fires the rebuild guard");
    let (_, built_turns, items) =
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_rebuild_watermark(cx));
    assert_eq!(built_turns, 3, "three settled turns built");
    assert!(
        items >= 3,
        "the transcript holds the built rows, got {items}"
    );

    // Paint three frames (the draw-probe warmup the geometry tests use).
    for _ in 0..3 {
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }
    let bounds = visual.debug_bounds("workspace-message-list");
    let bounds = bounds.expect("the message list wrapper must paint");
    assert!(
        bounds.size.height > px(100.),
        "the transcript band collapsed the list height again: {:?}",
        bounds.size
    );
    assert!(
        bounds.size.width > px(100.),
        "the message list lost its width: {:?}",
        bounds.size
    );
    manox_agent::thread_store::drop_global_for_test();
}
