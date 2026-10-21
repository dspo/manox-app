//! FULL-WIRING regression: `/new` mints a conversation and must land its
//! object row, exactly as the sidebar's 新建会话 path does.
//!
//! The regression class: `/new` sent `createSession` but never wrote the
//! conversation's row, so the fresh thread had no object behind it while
//! every picker kept addressing it. Picking a folder then died on the spot —
//! `ObjectStore::bump` asserts on a config write for a conversation the
//! store has never seen, and in a debug build that assert is an `abort()`.
//!
//! Own test binary: the harness initializes process-global singletons (see
//! `tests/common/mod.rs` — one `#[gpui::test]` per binary), and this one
//! dials the REAL in-proc host, so the store's state root must be hermetic.
#![cfg(feature = "test-support")]

use agent_ui::Workspace;
use gpui::{TestAppContext, VisualTestContext, px, size};
use gpui_component::Root;
use steer_agent_chrome_ui::register_fonts;

/// The real-host create/attach round-trip completes on the tokio runtime and
/// only then wakes the gpui side; poll instead of a single `run_until_parked`.
const PARK_ROUNDS: usize = 100;
const PARK_STEP: std::time::Duration = std::time::Duration::from_millis(50);

#[gpui::test]
async fn new_command_lands_the_conversation_row(cx: &mut TestAppContext) {
    // Hermetic HOME: the real host boots against a throwaway state root, not
    // the developer's ~/.manox. `manox_agent::init` installs the object store
    // under that root, so the row this test reads is the test's own.
    cx.update(|_cx| {
        manox_agent::runtime::init_hermetic_for_test();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init();
        // `init_hermetic_for_test` installs the runtime only; the app's real
        // boot goes through `manox_agent::init`, which also installs the
        // object store. Without it every store call in the app no-ops (they
        // all gate on `try_global`), and this test would pass vacuously.
        manox_agent::object_store::init();
    });
    cx.background_executor.allow_parking();

    let registry = std::sync::Arc::new(manox_harness::ProviderRegistry::new());
    registry
        .register_provider(
            "test-anthropic",
            manox_harness::ProviderConfig {
                name: Some("Test".into()),
                base_url: Some("https://example.invalid".into()),
                api: Some(manox_harness::provider_registry::Api::AnthropicMessages),
                models: vec![manox_harness::provider_registry::ProviderModelConfig {
                    id: "t-model".into(),
                    name: "T Model".into(),
                    reasoning: false,
                    input: vec![],
                    context_window: 8_192,
                    max_tokens: 1_024,
                    cost: Default::default(),
                    api: None,
                    base_url: None,
                    metadata: Default::default(),
                }],
                ..Default::default()
            },
        )
        .expect("register");
    manox_agent::provider_glue::install_for_test(registry);

    let ws_cell: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let (ws_for_build,) = (ws_cell.clone(),);
    cx.update(gpui_component::init);
    cx.update(|cx| {
        steer_i18n::init();
        // The slash registry is app-startup state (`main` calls this before
        // any workspace exists); without it `/new` parses to nothing.
        agent_ui::slash_command::init(cx);
    });
    let window = cx.open_window(size(px(1_280.), px(820.)), move |window, cx| {
        register_fonts(cx);
        let shell = agent_ui::chrome_assembly::mount(window, cx);
        let ws = agent_ui::dispatch::workspace_global().expect("assembly workspace");
        *ws_for_build.borrow_mut() = Some(ws);
        Root::new(shell, window, cx)
    });
    cx.run_until_parked();
    let ws = ws_cell.borrow().clone().expect("workspace captured");
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    for _ in 0..PARK_ROUNDS {
        cx.run_until_parked();
        if cx.update(|cx| ws.read(cx).diagnostic_ahp_connected(cx)) {
            break;
        }
        std::thread::sleep(PARK_STEP);
    }
    cx.run_until_parked();

    let before = cx
        .update(|cx| ws.read(cx).diagnostic_foreground_session(cx))
        .expect("the landing session must be foreground before /new");

    let parsed = agent_ui::slash_command::parse("/new").expect("/new is a registered command");
    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| {
            agent_ui::slash_command::dispatch(&parsed, ws, window, cx);
        });
    });

    // `/new` mints its id client-side and the create lands asynchronously, so
    // poll for the foreground to move off the landing session.
    let mut landed = None;
    for _ in 0..PARK_ROUNDS {
        cx.run_until_parked();
        let now = cx.update(|cx| ws.read(cx).diagnostic_foreground_session(cx));
        if let Some(id) = now
            && id != before
        {
            landed = Some(id);
            break;
        }
        std::thread::sleep(PARK_STEP);
    }
    let sid = landed.expect("the /new thread must bind a foreground session");

    // The row is the conversation's object: it must exist the moment the
    // thread does, without waiting for the first message (nothing else writes
    // one for this id — `set_draft` targets the outgoing thread).
    let objects = manox_agent::object_store::ObjectStore::try_global().expect("object store");
    let row = objects
        .conversation(&sid)
        .expect("the /new create path must land the conversation's object row");
    assert!(
        row.last_human_message_at.is_none(),
        "a fresh conversation has not happened yet"
    );
    assert!(
        objects.list_conversations().iter().all(|r| r.id != sid),
        "an unmaterialized conversation never lists in the sidebar"
    );

    manox_agent::thread_store::drop_global_for_test();
    agent_ui::dispatch::clear_globals_for_test();
}
