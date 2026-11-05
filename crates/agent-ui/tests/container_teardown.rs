//! FULL-WIRING regression for the container teardown (plan §3.2 A2): a
//! container's death reclaims everything under its registry key, the close
//! gesture kills the child process EXPLICITLY (the orphaned-cx-child fix),
//! the external death drops the record (the sidebar row with it), and
//! `/exit`'s browser leg is container-scoped — other containers' tabs
//! survive. The dock/right stash is keyed per container through the REAL
//! assembly observer.
//!
//! Own test binary: the harness initializes process-global singletons (see
//! `tests/common/mod.rs` — one `#[gpui::test]` per binary).
#![cfg(feature = "test-support")]

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use agent_ui::workspace::external_sessions::ExternalSessionLaunch;
use gpui::{AppContext as _, TestAppContext};
use steer_ext_agents::SessionHandle;

/// A REAL child (`/bin/cat` — blocks forever with no input) behind a session
/// handle, so an explicit kill is observable: `wait` returns only once the
/// child actually died. The socket lives in a tempdir so the test leaves no
/// state in the user's session directory.
fn spawn_cat() -> (tempfile::TempDir, Arc<SessionHandle>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("cat.sock");
    let spec = steer_ext_agents::LaunchSpec {
        program: PathBuf::from("/bin/cat"),
        args: vec![],
        env: BTreeMap::new(),
        summary: String::new(),
        detach: false,
        env_remove: vec![],
        agent_id: "test".into(),
        provider_name: "test".into(),
        model_id: None,
        pty: true,
        socket: Some(socket.to_string_lossy().into_owned()),
        cwd: None,
    };
    let handle = Arc::new(SessionHandle::spawn(&spec, None).expect("spawn /bin/cat"));
    (dir, handle)
}

