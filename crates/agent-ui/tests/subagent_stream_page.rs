//! The Captain status block's sub-agent lines and the main-column page behind
//! them: the block lists the parent chat's live runs, and a line's click opens
//! that child's own transcript in the main column (the child chat is the only
//! carrier of a sub-agent's text — the parent's chat carries the run's status).
#![cfg(feature = "test-support")]

mod common;

use common::{init_harness, open_workspace};
use gpui::{AppContext as _, TestAppContext, VisualTestContext};
use steer_agent_chat_ui::ahp_store::{AhpStore, chat_uri};

/// The parent chat's fold with one running sub-agent (the host's background
/// work), plus the child chat carrying one finished turn.
fn seed(store: &gpui::Entity<AhpStore>, parent: &str, child: &str, cx: &mut VisualTestContext) {
    store.update(&mut cx.cx, |s, _| {
        let parent_chat: ahp_types::state::ChatState = serde_json::from_value(serde_json::json!({
            "resource": chat_uri(parent),
            "title": "the captain's chat",
            "status": 0,
            "modifiedAt": "2026-10-15T00:00:00.000Z",
            "turns": [],
            "backgroundWork": [{
                "kind": "subagent",
                "id": child,
                "label": "Explore",
                "startedAt": "2026-10-15T00:00:00.000Z",
                "_meta": { "agentType": "Explore", "latestActivity": "fix the login redirect" },
                "chat": format!("ahp-chat:/{child}"),
            }],
        }))
        .expect("the parent chat snapshot parses");
        let child_chat: ahp_types::state::ChatState = serde_json::from_value(serde_json::json!({
            "resource": chat_uri(child),
            "title": "the child's chat",
            "status": 0,
            "modifiedAt": "2026-10-15T00:00:01.000Z",
            "turns": [{
                "id": "turn-1",
                "message": { "text": "fix the login redirect", "origin": { "kind": "user" } },
                "responseParts": [
                    { "kind": "markdown", "id": "turn-1-p1", "content": "the child's answer" },
                ],
                "state": "complete",
            }],
        }))
        .expect("the child chat snapshot parses");
        s.book.chats.insert(parent.to_string(), parent_chat);
        s.book.chats.insert(child.to_string(), child_chat);
    });
}

/// The block's lines come from the parent chat's live background work, and a
/// line's click puts THAT child's transcript in the main column (never the
/// parent's rows); the page's back control returns the conversation.
#[gpui::test]
async fn a_subagent_line_opens_that_childs_transcript(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let store = cx.update(|cx| cx.new(|_| AhpStore::detached()));
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store.clone(), "parent-1", cx);
    });
    seed(&store, "parent-1", "sub-ns-3", &mut visual);

    let rows = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_running_subagents(cx));
    assert_eq!(rows.len(), 1, "the strip lists the live run");
    assert_eq!(rows[0].id, "sub-ns-3");
    assert_eq!(rows[0].label, "Explore");
    assert_eq!(rows[0].activity.as_deref(), Some("fix the login redirect"));

    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_open_subagent_stream("sub-ns-3", cx);
    });
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_subagent_stream(cx)),
        Some("sub-ns-3".to_string()),
        "the clicked run owns the main column"
    );
    let texts = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_subagent_stream_texts(cx));
    assert!(
        texts.iter().any(|text| text.contains("the child's answer")),
        "the page renders the CHILD chat's transcript, got {texts:?}"
    );
    // The page paints as the main column's card (state alone would not).
    for _ in 0..3 {
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }
    assert!(
        visual.debug_bounds("subagent-stream-main").is_some(),
        "the sub-agent page paints in the main column"
    );

    // The run advances: the parent's background work reports new activity and
    // the child's chat grew. The store notify is the production trigger — the
    // page re-subscribes and rebuilds from the larger fold, so a running
    // child's page follows it instead of freezing at the click.
    store.update(&mut visual.cx, |s, cx| {
        let child: ahp_types::state::ChatState = serde_json::from_value(serde_json::json!({
            "resource": chat_uri("sub-ns-3"),
            "title": "the child's chat",
            "status": 0,
            "modifiedAt": "2026-10-15T00:00:02.000Z",
            "turns": [
                {
                    "id": "turn-1",
                    "message": { "text": "fix the login redirect", "origin": { "kind": "user" } },
                    "responseParts": [
                        { "kind": "markdown", "id": "turn-1-p1", "content": "the child's answer" },
                    ],
                    "state": "complete",
                },
                {
                    "id": "turn-2",
                    "message": { "text": "and the tests", "origin": { "kind": "user" } },
                    "responseParts": [
                        { "kind": "markdown", "id": "turn-2-p1", "content": "the child's follow-up" },
                    ],
                    "state": "complete",
                },
            ],
        }))
        .expect("the grown child chat snapshot parses");
        s.book.chats.insert("sub-ns-3".to_string(), child);
        let parent: ahp_types::state::ChatState = serde_json::from_value(serde_json::json!({
            "resource": chat_uri("parent-1"),
            "title": "the captain's chat",
            "status": 0,
            "modifiedAt": "2026-10-15T00:00:02.000Z",
            "turns": [],
            "backgroundWork": [{
                "kind": "subagent",
                "id": "sub-ns-3",
                "label": "Explore",
                "startedAt": "2026-10-15T00:00:00.000Z",
                "_meta": { "agentType": "Explore", "latestActivity": "running the tests" },
                "chat": "ahp-chat:/sub-ns-3",
            }],
        }))
        .expect("the ticking parent chat snapshot parses");
        s.book.chats.insert("parent-1".to_string(), parent);
        cx.notify();
    });
    cx.run_until_parked();
    let texts = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_subagent_stream_texts(cx));
    assert!(
        texts
            .iter()
            .any(|text| text.contains("the child's follow-up")),
        "the page follows the run's own ticks, got {texts:?}"
    );

    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_subagent_stream_back(cx);
    });
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_subagent_stream(cx)),
        None,
        "the back control returns the conversation"
    );
    agent_ui::dispatch::clear_globals_for_test();
}
