//! FULL-WIRING regression: a thread created FOR a project (the sidebar's
//! 新建会话 → Steer row hands `start_new_thread` the project directory) must
//! carry that directory AND its inherited model/access/effort picks all the
//! way to the host's durable journal — the user never picks the folder a
//! second time.
//!
//! The contract is the terminal one: the create intent carries the working
//! directories and the config, the host adopts both at create time (their
//! journal rows), and belonging resolves from the conversation's object row.
//! There is NO post-create client write — the retired compensation (AGENTS.md
//! ledger #904/#905) is gone, so the channel stays untouched until the host
//! materializes the session.
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
    // the developer's ~/.manox. The object store lands the conversation row
    // the host resolves the session's project from.
    cx.update(|_cx| {
        manox_agent::runtime::init_hermetic_for_test();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init();
        manox_agent::object_store::init();
    });
    cx.background_executor.allow_parking();

    // A deterministic provider registry. Two models so the inherited pick
    // differs from the registry default and therefore has to land a journal
    // row (the config choke point only writes real changes).
    let registry = std::sync::Arc::new(manox_harness::ProviderRegistry::new());
    let model = |id: &str, name: &str, reasoning: bool| {
        manox_harness::provider_registry::ProviderModelConfig {
            id: id.into(),
            name: name.into(),
            reasoning,
            input: vec![],
            context_window: 8_192,
            max_tokens: 1_024,
            cost: Default::default(),
            api: None,
            base_url: None,
            metadata: Default::default(),
        }
    };
    registry
        .register_provider(
            "test-anthropic",
            manox_harness::ProviderConfig {
                name: Some("Test".into()),
                base_url: Some("https://example.invalid".into()),
                api: Some(manox_harness::provider_registry::Api::AnthropicMessages),
                models: vec![
                    model("t-model", "T Model", false),
                    model("t-model-2", "T Model 2", true),
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

    // Seat the picks the composer's own pickers write (model, access mode,
    // reasoning effort) on the foreground. The new thread inherits them into
    // its create intent; the assertions below read them back from the host's
    // side, never from a post-create client write.
    let mut picks = serde_json::Map::new();
    picks.insert(
        "model".into(),
        serde_json::json!("test-anthropic/t-model-2"),
    );
    picks.insert("accessMode".into(), serde_json::json!("full-access"));
    picks.insert("reasoningEffort".into(), serde_json::json!("max"));
    cx.update(|cx| {
        ws.update(cx, |ws, cx| ws.diagnostic_seat_foreground_config(picks, cx));
    });

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
    let sid = landed.expect("the new thread must bind a foreground session");

    // The object row is the belonging authority: it carries the create
    // intent's project and picks, and the host resolves the session's project
    // from it (no project field on the create wire).
    let objects = manox_agent::object_store::ObjectStore::try_global().expect("object store");
    let row = objects
        .conversation(&sid)
        .expect("the create path must land the conversation's object row");
    let row_project = row
        .project_id
        .as_deref()
        .and_then(|pid| objects.project_path(pid))
        .expect("the object row must carry the project the thread was created for");
    assert_eq!(
        std::path::Path::new(&row_project),
        project_path,
        "the object row must bind the project directory the thread was created for"
    );
    assert_eq!(
        (row.model_provider.as_deref(), row.model_id.as_deref()),
        (Some("test-anthropic"), Some("t-model-2")),
        "the object row must carry the inherited model pick"
    );
    assert_eq!(
        row.access_mode.as_deref(),
        Some("full-access"),
        "the object row must carry the inherited access mode"
    );
    assert_eq!(
        row.reasoning_effort.as_deref(),
        Some("max"),
        "the object row must carry the inherited reasoning effort"
    );

    // The retired compensation wrote the channel's cwd right after the
    // attach. It is gone: a fresh session's journal is deferred until the
    // first accepted Submit, and the host seeds cwd only from a materialized
    // journal, so a cwd here can only be a client write.
    for _ in 0..20 {
        cx.run_until_parked();
        std::thread::sleep(PARK_STEP);
    }
    assert_eq!(
        cx.update(|cx| ws.read(cx).diagnostic_foreground_cwd(cx)),
        None,
        "the client must not patch the channel's cwd after create"
    );
    // The config seats may race the host's own create-time fold (the engine
    // journals the picks concurrently with the subscription). Both outcomes
    // are host-owned: never a client value other than the intent's picks.
    let config_seat = |value: Option<String>, expected: &str| {
        assert!(
            value.is_none() || value.as_deref() == Some(expected),
            "a post-create client write must not seat a value the create intent never carried: {value:?}"
        );
    };
    config_seat(
        cx.update(|cx| ws.read(cx).diagnostic_foreground_model(cx)),
        "test-anthropic/t-model-2",
    );
    config_seat(
        cx.update(|cx| ws.read(cx).diagnostic_foreground_access(cx)),
        "full-access",
    );
    config_seat(
        cx.update(|cx| ws.read(cx).diagnostic_foreground_effort(cx)),
        "max",
    );

    // The host's lazy-materialization contract: the create facts buffer on
    // the deferred journal and land on disk with the first accepted Submit.
    // That Submit is the only way to read back what the create intent
    // carried.
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.gallery_submit("probe", SubmitBehavior::Send, cx)
        });
    });

    // Wait for the journal to exist, then assert the create-time rows the
    // host wrote from the intent: project belonging, cwd grant, and all three
    // config picks.
    let mut rows = Vec::new();
    for _ in 0..PARK_ROUNDS {
        cx.run_until_parked();
        if let Some(path) = manox_agent::thread_store::try_global()
            .and_then(|store| store.read(|state| state.journal_path(&sid)))
            && let Ok(text) = std::fs::read_to_string(&path)
        {
            rows = text
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .collect();
            if rows.iter().any(|row| {
                row.get("type").and_then(|v| v.as_str()) == Some("thinking_level_change")
            }) {
                break;
            }
        }
        std::thread::sleep(PARK_STEP);
    }
    let has_row = |kind: &str| {
        rows.iter()
            .find(|row| row.get("type").and_then(|v| v.as_str()) == Some(kind))
            .unwrap_or_else(|| panic!("the journal must carry the create's `{kind}` row: {rows:?}"))
    };

    assert_eq!(
        has_row("project_change")
            .get("path")
            .and_then(|v| v.as_str()),
        project_path.to_str(),
        "the create intent's project must land the journal's belonging row"
    );
    assert_eq!(
        has_row("cwd_change").get("cwd").and_then(|v| v.as_str()),
        project_path.to_str(),
        "the create intent's working directory must land the journal's cwd row"
    );
    let model_row = has_row("model_change");
    assert_eq!(
        (
            model_row.get("provider").and_then(|v| v.as_str()),
            model_row.get("modelId").and_then(|v| v.as_str()),
        ),
        (Some("test-anthropic"), Some("t-model-2")),
        "the create intent's model must land the journal's model_change row"
    );
    assert_eq!(
        has_row("access_mode_change")
            .get("mode")
            .and_then(|v| v.as_str()),
        Some("full-access"),
        "the create intent's access mode must land the journal's access_mode_change row"
    );
    assert_eq!(
        has_row("thinking_level_change")
            .get("thinkingLevel")
            .and_then(|v| v.as_str()),
        Some("max"),
        "the create intent's reasoning effort must land the journal's thinking_level_change row"
    );

    // The channel's cwd folds from the host once the journal materializes
    // (the header/seed path); the project chip's belonging comes from the
    // object row asserted above.
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

    manox_agent::thread_store::drop_global_for_test();
    agent_ui::dispatch::clear_globals_for_test();
}
