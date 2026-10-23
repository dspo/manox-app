//! FULL-WIRING regression for `/exit` from a clean boot: the command must
//! land a fresh foreground conversation AND mark the archived thread's
//! object row — the bit is what keeps the row from outliving the archive as
//! a zombie draft candidate. The pre-fix failure was a silent no-op; the
//! progress notes journal on the archived thread (they are NOT asserted on
//! the fresh transcript by design).
#![cfg(feature = "test-support")]

mod common;

use gpui::TestAppContext;

#[gpui::test]
async fn exit_command_lands_a_new_foreground(cx: &mut TestAppContext) {
    let (ws, window) = common::full_wiring_boot(cx);
    let before = common::foreground_session(cx, &ws).expect("the landing session is bound");

    let after = common::run_slash_until_foreground_moves(cx, &ws, window, "/exit")
        .expect("/exit must land a fresh foreground conversation (silent no-op regression)");
    assert_ne!(
        before, after,
        "/exit must not land on the thread it archived"
    );

    let objects = manox_agent::object_store::ObjectStore::try_global().expect("object store");
    let archived_row = objects
        .conversation(&before)
        .expect("the archived conversation keeps its row");
    assert!(
        archived_row.archived,
        "/exit must mark the archived conversation's object row"
    );
    let fresh_row = objects
        .conversation(&after)
        .expect("the fresh conversation's row lands at creation");
    assert!(!fresh_row.archived, "a fresh conversation is not archived");

    common::full_wiring_teardown(cx);
}
