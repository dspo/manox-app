//! FULL-WIRING regressions for the zombie-draft chain: archiving must mark
//! the object row (never mint a new zombie), and a later mint must skip the
//! archived row instead of refilling it into a `createSession` the host
//! answers "already exists". The archived row is made the draft pool's ONLY
//! candidate (the fresh conversation is stamped spoken), so removing the bit
//! write deterministically lands the next mint back on it.
#![cfg(feature = "test-support")]

mod common;

use gpui::TestAppContext;

#[gpui::test]
async fn archived_draft_is_not_refilled_and_exit_lands(cx: &mut TestAppContext) {
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

    // 2. The fresh conversation is spoken, so the archived landing is the
    //    draft pool's only candidate. `/exit` must mint a new conversation,
    //    never reopen the archived one the refill would otherwise name.
    objects.mark_human_message(&second, 1_770_000_000);
    let third = common::run_slash_until_foreground_moves(cx, &ws, window, "/exit")
        .expect("/exit must land a fresh foreground conversation");
    assert_ne!(
        third, first,
        "/exit must not reopen the archived draft it just left"
    );
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
