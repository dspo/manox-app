//! FULL-WIRING regression: a thread created FOR a project (the sidebar's
//! 新建会话 → Steer row hands `start_new_thread` the project directory) must
//! carry that directory on the session channel the composer's project chip
//! reads — the user never picks the folder a second time.
//!
//! The regression class: `start_new_thread` sends the directory in the
//! `createSession` intent, but the session channel's seed for a
//! just-created session carried no working directories (the store row is
//! minted by the list scan, which only sees the journal the first turn
//! materializes), so the chip fell back to "choose project" until the first
//! turn landed.
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
async fn new_thread_inherits_the_project_directory(cx: &mut TestAppContext) {
    // Hermetic HOME: the real host boots against a throwaway state root, not
    // the developer's ~/.manox.
    cx.update(|_cx| {
        manox_agent::runtime::init_hermetic_for_test();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init();
    });
    cx.background_executor.allow_parking();

    // A deterministic provider registry: the Captain strip's provider segment
    // and wire tint only exist when the label resolves, and the hermetic
    // state root registers no provider at all.
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
    cx.update(|_cx| {
        steer_i18n::init();
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

    let before = cx.update(|cx| ws.read(cx).diagnostic_foreground_session(cx));

    // The project directory the sidebar row hands the new thread.
    let project = tempfile::tempdir().expect("project dir");
    let project_path = project.path().to_path_buf();

    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| {
            ws.start_new_thread(Some(project_path.clone()), window, cx);
        });
    });

    let mut landed = None;
    for _ in 0..PARK_ROUNDS {
        cx.run_until_parked();
        let now = cx.update(|cx| ws.read(cx).diagnostic_foreground_session(cx));
        if now != before && now.is_some() {
            landed = now;
            break;
        }
        std::thread::sleep(PARK_STEP);
    }
    assert!(
        landed.is_some(),
        "the new thread must bind a foreground session"
    );

    // The directory must land on the session channel the composer chip
    // reads. Poll: the seed arrives asynchronously over the attach.
    let mut cwd = None;
    for _ in 0..PARK_ROUNDS {
        cx.run_until_parked();
        cwd = cx.update(|cx| ws.read(cx).diagnostic_foreground_cwd(cx));
        if cwd.is_some() {
            break;
        }
        std::thread::sleep(PARK_STEP);
    }
    assert_eq!(
        cwd.as_deref(),
        project_path.to_str(),
        "the composer must inherit the project directory the thread was created for"
    );

    // The Captain strip's model identity: wire-shaped (so `ModelTag` can
    // resolve it) and resolving to a provider + a wire api. A display name
    // here resolves to neither — the provider segment and the model's
    // wire-api tint both vanish, which is the regression this pins.
    let label = cx.update(|cx| ws.read(cx).diagnostic_model_label(cx));
    assert!(
        label.starts_with("test-anthropic/"),
        "the strip's label must be the canonical wire id, got {label:?}"
    );
    let tag = steer_agent_chat_ui::model_tag::ModelTag::resolve(&label);
    assert_eq!(
        tag.provider.as_deref(),
        Some("Test"),
        "the strip must name the provider (label {label:?})"
    );
    assert_eq!(
        tag.wire_api.as_deref(),
        Some("anthropic"),
        "the model segment must carry the wire-api tint (label {label:?})"
    );
    manox_agent::thread_store::drop_global_for_test();
    agent_ui::dispatch::clear_globals_for_test();
}
