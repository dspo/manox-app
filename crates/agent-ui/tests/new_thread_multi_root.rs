//! FULL-WIRING regression for multi-root projects (B4): a thread created FOR
//! a two-root project must carry the FULL root group on the create intent —
//! primary first (the session's cwd), the second root right behind it — so
//! every root gets tool authorization, not just the primary. The session
//! channel's fold (`SessionState.workingDirectories`) is the witness, the
//! journal's `cwd_change` row its durable counterpart, and the membership
//! write its grouping tail.
//!
//! Own test binary: the harness initializes process-global singletons (see
//! `tests/common/mod.rs` — one `#[gpui::test]` per binary) and dials the
//! REAL in-proc host, so the store's state root must be hermetic.
#![cfg(feature = "test-support")]

mod common;

use agent_ui::Workspace;
use agent_ui::launcher::LauncherTarget;
use gpui::{AppContext as _, TestAppContext, px, size};
use gpui_component::Root;
use steer_agent_chrome_ui::register_fonts;

/// The real-host create/attach round-trip completes on the tokio runtime and
/// only then wakes the gpui side; poll instead of a single `run_until_parked`.
const PARK_ROUNDS: usize = 100;
const PARK_STEP: std::time::Duration = std::time::Duration::from_millis(50);

#[gpui::test]
async fn a_two_root_project_grants_both_roots_on_the_create(cx: &mut TestAppContext) {
    // Hermetic HOME: the real host boots against a throwaway state root, not
    // the developer's ~/.manox.
    cx.update(|_cx| {
        manox_agent::runtime::init_hermetic_for_test();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init();
        manox_agent::object_store::init();
    });
    cx.background_executor.allow_parking();

    // A deterministic provider registry: the flush submit materializes the
    // journal only when the turn's model resolves.
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
    let ws_for_build = ws_cell.clone();
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

    for _ in 0..PARK_ROUNDS {
        cx.run_until_parked();
        if cx.update(|cx| ws.read(cx).diagnostic_ahp_connected(cx)) {
            break;
        }
        std::thread::sleep(PARK_STEP);
    }
    cx.run_until_parked();

    // A two-root project: the primary is real, the secondary too — the
    // registry canonicalizes at create time (macOS tempdirs gain the
    // /private prefix), so every comparison uses that form.
    let primary_dir = tempfile::tempdir().expect("primary dir");
    let secondary_dir = tempfile::tempdir().expect("secondary dir");
    let primary = primary_dir.path().canonicalize().expect("canonicalize");
    let secondary = secondary_dir.path().canonicalize().expect("canonicalize");
    let settings = tempfile::tempdir().expect("settings dir");
    let settings_file = settings.path().join("settings.toml");
    cx.update(|_cx| {
        agent_ui::projects::diagnostic_redirect_settings_file(Some(settings_file.clone()));
    });
    let project = cx.update(|_cx| {
        agent_ui::projects::create_project(
            Some("Multi".into()),
            vec![primary.clone(), secondary.clone()],
            Some(primary.clone()),
        )
        .expect("create the project entity")
    });

    let before = cx.update(|cx| ws.read(cx).diagnostic_foreground_session(cx));

    // The launcher presets BY ID (the group-menu shape — locked, the chip's
    // pick face is gone) with a non-default pick, then sends: mint + attach
    // + flush.
    cx.update_window(window.into(), |_, window, cx| {
        let ws = ws.clone();
        ws.update(cx, |ws, cx| {
            ws.enter_launcher(
                Some(LauncherTarget::Project(project.id.clone())),
                true,
                window,
                cx,
            );
            ws.diagnostic_launcher_picks(Some("test-anthropic/t-model"), false, None, cx);
        });
    })
    .expect("window live");
    common::launcher_send(cx, &ws, window.into(), "hello multi");

    let mut landed = None;
    for _ in 0..PARK_ROUNDS {
        cx.run_until_parked();
        let now = cx.update(|cx| ws.read(cx).diagnostic_foreground_session(cx));
        if now.is_some() && now != before {
            landed = now;
            break;
        }
        std::thread::sleep(PARK_STEP);
    }
    let sid = landed.expect("the new thread must bind a foreground session");

    // The session channel's fold carries the FULL grant: both roots, primary
    // first (it is the session's cwd). Poll — the seed arrives over the
    // attach's subscribe.
    let mut granted = None;
    for _ in 0..PARK_ROUNDS {
        cx.run_until_parked();
        granted = cx.update(|cx| {
            ws.read(cx)
                .diagnostic_chat_store(cx)
                .and_then(|(store, bound)| {
                    store.read(cx).book.sessions.get(&bound).and_then(|state| {
                        state
                            .working_directories
                            .clone()
                            .map(|dirs| (bound.clone(), dirs))
                    })
                })
        });
        if granted.is_some() {
            break;
        }
        std::thread::sleep(PARK_STEP);
    }
    let (bound_sid, dirs) = granted.expect("the session fold must carry workingDirectories");
    assert_eq!(bound_sid, sid, "the folded session is the foreground one");
    let want = vec![
        format!("file://{}", primary.display()),
        format!("file://{}", secondary.display()),
    ];
    assert_eq!(
        dirs, want,
        "the create intent must grant the project's FULL root group, primary first"
    );

    // The cwd witness on the channel: the primary, not a fallback.
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
        primary.to_str(),
        "the primary root binds as the session's cwd"
    );

    // The grouping tail: the membership lands with the attach.
    assert_eq!(
        cx.update(|_cx| agent_ui::projects::membership_of(&sid))
            .as_deref(),
        Some(project.id.as_str()),
        "the launcher mint must attach the container to the project"
    );

    // The journal's durable counterpart: the belonging and the cwd rows.
    let journal = manox_agent::paths::core_session_file(Some(&primary), &sid)
        .expect("the session journal path");
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
            if has("message") && has("project_change") && has("cwd_change") {
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
        row("cwd_change").get("cwd").and_then(|v| v.as_str()),
        Some(primary.to_str().expect("utf8 primary")),
        "the primary root lands as the journal's cwd witness"
    );

    cx.update(|_cx| {
        agent_ui::projects::diagnostic_redirect_settings_file(None);
    });
    manox_agent::thread_store::drop_global_for_test();
    agent_ui::dispatch::clear_globals_for_test();
}
