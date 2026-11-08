//! FULL-WIRING repro matching the user's real sequence: open the app, run
//! `/new`, run `/new` again, then `/exit`. Since the launcher flip (design
//! §3.3) none of these mints anything — every one lands the launcher with the
//! foreground pinned — and the SENDS around them must land fresh foreground
//! conversations, never reopening the archived landings. The regression
//! class: a landing that quietly reuses (or fails to leave) the thread it
//! just archived.
#![cfg(feature = "test-support")]

mod common;

use gpui::TestAppContext;

#[gpui::test]
async fn new_then_exit_lands_the_launcher_each_time(cx: &mut TestAppContext) {
    let (ws, window) = common::full_wiring_boot(cx);
    let objects = manox_agent::object_store::ObjectStore::try_global().expect("object store");
    let landing = common::foreground_session(cx, &ws).expect("the landing session is bound");

    // ── /new: archive + launcher landing, no mint ─────────────────────
    common::run_slash(cx, &ws, window, "/new");
    assert!(
        common::wait_launcher(cx, &ws),
        "the first /new lands the launcher"
    );
    assert_eq!(
        common::foreground_session(cx, &ws).as_deref(),
        Some(landing.as_str()),
        "/new mints nothing — the foreground stays pinned"
    );
    assert!(
        objects
            .conversation(&landing)
            .expect("the archived landing keeps its row")
            .archived,
        "archiving must mark the object row"
    );

    // ── the send is the mint ──────────────────────────────────────────
    common::launcher_send(cx, &ws, window, "second conversation");
    let after_send = common::wait_foreground_moved(cx, &ws, &landing)
        .expect("the launcher send must land a fresh foreground");

    // ── /new again, then /exit: every landing is the launcher ────────
    common::run_slash(cx, &ws, window, "/new");
    assert!(
        common::wait_launcher(cx, &ws),
        "the second /new lands the launcher too"
    );
    common::run_slash(cx, &ws, window, "/exit");
    assert!(
        common::wait_launcher(cx, &ws),
        "/exit lands the launcher, not a fresh mint"
    );

    // The post-exit send must mint a fresh conversation — never reopen the
    // archived landing the refill machinery of old would have named.
    common::launcher_send(cx, &ws, window, "third conversation");
    let after_exit = common::wait_foreground_moved(cx, &ws, &after_send)
        .expect("/exit's launcher must send into a NEW conversation");
    assert_ne!(
        after_exit, landing,
        "the post-exit send must not reopen the archived landing"
    );
    let row = objects
        .conversation(&after_exit)
        .expect("the fresh conversation's row lands at creation");
    assert!(!row.archived, "the landed conversation is not archived");

    common::full_wiring_teardown(cx);
}
