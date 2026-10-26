//! Regression: a tool confirmation park (a `require_escalated` escalation
//! from Edit/Write/Bash) must surface as the unified confirmation card — the
//! parked call promoted to its own conversation row carrying the fold's
//! decision buttons — and the verdict must clear the parked state. Before
//! the unified card, such a park either blocked the thread invisibly or
//! surfaced as a full-column modal overlay foreign to the conversation.
//!
//! The test drives the PRODUCTION path: the seed sets only the parked state;
//! the row promotion + snapshot attach happen in the same
//! `sync_confirmation_snapshot` pass the render loop runs.
#![cfg(feature = "test-support")]

mod common;

use common::{init_harness, open_workspace};
use gpui::{AppContext as _, TestAppContext, VisualTestContext};
use manox_agent::PermissionDecision;
use steer_agent_chat_ui::ahp_store::{AhpStore, chat_uri};

#[gpui::test]
async fn confirmation_park_surfaces_as_the_conversation_card(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let before = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_pending_confirmation(cx));
    assert!(before.is_none(), "no pending card before the seed");

    // The parked call exists the production way: a tool-call part parked in
    // `pending-confirmation` on the fold. The workspace-side seed arms the
    // pending state (the diagnostic stand-in for the session channel's
    // input-needed entry); the sync projects the promoted card from the part.
    let store = cx.new(|_| AhpStore::detached());
    store.update(&mut visual.cx, |s, _| {
        s.diagnostic_seed_session_for_chat("sess-1", "sess-1")
    });
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store.clone(), "sess-1", cx);
    });
    store.update(&mut visual.cx, |s, _| {
        let mut route = |action: ahp_types::actions::StateAction| {
            s.book.apply(&chat_uri("sess-1"), &action);
        };
        route(ahp_types::actions::StateAction::ChatTurnStarted(
            ahp_types::actions::ChatTurnStartedAction {
                turn_id: "turn-1".to_string(),
                started_at: "2026-09-30T00:00:00Z".to_string(),
                message: serde_json::from_value(serde_json::json!({
                    "text": "edit it", "origin": { "kind": "user" },
                }))
                .expect("a user message parses"),
                queued_message_id: None,
                meta: None,
            },
        ));
        route(ahp_types::actions::StateAction::ChatToolCallStart(
            ahp_types::actions::ChatToolCallStartAction {
                turn_id: "turn-1".to_string(),
                tool_call_id: "call_1".to_string(),
                meta: None,
                tool_name: "Bash".to_string(),
                display_name: "Bash".to_string(),
                intention: Some("Run the build".to_string()),
                contributor: None,
            },
        ));
        route(ahp_types::actions::StateAction::ChatToolCallReady(
            ahp_types::actions::ChatToolCallReadyAction {
                turn_id: "turn-1".to_string(),
                tool_call_id: "call_1".to_string(),
                meta: None,
                contributor: None,
                intention: None,
                invocation_message: "Run the build".into(),
                tool_input: Some(ahp_types::state::ToolInput::Inline(
                    "{\"command\": \"cargo build\"}".to_string(),
                )),
                confirmation_title: None,
                risk_assessment: None,
                edits: None,
                editable: None,
                options: None,
                confirmed: None,
            },
        ));
    });
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_seed_pending_confirmation("call_1", "call_1", cx);
    });

    // The store observe arms the park (the live ask edge) and then projects:
    // the parked call surfaces as the top-level confirmation card in the same
    // pass. The test drives the two diagnostics directly.
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_sync_live_ask(cx);
        ws.diagnostic_sync_conversation(cx);
    });
    visual.cx.run_until_parked();
    let pending = workspace
        .read_with(&visual.cx, |ws, cx| ws.diagnostic_pending_confirmation(cx))
        .expect("the park surfaces as the pending confirmation");
    assert_eq!(pending.0, "call_1");
    assert_eq!(
        pending.1, "call_1",
        "the promotion keys the row by the tool-call id"
    );
    let attached =
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_confirmation_snapshot(cx));
    assert_eq!(
        attached.as_deref(),
        Some("call_1"),
        "the sync promotes the row and budgets the decision snapshot"
    );
    assert!(
        workspace.read_with(&visual.cx, |ws, cx| ws
            .diagnostic_list_state(cx)
            .item_count()
            > 0),
        "the promotion re-syncs the virtual list count — without it the card
         (and its buttons) never render"
    );
    assert!(
        workspace.read_with(&visual.cx, |ws, cx| ws
            .diagnostic_blocking_overlay_active(cx)),
        "the parked decision counts as a blocking surface for the overlays' mutex"
    );

    // The verdict names the park it settles; a mismatched id must be
    // ignored (a stale button must not settle a different park).
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.resolve_auth_for_test("other_park", PermissionDecision::Deny, cx);
    });
    let guarded = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_pending_confirmation(cx));
    assert_eq!(
        guarded
            .expect("a mismatched verdict never clears the park")
            .0,
        "call_1"
    );

    // The matching verdict clears the card, and the next sync strips the
    // row's decision snapshot.
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.resolve_auth_for_test("call_1", PermissionDecision::Deny, cx);
    });
    let after = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_pending_confirmation(cx));
    assert!(after.is_none(), "card cleared by the verdict");
    visual.cx.run_until_parked();
    let detached =
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_confirmation_snapshot(cx));
    assert!(detached.is_none(), "the sync strips the retired snapshot");

    // The wire's restore path: a new park re-arms the same surface.
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_seed_pending_confirmation("call_2", "call_2", cx);
    });
    visual.cx.run_until_parked();
    let rearmed = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_pending_confirmation(cx));
    assert_eq!(rearmed.expect("re-arm works").0, "call_2");
    manox_agent::thread_store::drop_global_for_test();
}
