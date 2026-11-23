//! The pi runtime against the real `dist/pi-acp`, bind-level. Gated: it needs a
//! built harness (`cd pi-harness && bun run build`) and no model.
//!
//! ```text
//! MANOX_RUN_LIVE=1 cargo test -p steer-agent-chat-ui --test pi_runtime_live -- --ignored
//! ```

use std::time::Duration;

use steer_agent_chat_ui::pi::{OpenMode, PiBook, PiRuntime, PiSessionState, StoreEvent};

const TIMEOUT: Duration = Duration::from_secs(60);
const SESSION: &str = "22222222-2222-4222-8222-222222222222";

#[tokio::test]
#[ignore = "needs a built pi-acp; enable with MANOX_RUN_LIVE=1"]
async fn the_real_harness_binds_through_the_runtime() {
    if std::env::var("MANOX_RUN_LIVE").as_deref() != Ok("1") {
        eprintln!("MANOX_RUN_LIVE is not 1; skipping the live pi runtime test");
        return;
    }
    let Some(bin) = steer_acp::resolve_pi_acp() else {
        panic!("no pi-acp found; run `cd pi-harness && bun run build` or set STEER_PI_ACP");
    };
    let root = std::env::temp_dir().join(format!("steer-pi-live-{}", std::process::id()));
    let agent_dir = root.join("pi-agent");
    std::fs::create_dir_all(&agent_dir).expect("the temporary state root is created");

    let runtime = PiRuntime::new(bin, &root, &root)
        .with_env("PI_CODING_AGENT_DIR", agent_dir.display().to_string());
    let (events, receiver) = async_channel::unbounded();
    let connection = tokio::time::timeout(
        TIMEOUT,
        runtime.open(Some(SESSION), &root, OpenMode::History, &events),
    )
    .await
    .expect("the bind answers")
    .expect("the session opens (an empty rebuild when the file is absent)");
    assert_eq!(connection.session_id(), SESSION);

    let mut book = PiBook::new();
    book.open(SESSION, &root);
    let mut ready = false;
    while !ready {
        let event = tokio::time::timeout(TIMEOUT, receiver.recv())
            .await
            .expect("the bind reports")
            .expect("the channel stays open");
        ready = matches!(event, StoreEvent::Ready { .. });
        book.apply(&event);
    }
    let view = book.view(SESSION).expect("the view exists");
    assert_eq!(view.state, PiSessionState::Ready);
    assert!(
        !view.config_options.is_empty(),
        "the real harness announces its model and thinking selectors"
    );

    tokio::time::timeout(TIMEOUT, connection.close())
        .await
        .expect("close settles");
    let _ = std::fs::remove_dir_all(&root);
}
