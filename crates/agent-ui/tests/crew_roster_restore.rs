//! The crew node's persistent roster (plan PR-A4): the switch-away-and-back
//! / restart rebuild retires its KNOWN DOWNGRADE — the retired path fed an
//! empty message set to the transcript scan (the sub-agent tree rides the
//! x-manox channels, which no message scan can see) and rebuilt nothing;
//! the attach now rebuilds the rail's crew rows from the persistent
//! child-run roster (the work channel's registry, settled runs included),
//! and the roster IS the restore source.
//!
//! Own test binary: the shared harness initializes process-global
//! singletons (see `tests/common/mod.rs` — one `#[gpui::test]` per binary).
//!
//! The roster rows ride the SYNTHETIC `x-manox-work/childRunsChanged`
//! action: the pinned manox tag (v0.6.11) does not declare it yet, so the
//! fold only activates with the later tag bump — the fixtures here pin the
//! wire shape upstream #989 committed.
#![cfg(feature = "test-support")]

mod common;

use common::{init_harness, landing_thread, open_workspace};
use gpui::{AppContext as _, TestAppContext, VisualTestContext};
use steer_agent_chat_ui::ahp_store::work_uri;

/// One roster action, production-shaped (upstream #989's wire form: one run
/// row per action, upserted by `runId`).
fn child_run(run: serde_json::Value) -> ahp_types::actions::StateAction {
    ahp_types::actions::StateAction::Unknown(serde_json::json!({
        "type": "x-manox-work/childRunsChanged",
        "run": run,
    }))
}

/// The attach rebuilds the rail's crew rows from the persistent roster —
/// live rows under the running mark, settled rows under their verdict's
/// mark — where the retired transcript-scan path rebuilt nothing.
#[gpui::test]
async fn the_attach_rebuild_crew_rows_from_the_roster(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    // First attach binds the multiplexer's own store to the session — the
    // book the attach path reads is that store's, so the roster seeds
    // through the pair the diagnostics expose.
    let any = window.into();
    cx.update_window(any, |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            ws.diagnostic_attach_thread(landing_thread("cap-1"), true, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();

    // Seed the roster: one live run, one settled with its verdict — the
    // durable account the live-only background work never carries.
    let seeded = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_chat_store(cx));
    let (store, sid) = seeded.expect("the attach bound the foreground store");
    assert_eq!(sid, "cap-1");
    store.update(&mut visual.cx, |s, cx| {
        for run in [
            serde_json::json!({
                "runId": "sub-1", "kind": "Explore", "label": "survey",
                "status": "running", "startedAt": "2026-10-15T00:00:00.000Z",
                "chat": "ahp-chat:/sub-1",
            }),
            serde_json::json!({
                "runId": "sub-2", "kind": "Write", "label": "draft",
                "status": "settled", "verdict": "timed-out",
                "startedAt": "2026-10-15T00:00:01.000Z",
                "chat": "ahp-chat:/sub-2",
            }),
        ] {
            assert_eq!(
                s.book.apply(&work_uri("cap-1"), &child_run(run)),
                steer_agent_chat_ui::ahp_store::FoldEffect::Changed
            );
        }
        cx.notify();
    });

    // The switch back: the same id re-attaches through the production
    // switch path (same id ⇒ the store pair stays bound), and the roster
    // rebuild runs over the seeded book.
    cx.update_window(any, |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            ws.diagnostic_attach_thread(landing_thread("cap-1"), true, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();

    let rows = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_rail_subagent_rows(cx));
    assert_eq!(
        rows,
        vec![
            (
                "sub-1".to_string(),
                manox_agent::ToolCallStatus::Running,
                "Explore".to_string(),
            ),
            (
                "sub-2".to_string(),
                manox_agent::ToolCallStatus::Error,
                "Write".to_string(),
            ),
        ],
        "the rebuild lists the whole roster — live AND settled — in dispatch order"
    );
    manox_agent::thread_store::drop_global_for_test();
    agent_ui::dispatch::clear_globals_for_test();
}
