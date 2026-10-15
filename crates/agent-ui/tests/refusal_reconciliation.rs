//! Regression: an envelope carrying `rejectionReason` is the host's
//! refusal — its action must not enter the fold, our optimistic twin rolls
//! back with a corrective notice, a foreign client's refusal stays a
//! diagnostics line, and the watermark advances either way (refusals are
//! sequenced like every other echo, so a reconnect must not replay them).
//!
//! Own test binary: the shared harness initializes process-global
//! singletons (see `tests/common/mod.rs` — one `#[gpui::test]` per binary).
#![cfg(feature = "test-support")]

mod common;

use ahp_types::actions::{ActionEnvelope, ActionOrigin, SessionConfigChangedAction, StateAction};
use ahp_types::common::JsonObject;
use common::init_harness;
use gpui::AppContext as _;
use steer_agent_chat_ui::ahp_store::{AhpStore, CLIENT_ID, chat_uri, session_uri};

fn config_action(pairs: &[(&str, &str)]) -> StateAction {
    let mut config = JsonObject::new();
    for (key, value) in pairs {
        config.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }
    StateAction::SessionConfigChanged(SessionConfigChangedAction {
        config,
        replace: None,
    })
}

fn envelope(
    channel: &str,
    action: StateAction,
    origin: Option<&str>,
    rejection: Option<&str>,
    server_seq: u64,
) -> ahp::ClientEvent {
    ahp::ClientEvent {
        channel: channel.to_string(),
        event: ahp::SubscriptionEvent::Action(ActionEnvelope {
            channel: channel.to_string(),
            action,
            server_seq,
            origin: origin.map(|client_id| ActionOrigin {
                client_id: client_id.to_string(),
                client_seq: 1,
            }),
            rejection_reason: rejection.map(str::to_string),
        }),
    }
}

fn seed_session(
    store: &gpui::Entity<AhpStore>,
    sid: &str,
    cid: &str,
    cx: &mut gpui::TestAppContext,
) {
    store.update(cx, |s, _| {
        let state = serde_json::from_value(serde_json::json!({
            "resource": session_uri(sid),
            "provider": "test",
            "title": sid,
            "status": 1,
            "modifiedAt": "2026-10-15T00:00:00.000Z",
            "lifecycle": "ready",
            "activeClients": [],
            "chats": [],
            "defaultChat": chat_uri(cid),
        }))
        .expect("the session snapshot parses");
        s.book.apply_snapshot(&session_uri(sid), state);
    });
}

/// The notices a consumer actually reads: the display buckets are keyed by
/// the OWNING SESSION, so an `ahp-chat:` refusal must resolve through it
/// rather than through the chat's own id. Draining the chat's bucket here as
/// well would hide a notice posted where nothing reads it — exactly the
/// stranded bubble these tests exist to catch.
fn notice_texts(store: &gpui::Entity<AhpStore>, cx: &mut gpui::TestAppContext) -> Vec<String> {
    store.update(cx, |s, _| {
        s.drain_chat_events("s-1")
            .iter()
            .filter_map(|event| match event {
                steer_agent_chat_ui::chat_fold::ChatEvent::Notice { text } => Some(text.clone()),
                _ => None,
            })
            .collect()
    })
}

/// Our own refused model pick: the value must not enter the fold, the
/// optimistic twin rolls back, a corrective notice lands on the session's
/// display bucket, the diagnostics deque records it, and the watermark
/// still advances past the refusal.
#[gpui::test]
async fn an_own_refusal_rolls_back_notifies_and_advances_the_watermark(
    cx: &mut gpui::TestAppContext,
) {
    init_harness(cx);
    let store = cx.new(|_| AhpStore::detached());
    seed_session(&store, "s-1", "c-1", cx);

    store.update(cx, |s, _| {
        let mut pick = serde_json::Map::new();
        pick.insert(
            "model".to_string(),
            serde_json::Value::String("prov/deepseek".into()),
        );
        s.optimistic_config("s-1", &pick);
    });
    assert_eq!(
        store.read_with(cx, |s, _| {
            steer_agent_chat_ui::ahp_store::leaf(&s.book, "s-1")
                .model_id()
                .map(str::to_string)
        }),
        Some("prov/deepseek".to_string()),
        "the optimistic twin shows the pick before the echo"
    );

    store.update(cx, |s, cx| {
        s.diagnostic_route_event(
            envelope(
                &session_uri("s-1"),
                config_action(&[("model", "prov/deepseek")]),
                Some(CLIENT_ID),
                Some("model prov/deepseek cannot be served: credential gone"),
                7,
            ),
            cx,
        )
    });
    cx.run_until_parked();

    assert_eq!(
        store.read_with(cx, |s, _| {
            steer_agent_chat_ui::ahp_store::leaf(&s.book, "s-1")
                .model_id()
                .map(str::to_string)
        }),
        None,
        "a refused action never enters the fold, and the optimistic twin rolled back"
    );
    assert_eq!(
        store.read_with(cx, |s, _| s.book.server_seq),
        7,
        "the refusal advances the watermark"
    );
    assert!(
        store.read_with(cx, |s, _| s
            .rejections()
            .any(|line| line.contains("cannot be served"))),
        "the refusal is recorded for diagnostics"
    );
    assert!(
        notice_texts(&store, cx)
            .iter()
            .any(|text| text.contains("cannot be served")),
        "our own refusal surfaces a corrective notice"
    );
}

