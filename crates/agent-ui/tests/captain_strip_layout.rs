//! Regression: a narrow card must not lose the Captain strip's clock or its
//! activity summary. The lead-in's chrome (prefix, separators, clock) is
//! fixed at shrink 0, so the model name and the activity are the only
//! elements that may yield. If the model is fixed too, a long model id
//! overflows the row and `overflow_hidden` clips the clock and the activity
//! instead of ellipsizing them — a container-level `truncate()` cannot
//! ellipsize a flex child that never shrinks.
//!
//! Own test binary: the shared harness singletons are app-affine, so one
//! `#[gpui::test]` per process.

#![cfg(feature = "test-support")]

mod common;

use common::{init_harness, open_workspace_at};
use gpui::{AppContext as _, TestAppContext, VisualTestContext, px};
use steer_agent_chat_ui::ahp_store::{AhpStore, chat_uri};

const SESSION: &str = "sess-captain-strip";
/// Long but realistic: no registry entry, so the tag renders the raw wire id —
/// the widest model segment the strip can see.
const MODEL: &str = "openrouter/anthropic/claude-sonnet-4-5";

fn running_chat() -> ahp_types::state::ChatState {
    serde_json::from_value(serde_json::json!({
        "resource": chat_uri(SESSION),
        "title": "captain strip layout",
        "status": 0,
        "modifiedAt": "2026-10-23T00:00:00.000Z",
        "turns": [],
        "activeTurn": {
            "id": "turn-1",
            "startedAt": "2026-10-23T00:00:00.000Z",
            "message": { "text": "repaint the strip", "origin": { "kind": "user" } },
            "responseParts": [
                { "kind": "markdown", "id": "turn-1-p0", "content": "working on it" }
            ],
        },
    }))
    .expect("the running chat snapshot parses")
}

fn model_pick() -> serde_json::Map<String, serde_json::Value> {
    let mut pick = serde_json::Map::new();
    pick.insert("model".to_string(), serde_json::json!(MODEL));
    pick
}

#[gpui::test]
async fn a_narrow_card_keeps_the_strips_clock_and_activity(cx: &mut TestAppContext) {
    init_harness(cx);
    // The real narrow shape: the default 1100px window with both side panes
    // open leaves ~360px of card (the card-width note in render.rs), and
    // `centered` spends 16px of padding on each side.
    let (window, workspace) = open_workspace_at(cx, 360.0);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let store = cx.update(|cx| cx.new(|_| AhpStore::detached()));
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store.clone(), SESSION, cx);
    });
    store.update(&mut visual.cx, |s, _| {
        s.optimistic_config(SESSION, &model_pick());
        s.book.chats.insert(SESSION.into(), running_chat());
    });
    workspace.update(&mut visual.cx, |ws, cx| ws.diagnostic_sync_conversation(cx));

    // Flush the store-observe callbacks now, then let a whole second of local
    // turn time pass: the clock renders only after it ticks.
    visual.run_until_parked();
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    for _ in 0..3 {
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual.run_until_parked();
    }

    let strip = visual
        .debug_bounds("workspace-captain-strip")
        .expect("the captain row paints");
    let clock = visual
        .debug_bounds("workspace-captain-clock")
        .expect("the elapsed clock paints");
    let activity = visual
        .debug_bounds("workspace-captain-activity")
        .expect("the activity summary paints");
    let right = strip.origin.x + strip.size.width;
    assert!(
        clock.origin.x + clock.size.width <= right + px(0.5),
        "the clock must stay inside the row: clock={clock:?}, row={strip:?}"
    );
    assert!(
        activity.origin.x + activity.size.width <= right + px(0.5),
        "the activity must stay inside the row: activity={activity:?}, row={strip:?}"
    );
    assert!(
        activity.size.width > px(8.),
        "the activity must keep real width, not be clipped to nothing: {activity:?}"
    );
    assert!(
        strip.size.height < px(32.),
        "the row must stay on one line: {strip:?}"
    );
}
