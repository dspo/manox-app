//! Regression for the launcher submits' spawn-cwd fallback: the three
//! submits (terminal, CLI agent, pi) run inside `Context<Workspace>`, so the
//! fallback must read the workspace they already hold. Reaching it through
//! the dispatch global re-enters the held lease — `cannot read
//! agent_ui::workspace::Workspace while it is already being updated` — and
//! takes the app down through the macOS event boundary (the 2026-11-24
//! pi-action crash).
//!
//! Light harness (no real AHP host): the assertions are the resolution
//! itself and that it lands on the foreground container's attrs; nothing is
//! spawned.
//!
//! Own test binary: the harness initializes process-global singletons (see
//! `tests/common/mod.rs` — one `#[gpui::test]` per binary).
#![cfg(feature = "test-support")]

mod common;

use gpui::TestAppContext;

#[gpui::test]
fn the_launcher_fallback_resolves_inside_the_held_workspace_lease(cx: &mut TestAppContext) {
    common::init_harness(cx);
    let (_window, ws) = common::open_workspace(cx);
    // The production shape: the mounted workspace IS the dispatch global. The
    // global face would therefore re-enter this very entity's lease.
    cx.update(|_cx| agent_ui::dispatch::set_workspace(ws.clone()));

    let attrs = cx.update(|cx| ws.update(cx, |ws, cx| ws.diagnostic_spawn_attrs(cx)));

    // The resolution matches what the workspace's own foreground-container
    // read yields — not a stale or home-substituted value. (The harness boots
    // with a bound session, so a foreground container always exists here.)
    let (expected_cwd, expected_env) = cx.update(|cx| {
        let fg = ws
            .read(cx)
            .foreground_container_id(cx)
            .expect("the harness boots a bound session");
        let attrs = ws.read(cx).container_attrs(&fg, cx);
        (attrs.cwd, attrs.env)
    });
    assert_eq!(attrs.cwd, expected_cwd, "the foreground container's cwd");
    assert_eq!(attrs.env, expected_env, "the foreground container's env");

    cx.update(|_cx| agent_ui::dispatch::clear_globals_for_test());
}
