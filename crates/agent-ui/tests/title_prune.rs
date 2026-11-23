//! The title bridge's cleanup judgement, end to end: the store fold queues
//! DELETIONS (a `sessionRemoved` carrying no archive mark) and the
//! projection pump drains them into the title store; an archive exit never
//! queues — the row left the catalogue but the session survives on the host
//! (same id, revivable), so its program title survives too.
//!
//! Driven through the production dispatch point (`diagnostic_route_event`)
//! and the production pump (the chrome assembly's project pass) — not by
//! calling `prune` directly, which would test the wiring's parts and miss
//! its connective tissue.
//!
//! Own test binary: the shared harness initializes process-global singletons
//! (see `tests/common/mod.rs` — one `#[gpui::test]` per binary).
#![cfg(feature = "test-support")]

mod common;

use ahp_types::SessionRemovedParams;
use ahp_types::actions::{ActionEnvelope, StateAction};
use ahp_types::state::SessionSummary;
use common::full_wiring_boot_shell;
use gpui::{AppContext as _, TestAppContext};
use steer_agent_chat_ui::ahp_store::{AhpStore, session_uri};

fn catalogue_row(id: &str) -> SessionSummary {
    serde_json::from_value(serde_json::json!({
        "resource": session_uri(id),
        "provider": "test",
        "title": format!("row {id}"),
        "status": 0,
        "createdAt": "2026-01-01T00:00:00Z",
        "modifiedAt": "2026-01-01T00:00:00Z",
    }))
    .expect("the row parses")
}

fn root_action(action: StateAction) -> ahp::ClientEvent {
    ahp::ClientEvent {
        channel: ahp_types::ROOT_RESOURCE_URI.to_string(),
        event: ahp::SubscriptionEvent::Action(ActionEnvelope {
            channel: ahp_types::ROOT_RESOURCE_URI.to_string(),
            action,
            server_seq: 1,
            origin: None,
            rejection_reason: None,
        }),
    }
}

fn removed(session: &str) -> ahp::ClientEvent {
    ahp::ClientEvent {
        channel: ahp_types::ROOT_RESOURCE_URI.to_string(),
        event: ahp::SubscriptionEvent::SessionRemoved(SessionRemovedParams {
            channel: ahp_types::ROOT_RESOURCE_URI.to_string(),
            session: session_uri(session),
        }),
    }
}

fn archived_changed(ids: &[&str]) -> StateAction {
    StateAction::Unknown(serde_json::json!({
        "type": manox_ahp::ext::actions::ARCHIVED_CHANGED,
        "sessions": ids,
        "archived": true,
    }))
}

fn program_title(session_id: &str) -> Option<String> {
    agent_ui::title_store::TitleStore::global().program_title(session_id)
}

#[gpui::test]
async fn peer_archive_keeps_the_program_title_and_a_deletion_prunes_it(cx: &mut TestAppContext) {
    let (ws, _window, _shell) = full_wiring_boot_shell(cx);

    // Two background sessions with landed program titles. The process-wide
    // store is fresh in this binary, so `feed` mints each interpreter.
    agent_ui::title_store::TitleStore::global().feed("s-arch", b"\x1b]2;archived title\x07");
    agent_ui::title_store::TitleStore::global().feed("s-del", b"\x1b]2;deleted title\x07");
    assert_eq!(program_title("s-arch").as_deref(), Some("archived title"));
    assert_eq!(program_title("s-del").as_deref(), Some("deleted title"));

    // A detached store bound the way the tests re-aim the workspace: the
    // pump drains THIS store's deletion queue.
    let store = cx.update(|cx| cx.new(|_| AhpStore::detached()));
    cx.update(|cx| {
        ws.update(cx, |ws, cx| {
            ws.diagnostic_bind_store(store.clone(), "s-del", cx)
        });
    });
    cx.update(|cx| {
        store.update(cx, |s, _| {
            s.book
                .seed_summaries(vec![catalogue_row("s-arch"), catalogue_row("s-del")]);
        });
    });

    // A peer archives s-arch's container: the accepted `archivedChanged`
    // echo reaches every root subscriber first, then the catalogue diff
    // pump publishes the row's removal. s-del's row leaves with NO archive
    // write — a bare dispose.
    cx.update(|cx| {
        store.update(cx, |s, cx| {
            s.diagnostic_route_event(root_action(archived_changed(&["s-arch"])), cx);
            s.diagnostic_route_event(removed("s-arch"), cx);
            s.diagnostic_route_event(removed("s-del"), cx);
        });
    });
    cx.run_until_parked();

    assert!(
        !cx.update(|cx| store.read(cx).book.summaries.contains_key("s-arch")),
        "the archive exit still drops the catalogue row"
    );
    assert_eq!(
        program_title("s-arch").as_deref(),
        Some("archived title"),
        "an archive exit is not a deletion: the title survives"
    );
    assert!(
        program_title("s-del").is_none(),
        "a removal with no archive mark prunes the title"
    );
}
