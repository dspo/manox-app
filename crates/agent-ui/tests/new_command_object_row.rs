//! FULL-WIRING regression for the create path's object row. The premise
//! moved with the launcher flip (design §3.3): `/new` itself mints NOTHING —
//! it archives the outgoing conversation and lands the launcher — so the
//! regression class ("a create that skips its row leaves every picker
//! addressing an id the store has never seen") now lives on the launcher's
//! ⏎ send, the one remaining native mint: its row must land at creation,
//! unmaterialized and unlisted, exactly as the old paths owed.
//!
//! Own test binary: the harness initializes process-global singletons (see
//! `tests/common/mod.rs` — one `#[gpui::test]` per binary), and this one
//! dials the REAL in-proc host, so the store's state root must be hermetic.
#![cfg(feature = "test-support")]

mod common;

use gpui::TestAppContext;

#[gpui::test]
async fn new_command_lands_nothing_and_the_send_lands_the_row(cx: &mut TestAppContext) {
    // Hermetic HOME: the real host boots against a throwaway state root, not
    // the developer's ~/.manox. `manox_agent::init` installs the object store
    // under that root, so the row this test reads is the test's own.
    cx.update(|_cx| {
        manox_agent::runtime::init_hermetic_for_test();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init();
        // `init_hermetic_for_test` installs the runtime only; the app's real
        // boot goes through `manox_agent::init`, which also installs the
        // object store. Without it every store call in the app no-ops (they
        // all gate on `try_global`), and this test would pass vacuously.
        manox_agent::object_store::init();
    });
    cx.background_executor.allow_parking();

    let (ws, window) = common::full_wiring_boot(cx);
    let before = common::foreground_session(cx, &ws).expect("the landing session is bound");
    let objects = manox_agent::object_store::ObjectStore::try_global().expect("object store");
    let rows_before = objects.list_conversations().len();

    // `/new` archives the outgoing conversation and lands the launcher — no
    // mint, no new row, foreground pinned.
    common::run_slash(cx, &ws, window, "/new");
    assert!(
        common::wait_launcher(cx, &ws),
        "/new lands the launcher without minting"
    );
    assert_eq!(
        common::foreground_session(cx, &ws).as_deref(),
        Some(before.as_str()),
        "/new mints nothing"
    );
    assert_eq!(
        objects.list_conversations().len(),
        rows_before,
        "/new must not land an object row — it no longer creates anything"
    );

    // The launcher's ⏎ send is the create: its object row must exist the
    // moment the thread does, without waiting for the first message (nothing
    // else writes one for this id).
    common::launcher_send(cx, &ws, window, "the first message");
    let sid = common::wait_foreground_moved(cx, &ws, &before)
        .expect("the launcher send must bind a fresh foreground session");
    let row = objects
        .conversation(&sid)
        .expect("the launcher create path must land the conversation's object row");
    assert!(
        !row.archived,
        "the created conversation's row starts unarchived"
    );
    assert!(
        objects.list_conversations().iter().all(|r| r.id != sid),
        "an unmaterialized conversation never lists in the sidebar"
    );

    common::full_wiring_teardown(cx);
}
