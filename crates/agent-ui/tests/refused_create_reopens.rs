//! FULL-WIRING pin for the reopen leg: a refill that names a live draft the
//! host still carries gets `createSession` refused with the protocol's
//! SESSION_ALREADY_EXISTS — the landing must OPEN that session (the refill's
//! whole point: return to the unfinished conversation) instead of dying on
//! the refusal. The archive-time bit keeps retired rows out of the refill, so
//! a refusal that gets here is a live blank draft — the same shape the host's
//! claim table reuses for a blank id.
//!
//! The shape is forged with public APIs only: the boot landing is
//! host-created, its row unarchived and never spoken; a second mint moves
//! the foreground away, and stamping that one as spoken (what any first
//! message does) leaves the landing as the draft pool's only candidate.
#![cfg(feature = "test-support")]

mod common;

use gpui::TestAppContext;

#[gpui::test]
async fn refused_create_reopens_the_held_session(cx: &mut TestAppContext) {
    let (ws, window) = common::full_wiring_boot(cx);
    let objects = manox_agent::object_store::ObjectStore::try_global().expect("object store");
    let landing = common::foreground_session(cx, &ws).expect("landing session bound");

    // A second conversation (foreground moves there once its create lands);
    // the landing stays unarchived and never spoken — host-held.
    let second = common::new_container(cx, &ws, window, None);
    assert_ne!(second, landing, "the foreground refill excludes itself");
    for _ in 0..common::PARK_ROUNDS {
        cx.run_until_parked();
        if common::foreground_session(cx, &ws).as_deref() == Some(second.as_str()) {
            break;
        }
        std::thread::sleep(common::PARK_STEP);
    }
    assert_eq!(
        common::foreground_session(cx, &ws).as_deref(),
        Some(second.as_str()),
        "the second mint must attach"
    );

    // The user spoke in the current conversation (any first message marks
    // its row materialized) — leaving the landing as the draft pool's only
    // candidate: unarchived, never spoken, and a session the host holds.
    objects.mark_human_message(&second, 1_770_000_000);

    let minted = common::new_container(cx, &ws, window, None);
    assert_eq!(
        minted, landing,
        "the refill must name the live draft the host still carries"
    );

    // The reopen leg: the create refusal is not a dead end — the landing
    // opens and the foreground moves onto it. Reverting the leg strands the
    // foreground on `second` and this wait times out.
    for _ in 0..common::PARK_ROUNDS {
        cx.run_until_parked();
        if common::foreground_session(cx, &ws).as_deref() == Some(landing.as_str()) {
            common::full_wiring_teardown(cx);
            return;
        }
        std::thread::sleep(common::PARK_STEP);
    }
    panic!(
        "a refused (already-existing) create must still land the foreground on {}",
        landing
    );
}
