//! FULL-WIRING regression: a thread created FOR a project (the sidebar's
//! 新建会话 → Steer row hands `start_new_thread` the project directory) must
//! carry that directory on the session channel the composer's project chip
//! reads — the user never picks the folder a second time.
//!
//! The regression class: a deferred create's channel must expose the
//! intent before the journal exists. The host seeds it from the
//! `createSession` intent and the object row; the client writes nothing.
//! The first accepted Submit materializes the journal, whose rows are the
//! durable witness of the same facts.
//!
//! Own test binary: the harness initializes process-global singletons (see
//! `tests/common/mod.rs` — one `#[gpui::test]` per binary), and this one
//! dials the REAL in-proc host, so the store's state root must be hermetic.
#![cfg(feature = "test-support")]

use agent_ui::Workspace;
use agent_ui::workspace::SubmitBehavior;
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
        // The object row is the deferred create's durable carrier: the host
        // resolves belonging and the config picks from it before any journal
        // exists. The app installs this at boot (`gallery::launch`); the
        // hermetic runtime alone does not.
        manox_agent::object_store::init();
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
                // Two models: the first is the registry default, so a create
                // that picks the second proves the intent's config travelled
                // (a dropped pick would silently run the default).
                models: vec![
                    manox_harness::provider_registry::ProviderModelConfig {
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
                    },
                    manox_harness::provider_registry::ProviderModelConfig {
                        id: "t-model-2".into(),
                        name: "T Model 2".into(),
                        reasoning: true,
                        input: vec![],
                        context_window: 8_192,
                        max_tokens: 1_024,
                        cost: Default::default(),
                        api: None,
                        base_url: None,
                        metadata: Default::default(),
                    },
                ],
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
    let project_string = project_path.to_string_lossy().into_owned();

    // The draft row's picks ARE the create intent's config: `start_new_thread`
    // refills an unspoken row under this project, and its row wins over
    // inheritance. Non-default picks (the second model, full access, max
    // effort) make a dropped intent visible in the journal and the chip.
    let draft_id = uuid::Uuid::new_v4().to_string();
    let objects = manox_agent::object_store::ObjectStore::try_global().expect("object store");
    objects.create_conversation(&draft_id, Some(project_string.as_str()));
    objects.set_model(&draft_id, Some("test-anthropic"), Some("t-model-2"));
    objects.set_access_mode(&draft_id, Some("full-access"));
    objects.set_reasoning_effort(&draft_id, Some("max"));

    let mut created = None;
    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| {
            created = Some(ws.start_new_thread(Some(project_path.clone()), window, cx));
        });
    });
    assert_eq!(
        created.as_deref(),
        Some(draft_id.as_str()),
        "the create refills the project's draft row"
    );
    let sid = created.expect("the create returns its session id");
    let journal = manox_agent::paths::core_session_file(Some(&project_path), &sid)
        .expect("the session journal path");

    // Lazy materialization: the create seeds the channel from the intent and
    // the object row, and no journal file exists until the first turn.
    assert!(
        !journal.exists(),
        "a deferred create must not write its journal before the first turn: {}",
        journal.display()
    );

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

    // The Captain strip's model identity: the PICKED non-default model,
    // wire-shaped (so `ModelTag` can resolve it) and resolving to a provider
    // + a wire api. A display name here resolves to neither — the provider
    // segment and the model's wire-api tint both vanish, which is the
    // regression this pins.
    let label = cx.update(|cx| ws.read(cx).diagnostic_model_label(cx));
    assert_eq!(
        label, "test-anthropic/t-model-2",
        "the strip's label must be the canonical wire id of the picked model"
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

    // The journal is the durable witness: the first accepted Submit
    // materializes the deferred session, and the create intent's facts land
    // as rows — no client-side channel writes needed.
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.gallery_submit("hello", SubmitBehavior::Send, cx);
        });
    });
    let mut rows: Vec<serde_json::Value> = Vec::new();
    for _ in 0..PARK_ROUNDS {
        cx.run_until_parked();
        if let Ok(text) = std::fs::read_to_string(&journal) {
            rows = text
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            let has = |kind: &str| {
                rows.iter()
                    .any(|row| row.get("type").and_then(|v| v.as_str()) == Some(kind))
            };
            if has("message")
                && has("project_change")
                && has("cwd_change")
                && has("model_change")
                && has("access_mode_change")
                && has("thinking_level_change")
            {
                break;
            }
        }
        std::thread::sleep(PARK_STEP);
    }
    let row = |kind: &str| {
        rows.iter()
            .find(|row| row.get("type").and_then(|v| v.as_str()) == Some(kind))
            .unwrap_or_else(|| panic!("the journal must carry a `{kind}` row: {rows:?}"))
    };
    assert_eq!(
        row("project_change").get("path").and_then(|v| v.as_str()),
        Some(project_string.as_str()),
        "the object row's project lands as the journal's belonging"
    );
    assert_eq!(
        row("cwd_change").get("cwd").and_then(|v| v.as_str()),
        Some(project_string.as_str()),
        "the create intent's working directory lands as the cwd witness"
    );
    assert_eq!(
        row("model_change").get("provider").and_then(|v| v.as_str()),
        Some("test-anthropic"),
        "the model pick lands its provider"
    );
    assert_eq!(
        row("model_change").get("modelId").and_then(|v| v.as_str()),
        Some("t-model-2"),
        "the model pick lands its id"
    );
    assert_eq!(
        row("access_mode_change")
            .get("mode")
            .and_then(|v| v.as_str()),
        Some("full-access"),
        "the access pick lands its mode"
    );
    assert_eq!(
        row("thinking_level_change")
            .get("thinkingLevel")
            .and_then(|v| v.as_str()),
        Some("max"),
        "the effort pick lands its level"
    );
    manox_agent::thread_store::drop_global_for_test();
    agent_ui::dispatch::clear_globals_for_test();
}
