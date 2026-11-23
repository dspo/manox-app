//! The pi session sheet's windowed regression tests: a seeded transcript with
//! thinking, a plain reply, and a tool call must draw its cards through the
//! production [`PiSessionView`] without a harness process, and a session the
//! store no longer knows must show the closed face instead of an endless
//! loading state.
//!
//! The transcript is seeded through the store's test-support fold seam; the
//! window draws offscreen (AGENTS.md「出 UI 图」). Render runs while the view
//! entity holds its gpui lease, so a render that reaches back into its own
//! entity aborts before any bounds could be read — the first pass here is the
//! meaningful one.

use gpui::{AppContext as _, TestAppContext, VisualTestContext, px, size};
use serde_json::json;
use steer_acp::SessionInfo;
use steer_agent_chat_ui::pi::{AcpStore, StoreEvent};
use steer_agent_chat_ui::views::pi::PiSessionView;

const SESSION: &str = "22222222-2222-4222-8222-222222222222";

fn init_theme(cx: &mut TestAppContext) {
    cx.update(|cx| {
        if !cx.has_global::<gpui_component::Theme>() {
            gpui_component::init(cx);
        }
    });
}

fn ready_event() -> StoreEvent {
    StoreEvent::Ready {
        session_id: SESSION.to_owned(),
        info: SessionInfo {
            session_id: SESSION.to_owned(),
            config_options: json!([]),
            modes: json!({}),
            raw: json!({}),
        },
    }
}

fn update(value: serde_json::Value) -> StoreEvent {
    StoreEvent::Update {
        session_id: SESSION.to_owned(),
        update: value,
    }
}

/// A turn whose reply carries thinking, plain text, and one tool call.
fn seeded_turn() -> Vec<StoreEvent> {
    vec![
        update(json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "hello"},
            "messageId": "e0",
            "_meta": {"pi": {"entryId": "e0"}},
        })),
        update(json!({
            "sessionUpdate": "agent_thought_chunk",
            "content": {"type": "text", "text": "weighing it"},
            "messageId": "m1",
        })),
        update(json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "hi there"},
            "messageId": "m1",
        })),
        update(json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": ""},
            "messageId": "e1",
            "_meta": {"pi": {"settled": true, "entryId": "e1"}},
        })),
        update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "t1",
            "name": "bash",
            "title": "bash ls",
            "status": "in_progress",
        })),
        update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "t1",
            "status": "completed",
        })),
    ]
}

fn draw(visual: &mut VisualTestContext) {
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
}

#[gpui::test]
async fn thoughts_and_tool_calls_render_without_a_double_lease(cx: &mut TestAppContext) {
    init_theme(cx);
    let store = cx.update(|cx| cx.new(|cx| AcpStore::new(None, cx)));
    cx.update(|cx| {
        store.update(cx, |store, cx| {
            store.open_for_test(SESSION, std::path::Path::new("/tmp"));
            store.fold_event_for_test(&ready_event(), cx);
            for event in seeded_turn() {
                store.fold_event_for_test(&event, cx);
            }
        });
    });

    let window = cx.open_window(size(px(760.), px(720.)), |window, cx| {
        PiSessionView::new(store.clone(), SESSION, window, cx)
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    draw(&mut visual);

    let reasoning = visual
        .debug_bounds("pi-reasoning:e1-trigger")
        .expect("the thinking card paints");
    assert!(reasoning.size.height > px(0.), "the trigger row has height");
    let tool = visual
        .debug_bounds("pi-tool:t1-header")
        .expect("the tool card paints");
    assert!(tool.size.height > px(0.), "the card header has height");
}

#[gpui::test]
async fn a_session_without_a_store_view_shows_the_closed_face(cx: &mut TestAppContext) {
    init_theme(cx);
    let store = cx.update(|cx| cx.new(|cx| AcpStore::new(None, cx)));

    let window = cx.open_window(size(px(760.), px(720.)), |window, cx| {
        PiSessionView::new(store.clone(), SESSION, window, cx)
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    draw(&mut visual);

    assert!(
        visual.debug_bounds("pi-closed").is_some(),
        "the closed face paints instead of an endless loading state"
    );
}
