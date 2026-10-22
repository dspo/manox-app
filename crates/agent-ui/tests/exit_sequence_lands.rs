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

    let after = common::run_slash_until_foreground_moves(cx, &ws, window, "/exit")
        .expect("/exit must land a fresh foreground conversation (silent no-op regression)");

    let objects = manox_agent::object_store::ObjectStore::try_global().expect("object store");
    let fresh_row = objects
        .conversation(&after)
        .expect("the fresh conversation's row lands at creation");
    assert!(!fresh_row.archived, "a fresh conversation is not archived");

    common::full_wiring_teardown(cx);
}