/// Poll until the child behind `handle` is dead — `wait` reaps it and
/// reports a signal termination only when OUR kill ended it (an un-killed
/// cat would block the channel forever).
fn wait_child_dead(cx: &mut TestAppContext, handle: Arc<SessionHandle>) -> bool {
    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    std::thread::spawn(move || {
        let killed = handle
            .wait()
            .ok()
            .is_some_and(|result| result.termination.is_some());
        let _ = tx.send(killed);
    });
    for _ in 0..250 {
        cx.run_until_parked();
        if let Ok(killed) = rx.try_recv() {
            return killed;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    false
}

/// Spawn an external container the way the project menu does (the record
/// lands through the unified container entry) and return its row id together
/// with the PTY's output side (the death signal's driver).
fn spawn_external(
    cx: &mut TestAppContext,
    ws: &gpui::Entity<agent_ui::Workspace>,
    window: gpui::AnyWindowHandle,
    agent_id: &'static str,
    handle: Option<Arc<SessionHandle>>,
) -> (String, common::TerminalFixture) {
    let fixture = common::ghostty_fixture(cx);
    let ws = ws.clone();
    cx.update_window(window, |_, window, cx| {
        ws.update(cx, |ws, cx| {
            ws.spawn_external_session(
                ExternalSessionLaunch {
                    agent_id,
                    cx_session_id: String::new(),
                    label: "Claude Code".into(),
                    svg: "icons/claude.svg",
                    cwd: std::path::PathBuf::from("/tmp"),
                    handle,
                },
                fixture.view.clone(),
                window,
                cx,
            );
        });
    })
    .expect("spawn the external container");
    cx.run_until_parked();
    let row = ws.read_with(cx, |ws, _| {
        ws.diagnostic_external_session_ids()
            .into_iter()
            .find(|id| id.starts_with(&format!("external:{agent_id}:")))
            .expect("the external container registered")
    });
    (row, fixture)
}

/// Poll until `predicate` holds (the teardown beats run across real pumps
/// and a deferred App callback, so every arrival is polled, not assumed).
/// `cx` passes INTO the predicate — the reads need the context.
fn wait_until(
    cx: &mut TestAppContext,
    what: &str,
    mut predicate: impl FnMut(&mut TestAppContext) -> bool,
) {
    for _ in 0..250 {
        cx.run_until_parked();
        if predicate(cx) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("never settled: {what}");
}

#[gpui::test]
async fn container_teardown_reclaims_its_resources(cx: &mut TestAppContext) {
    let (ws, window, shell) = common::full_wiring_boot_shell(cx);

    // ── closing an external row kills the child explicitly ──────────────
    let (_dir, cat) = spawn_cat();
    let (row, _row_probe) = spawn_external(cx, &ws, window, "claude", Some(cat.clone()));
    cx.update(|cx| ws.update(cx, |ws, cx| ws.close_external_session(&row, cx)));
    wait_until(cx, "the closed external row dropped", |cx| {
        !ws.read_with(cx, |ws, _| ws.is_external_session(&row))
    });
    assert!(
        wait_child_dead(cx, cat),
        "closing the external row killed the child (no orphaned cx process)"
    );

    // ── an external death drops the row and reclaims the resources ──────
    let (row2, row2_probe) = spawn_external(cx, &ws, window, "codex", None);
    // Plant the resources the container would hold: a CLI tab's kill handle
    // (a REAL child, so the reclaim is observable) and a browser route.
    let (_dir2, cat2) = spawn_cat();
    ws.update(cx, |ws, _| {
        ws.diagnostic_register_kill_handle(&row2, "codex-tab-1", cat2.clone());
    });
    // The pane registry's durable snapshot (A12): one under the dying
    // container's key, one under a foreign key — the teardown removes only
    // the dead container's.
    let pane_payload = r#"{"visible":true,"active":0,"tabs":[]}"#.to_string();
    let foreign_pane_key = "external:codex:pane-registry-survivor";
    agent_ui::chrome_assembly::diagnostic_persist_right_pane(&row2, pane_payload.clone());
    agent_ui::chrome_assembly::diagnostic_persist_right_pane(foreign_pane_key, pane_payload);
    assert!(
        agent_ui::chrome_assembly::diagnostic_right_pane_snapshot(&row2).is_some(),
        "the snapshot is planted"
    );
    // The browser-route legs assert only where the integrated browser
    // compiles (Linux/FreeBSD have no browser host — nothing can register).
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    let route2 = ws.update(cx, |ws, cx| ws.diagnostic_register_browser_route(&row2, cx));
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    assert_eq!(
        ws.read_with(cx, |ws, _| ws.diagnostic_container_browser_tabs(&row2))
            .len(),
        1,
        "the route is owned by the external container"
    );
    // The membership: the container was minted into a project (the
    // launcher's gestures write it at the same seam) — the death must prune
    // it, an external container has no revival to keep the fact for. The
    // redirect keeps the registry writes hermetic (the boot's projection
    // reads already settled; the redirect drops the cache onto the test
    // file before the first write).
    let settings_dir = tempfile::tempdir().expect("settings dir");
    let settings_file = settings_dir.path().join("settings.toml");
    let project_root = tempfile::tempdir().expect("project root");
    agent_ui::projects::diagnostic_redirect_settings_file(Some(settings_file.clone()));
    let project = agent_ui::projects::create_project(
        Some("Teardown".into()),
        vec![project_root.path().to_path_buf()],
        None,
    )
    .expect("create the project");
    agent_ui::projects::set_membership(&row2, &project.id);
    // A foreign container's membership (the keyed-prune witness — it names
    // no container that dies here).
    let foreign_member_key = "external:codex:membership-survivor";
    agent_ui::projects::set_membership(&foreign_member_key, &project.id);
    assert_eq!(
        agent_ui::projects::membership_of(&row2).as_deref(),
        Some(project.id.as_str()),
        "the membership is planted"
    );
    // The 0 号 exits: the death predicate flips and the workspace's own
    // subscription runs the ONE teardown.
    row2_probe.probe.exit(Some(0));
    wait_until(cx, "the dead external's row dropped", |cx| {
        !ws.read_with(cx, |ws, _| ws.is_external_session(&row2))
    });
    assert!(
        !ws.read_with(cx, |ws, _| ws.external_session_foreground()),
        "the dead container was foreground — the main column falls back to the conversation"
    );
    assert!(
        agent_ui::projects::membership_of(&row2).is_none(),
        "the death pruned the container's membership"
    );
    assert_eq!(
        agent_ui::projects::membership_of(foreign_member_key).as_deref(),
        Some(project.id.as_str()),
        "the prune is keyed — a foreign container's membership survives"
    );
    agent_ui::projects::diagnostic_redirect_settings_file(None);
    assert!(
        wait_child_dead(cx, cat2),
        "the death reclaimed the container's CLI-tab child"
    );
    wait_until(
        cx,
        "the death reclaimed the dead container's pane snapshot",
        |_cx| agent_ui::chrome_assembly::diagnostic_right_pane_snapshot(&row2).is_none(),
    );
    assert!(
        agent_ui::chrome_assembly::diagnostic_right_pane_snapshot(foreign_pane_key).is_some(),
        "the removal is keyed — a foreign container's snapshot survives"
    );
    agent_ui::chrome_assembly::remove_right_pane(foreign_pane_key);
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    wait_until(
        cx,
        "the death reclaimed the container's browser route",
        |cx| {
            ws.read_with(cx, |ws, _| {
                ws.diagnostic_container_browser_tabs(&row2).is_empty()
            })
        },
    );
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    let _ = route2;

    // ── the /exit browser leg is container-scoped ───────────────────────
    // A FOREIGN container's browser route must survive the /exit — the
    // reclaim is keyed, not a global nuke. (The outgoing side shares its
    // reclaim face with the death path asserted above; this section runs
    // BEFORE the dock section because a fake terminal view parked in the
    // shell is a test-only construct whose presence across the waterfall's
    // simulated-timer beats wedges the hermetic host — the sections stay
    // independent.) The browser asserts ride the integrated browser's
    // platforms; the /exit waterfall itself runs everywhere below.
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    {
        let foreign = "external:claude:00000000-0000-0000-0000-000000000000";
        let foreign_route = ws.update(cx, |ws, cx| {
            ws.diagnostic_register_browser_route(foreign, cx)
        });
        common::run_slash(cx, &ws, window, "/exit");
        assert!(common::wait_launcher(cx, &ws), "/exit lands the launcher");
        assert!(
            ws.read_with(cx, |ws, _| ws
                .diagnostic_container_browser_tabs(foreign)
                .contains(&foreign_route)),
            "/exit must not nuke other containers' browser tabs"
        );
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        common::run_slash(cx, &ws, window, "/exit");
        assert!(common::wait_launcher(cx, &ws), "/exit lands the launcher");
    }

    // ── the dock/right stash is keyed per container ─────────────────────
    let fake_dock_view =
        |cx: &mut TestAppContext| gpui::AnyView::from(common::ghostty_fixture(cx).view);
    let attach = |cx: &mut TestAppContext,
                  id: &str,
                  ws: &gpui::Entity<agent_ui::Workspace>,
                  window: gpui::AnyWindowHandle| {
        let ws = ws.clone();
        let id = id.to_string();
        cx.update_window(window, |_, window, cx| {
            ws.update(cx, |ws, cx| {
                ws.diagnostic_attach_thread(common::landing_thread(&id), true, window, cx);
            });
        })
        .expect("attach lands on the live window");
        cx.run_until_parked();
    };
    attach(cx, "stash-a", &ws, window);
    attach(cx, "stash-b", &ws, window);
    let owners = ws.read_with(cx, |ws, _| ws.diagnostic_live_owners());
    assert_eq!(
        owners,
        (Some("stash-b".into()), Some("stash-b".into())),
        "the live dock/right owner slots follow the foreground container"
    );
    let dock_view = fake_dock_view(cx);
    shell.update(cx, |shell, cx| shell.set_panel_view(dock_view, cx));
    cx.run_until_parked();
    // Switching away parks the dock view under stash-b; coming back
    // restores it — the per-container stash round trip through the REAL
    // observer.
    attach(cx, "stash-a", &ws, window);
    wait_until(cx, "the dock view parked under stash-b", |cx| {
        ws.read_with(cx, |ws, _| {
            ws.diagnostic_dock_stash()
                .iter()
                .any(|(container, has_view)| container == "stash-b" && *has_view)
        })
    });
    attach(cx, "stash-b", &ws, window);
    wait_until(cx, "the dock view restored for stash-b", |cx| {
        ws.read_with(cx, |ws, _| {
            // The restore TAKES the view (the emptied entry stays — only a
            // teardown reclaims the key), so the assertion reads the flag.
            !ws.diagnostic_dock_stash()
                .iter()
                .any(|(container, has_view)| container == "stash-b" && *has_view)
        })
    });

    common::full_wiring_teardown(cx);
}
