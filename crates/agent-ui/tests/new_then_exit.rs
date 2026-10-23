//! FULL-WIRING repro matching the user's real sequence: open the app, run
//! `/new` (which visibly worked), run `/new` again, then `/exit`. Every step
//! must land a fresh foreground conversation, and `/exit` must not come back
//! to the boot landing it archived. The regression class: `/exit`'s fresh
//! landing refilled a zombie draft whose `createSession` the host refused, and
//! the refusal died silently — the command looked dead while `/new` (which
//! mints its id outright) kept working.
#![cfg(feature = "test-support")]

mod common;

use gpui::TestAppContext;

#[gpui::test]
async fn new_then_exit_lands_fresh_each_time(cx: &mut TestAppContext) {
    let (ws, window) = common::full_wiring_boot(cx);
    let landing = common::foreground_session(cx, &ws).expect("the landing session is bound");

    let after_new = common::run_slash_until_foreground_moves(cx, &ws, window, "/new")
        .expect("the first /new moves the foreground");
    let after_new2 = common::run_slash_until_foreground_moves(cx, &ws, window, "/new")
        .expect("the second /new must mint fresh, not refill");
    let after_exit = common::run_slash_until_foreground_moves(cx, &ws, window, "/exit")
        .expect("/exit must land a NEW conversation, not a silent no-op");
    assert_ne!(after_new, after_new2);
    assert_ne!(after_new2, after_exit);
    assert_ne!(
        after_exit, landing,
        "/exit must mint fresh instead of reopening the archived landing"
    );
    let row = manox_agent::object_store::ObjectStore::try_global()
        .expect("object store")
        .conversation(&after_exit)
        .expect("the fresh conversation's row lands at creation");
    assert!(!row.archived, "the landed conversation is not archived");

    common::full_wiring_teardown(cx);
}