/// Another client's refusal: the action still never enters the fold (the
/// host broadcast the refusal to every subscriber), but nothing of OURS
/// rolls back, no notice lands, and only the diagnostics deque grows.
#[gpui::test]
async fn a_foreign_refusal_folds_nothing_and_stays_quiet(cx: &mut gpui::TestAppContext) {
    init_harness(cx);
    let store = cx.new(|_| AhpStore::detached());
    seed_session(&store, "s-1", "c-1", cx);

    store.update(cx, |s, cx| {
        s.diagnostic_route_event(
            envelope(
                &session_uri("s-1"),
                config_action(&[("model", "prov/other-client-pick")]),
                Some("some-laptop"),
                Some("model prov/other-client-pick cannot be served"),
                3,
            ),
            cx,
        )
    });
    cx.run_until_parked();

    assert!(
        store.read_with(cx, |s, _| {
            steer_agent_chat_ui::ahp_store::leaf(&s.book, "s-1")
                .model_id()
                .is_none()
        }),
        "a refused action never enters the fold, whoever dispatched it"
    );
    assert_eq!(
        store.read_with(cx, |s, _| s.book.server_seq),
        3,
        "the watermark advances regardless"
    );
    assert!(
        store.read_with(cx, |s, _| s
            .rejections()
            .any(|line| line.contains("other-client"))),
        "foreign refusals are diagnostics, not UI"
    );
    assert!(
        notice_texts(&store, cx).is_empty(),
        "a foreign refusal posts no notice on our transcript"
    );
}

/// Our own refused NON-config action (the shape a rejected submit takes):
/// nothing rolls back, but the refusal still speaks — the composer pushes
/// its bubble optimistically, and silence here would strand it with no
/// reply and no explanation.
#[gpui::test]
async fn an_own_refused_non_config_action_still_speaks(cx: &mut gpui::TestAppContext) {
    init_harness(cx);
    let store = cx.new(|_| AhpStore::detached());
    seed_session(&store, "s-1", "c-1", cx);

    store.update(cx, |s, cx| {
        s.diagnostic_route_event(
            envelope(
                &chat_uri("c-1"),
                StateAction::SessionTitleChanged(ahp_types::actions::SessionTitleChangedAction {
                    title: "must not land".into(),
                }),
                Some(CLIENT_ID),
                Some("the session's journal is not writable from this process"),
                11,
            ),
            cx,
        )
    });
    cx.run_until_parked();

    assert_eq!(
        store.read_with(cx, |s, _| s.book.server_seq),
        11,
        "the refusal advances the watermark"
    );
    assert!(
        notice_texts(&store, cx)
            .iter()
            .any(|text| text.contains("not writable")),
        "an own refused action is never silent"
    );
}

/// Three writes to one key in flight, refused in order: the fold must end at
/// the base of the FIRST refusal. Rewriting only the newest later entry
/// leaves the ones in between pointing at the refused value, so their own
/// refusal restores a value the host rejected — the outcome the value-chain
/// scheme exists to rule out.
#[gpui::test]
async fn a_refused_write_never_resurfaces_as_a_rollback_target(cx: &mut gpui::TestAppContext) {
    init_harness(cx);
    let store = cx.new(|_| AhpStore::detached());
    seed_session(&store, "s-1", "c-1", cx);

    store.update(cx, |s, _| {
        for value in ["prov/a", "prov/b", "prov/c"] {
            let mut pick = serde_json::Map::new();
            pick.insert(
                "model".to_string(),
                serde_json::Value::String(value.to_string()),
            );
            s.optimistic_config("s-1", &pick);
        }
    });

    store.update(cx, |s, cx| {
        for (seq, value) in ["prov/a", "prov/b", "prov/c"].into_iter().enumerate() {
            s.diagnostic_route_event(
                envelope(
                    &session_uri("s-1"),
                    config_action(&[("model", value)]),
                    Some(CLIENT_ID),
                    Some("model cannot be served"),
                    seq as u64 + 1,
                ),
                cx,
            );
        }
    });
    cx.run_until_parked();

    assert_eq!(
        store.read_with(cx, |s, _| {
            steer_agent_chat_ui::ahp_store::leaf(&s.book, "s-1")
                .model_id()
                .map(str::to_string)
        }),
        None,
        "the last refusal restores the first refusal's base, never a refused value"
    );
}
