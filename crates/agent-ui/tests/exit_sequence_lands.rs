//! FULL-WIRING regression for `/exit` from a clean boot: the command must
//! land the LAUNCHER (no fresh sibling is minted — design §3.3), mark the
//! archived thread's object row, and the next send from the launcher must
//! mint a fresh conversation instead of reopening the archived landing. The
//! pre-fix failure was a silent no-op; the progress notes journal on the
//! archived thread (they are NOT asserted on any transcript by design).
#![cfg(feature = "test-support")]

mod common;

use gpui::TestAppContext;

#[gpui::test]
async fn exit_command_lands_the_launcher(cx: &mut TestAppContext) {
    let (ws, window) = common::full_wiring_boot(cx);
    let before = common::foreground_session(cx, &ws).expect("the landing session is bound");

    common::run_slash(cx, &ws, window, "/exit");
    assert!(
        common::wait_launcher(cx, &ws),
        "/exit must land the launcher (silent no-op regression)"
    );
    // The landing mints nothing: the foreground stays pinned on the archived
    // session's store until the next landing replaces it.
    assert_eq!(
        common::foreground_session(cx, &ws).as_deref(),
        Some(before.as_str()),
        "/exit must not mint a fresh sibling"
    );

    let objects = manox_agent::object_store::ObjectStore::try_global().expect("object store");
    let archived_row = objects
        .conversation(&before)
        .expect("the archived conversation keeps its row");
    assert!(
        archived_row.archived,
        "/exit must mark the archived conversation's object row"
    );

    // The launcher's send is the mint: a fresh conversation, never the
    // archived landing.
    common::launcher_send(cx, &ws, window, "after exit");
    let after = common::wait_foreground_moved(cx, &ws, &before)
        .expect("the launcher send must land a fresh foreground conversation");
    let fresh_row = objects
        .conversation(&after)
        .expect("the fresh conversation's row lands at creation");
    assert!(!fresh_row.archived, "a fresh conversation is not archived");

    common::full_wiring_teardown(cx);
}
