//! Regression: a tool confirmation park (a `sandbox_permissions` escalation
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
use gpui::{TestAppContext, VisualTestContext};
use manox_agent::PermissionDecision;

#[gpui::test]
async fn confirmation_park_surfaces_as_the_conversation_card(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let before = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_pending_confirmation(cx));
    assert!(before.is_none(), "no pending card before the seed");

    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_seed_pending_confirmation("call_1", "call_1", cx);
    });

    // The render loop runs the sync every frame: the seed alone surfaces
    // nothing, the next frame promotes the row and budgets the decision
    // snapshot onto it.
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
