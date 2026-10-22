//! FULL-WIRING regressions for the zombie-draft chain: archiving must mark
//! the object row (never mint a new zombie), and `/exit` must land even when
//! the draft pool holds a session the host still carries (the reopen leg for
//! EXISTING pre-fix zombies — reverting either half fails a test here).
#![cfg(feature = "test-support")]

mod common;

use gpui::TestAppContext;

#[gpui::test]
async fn exit_refills_a_zombie_draft_and_still_lands(cx: &mut TestAppContext) {
    let (ws, window) = common::full_wiring_boot(cx);
    let objects = manox_agent::object_store::ObjectStore::try_global().expect("object store");

    // 1. `/new` archives the landing conversation. The archived row must
    //    carry the decision: an unmarked row is a zombie draft candidate
    //    that outlives its own archive.
    let first = common::foreground_session(cx, &ws).expect("landing session bound");
    let second = common::run_slash_until_foreground_moves(cx, &ws, window, "/new")
        .expect("/new moves the foreground");
    let first_row = objects
        .conversation(&first)
        .expect("the archived conversation keeps its row");
    assert!(
        first_row.archived,
        "archiving must mark the object row — an unmarked row is a zombie draft"
    );

    // 2. `/exit` lands a fresh conversation even with host-known rows in
    //    the archive (the draft pool's exclusion must not dead-end the
    //    mint).
    let third = common::run_slash_until_foreground_moves(cx, &ws, window, "/exit")
        .expect("/exit must land a fresh foreground conversation");
    assert_ne!(second, third);
    let exited_row = objects
        .conversation(&second)
        .expect("the exited conversation keeps its row");
    assert!(
        exited_row.archived,
        "/exit must mark the archived conversation's object row"
    );
    let fresh_row = objects
        .conversation(&third)
        .expect("the fresh conversation's row lands at creation");
    assert!(!fresh_row.archived, "a fresh conversation is not archived");

    common::full_wiring_teardown(cx);
}
