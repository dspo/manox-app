//! FULL-WIRING regression for the zombie-draft guard's surviving half: the
//! archive must mark the object row, and the launcher's send must mint a
//! fresh id — never the id of a row the archive just marked (the pre-launcher
//! mint could refill such a row into a `createSession` the host answers
//! "already exists"; the launcher keeps that class dead by minting only
//! client-fresh ids and letting the attach land them).
#![cfg(feature = "test-support")]

mod common;

use gpui::TestAppContext;

#[gpui::test]
async fn archived_rows_are_marked_and_the_send_mints_fresh(cx: &mut TestAppContext) {
    let (ws, window) = common::full_wiring_boot(cx);
    let objects = manox_agent::object_store::ObjectStore::try_global().expect("object store");

    // 1. `/new` archives the landing conversation and lands the launcher —
    //    the archived row must carry the decision (an unmarked row is a
    //    zombie candidate that outlives its own archive).
    let first = common::foreground_session(cx, &ws).expect("landing session bound");
    common::run_slash(cx, &ws, window, "/new");
    assert!(
        common::wait_launcher(cx, &ws),
        "/new lands the launcher without minting"
    );
    let first_row = objects
        .conversation(&first)
        .expect("the archived conversation keeps its row");
    assert!(
        first_row.archived,
        "archiving must mark the object row — an unmarked row is a zombie draft"
    );

    // 2. The send from the launcher mints a NEW id, never the archived one,
    //    and the fresh row starts unarchived.
    common::launcher_send(cx, &ws, window, "after archive");
    let second = common::wait_foreground_moved(cx, &ws, &first)
        .expect("the launcher send must land a fresh foreground conversation");
    assert_ne!(
        second, first,
        "the send must not reopen the row the archive just marked"
    );
    let fresh_row = objects
        .conversation(&second)
        .expect("the fresh conversation's row lands at creation");
    assert!(!fresh_row.archived, "a fresh conversation is not archived");

    common::full_wiring_teardown(cx);
}
