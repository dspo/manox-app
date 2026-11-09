//! The ask card's one-card invariant, pinned through the REAL production
//! chain: a frozen journal literal (the runtime's non-blocking ask shape) →
//! the upstream Translator → the AhpStore fold → the workspace edges → the
//! render routing the message column reads.
//!
//! The runtime's ask tool journals the AskUserQuestion TOOL CALL and the
//! elicitation under the SAME `call_…` id, and the tool call completes at
//! once ("question delivered") in the same second the elicitation parks.
//! Projecting both faces as their own top-level row minted TWO same-id ask
//! cards; `find_tool` landed on the tool-call row — a Running (then Success)
//! one, while the interactive form renders on PendingApproval alone — so the
//! form never rendered and a dead plain card sat beside it. The projection
//! now merges the faces into one card (the transcript's `ask_call_ids`
//! pre-scan); this pins the routing end to end so the seam cannot rot
//! silently again.
#![cfg(feature = "test-support")]

mod common;

use common::{init_harness, open_workspace};
use gpui::{AppContext as _, TestAppContext, VisualTestContext};
use manox_ahp_runtime::ahp::projection::Translator;
use manox_harness::session::SessionTreeEntry;
use steer_agent_chat_ui::ahp_store::{AhpStore, chat_uri};

const ASK_ID: &str = "call_probe";

/// The journal shape one real ask produces (frozen from the 2026-11
/// presentation-studio session): turn start → the model's ask call (running,
/// full questions input) → the gate (the elicitation, SAME id) → the call's
/// instant success + prose result. The decision lands much later.
fn journal() -> Vec<serde_json::Value> {
    let questions = serde_json::json!([
        {
            "header": "编辑器", "id": "editor", "multiSelect": false,
            "question": "编辑器面板用什么方案？",
            "options": [
                {"label": "Monaco", "recommended": true, "description": "内嵌 web 编辑器"},
                {"label": "CodeMirror 6"},
            ],
        },
        {
            "header": "范围", "id": "scope", "multiSelect": false,
            "question": "第一步先做到哪个范围？",
            "options": [
                {"label": "M1", "recommended": true},
                {"label": "M1+M2"},
            ],
        },
    ]);
    let base = |id: &str, extra: serde_json::Value| {
        let mut object = serde_json::json!({
            "id": id,
            "parentId": serde_json::Value::Null,
            "timestamp": "2026-11-02T09:54:35.000Z",
        });
        if let (Some(base), Some(extra)) = (object.as_object_mut(), extra.as_object()) {
            for (k, v) in extra {
                base.insert(k.clone(), v.clone());
            }
        }
        object
    };
    vec![
        base("e-1", serde_json::json!({"type": "turn_start"})),
        base(
            "e-2",
            serde_json::json!({
                "type": "tool_call", "callId": ASK_ID,
                "name": "AskUserQuestion", "title": "AskUserQuestion",
                "status": "running", "input": {"questions": questions},
            }),
        ),
        base(
            "e-3",
            serde_json::json!({
                "type": "tool_call", "callId": ASK_ID,
                "name": "AskUserQuestion", "title": "Clarifying question",
                "status": "pending-approval", "input": {"questions": questions},
            }),
        ),
        base(
            "e-4",
            serde_json::json!({
                "type": "question", "kind": "request", "authId": ASK_ID,
                "payload": {
                    "toolName": "AskUserQuestion",
                    "summary": "Clarifying question",
                    "input": {"questions": questions},
                },
            }),
        ),
        base(
            "e-5",
            serde_json::json!({
                "type": "tool_call", "callId": ASK_ID,
                "name": "AskUserQuestion", "title": "AskUserQuestion",
                "status": "success", "input": serde_json::Value::Null,
            }),
        ),
        base(
            "e-6",
            serde_json::json!({
                "type": "tool_result", "callId": ASK_ID,
                "output": "Question delivered to the user. This turn ends here — the card is the conversation's last item.",
                "isError": false,
            }),
        ),
    ]
}

macro_rules! drive {
    ($workspace:expr, $cx:expr) => {
        $workspace.update(&mut $cx, |ws, cx| {
            ws.diagnostic_sync_conversation(cx);
            ws.diagnostic_sync_live_ask(cx);
            ws.diagnostic_reconcile_pending_with_projections(cx);
            ws.diagnostic_sync_ask_card_snapshots(cx);
        });
    };
}

#[gpui::test]
async fn one_ask_id_is_one_renderable_card(cx: &mut TestAppContext) {
    init_harness(cx);
    let (_window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(_window.into(), cx);

    let store = cx.update(|cx| cx.new(|_| AhpStore::detached()));
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store.clone(), "sess-1", cx);
    });

    // The frozen journal replays the production way — the translator turns
    // the entries into the fold actions.
    let mut translator = Translator::new();
    for (seq, entry) in journal().into_iter().enumerate() {
        let entry: SessionTreeEntry = serde_json::from_value(entry).expect("entry parses");
        for emitted in translator.on_entry("c-1", "sess-1", seq as u64, &entry) {
            store.update(&mut visual.cx, |s, _| {
                s.book.apply(&chat_uri("sess-1"), &emitted.action)
            });
        }
    }
    drive!(workspace, visual.cx);

    workspace.read_with(&visual.cx, |ws, cx| {
        assert_eq!(
            ws.diagnostic_pending_ask_id(cx).as_deref(),
            Some(ASK_ID),
            "the elicitation arms the pending ask"
        );
        assert_eq!(
            ws.diagnostic_tool_call_count(ASK_ID, cx),
            1,
            "ONE ask id is ONE top-level card — never a second same-id row"
        );
        // The render routing the message column reads: the form renders only
        // on the PendingApproval row that carries the snapshot.
        assert_eq!(
            ws.diagnostic_find_tool_row(ASK_ID, cx),
            Some((manox_agent::ToolCallStatus::PendingApproval, true)),
            "the interactive form renders on the merged card"
        );
    });

    // The user answers: the card settles — Success, one row still, and the
    // output is the canonical answers JSON the settled phase's markdown
    // folds.
    let completed: ahp_types::actions::StateAction = serde_json::from_value(serde_json::json!({
        "type": "chat/inputCompleted", "requestId": ASK_ID, "response": "accept",
        "answers": {
            "editor": {"state": "submitted", "value": {
                "kind": "selected", "value": "Monaco", "freeformValues": null,
            }},
            "scope": {"state": "skipped", "freeformValues": null},
        },
    }))
    .expect("the completion parses");
    store.update(&mut visual.cx, |s, _| {
        s.book.apply(&chat_uri("sess-1"), &completed)
    });
    drive!(workspace, visual.cx);

    workspace.read_with(&visual.cx, |ws, cx| {
        assert_eq!(ws.diagnostic_pending_ask_id(cx), None, "settled retires");
        assert_eq!(
            ws.diagnostic_tool_call_count(ASK_ID, cx),
            1,
            "the settled card is still the one card"
        );
        let (status, carries_form) = ws
            .diagnostic_find_tool_row(ASK_ID, cx)
            .expect("the settled row exists");
        assert_eq!(status, manox_agent::ToolCallStatus::Success);
        assert!(!carries_form, "the form state retired with the answer");
    });
    manox_agent::thread_store::drop_global_for_test();
}
