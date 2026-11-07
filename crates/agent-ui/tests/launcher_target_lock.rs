//! Regression for the creation-time target fix (plan A1, decisions D2/D3):
//! the project-item entries' preset LOCKS the launcher page (the lock bit is
//! what withholds the chrome's chip pick handler — display-only), and a bare
//! ⌘N entry lands the unlocked NO-PROJECT default. The target never carries
//! across conversations: leaving the launcher and re-entering bare resets
//! both the target and a leftover lock.
//!
//! Light harness (no real AHP host): the assertions are state-machine reads
//! on the workspace's launcher entity; nothing is minted.
//!
//! Own test binary: the harness initializes process-global singletons (see
//! `tests/common/mod.rs` — one `#[gpui::test]` per binary).
#![cfg(feature = "test-support")]

mod common;

use agent_ui::launcher::LauncherTarget;
use gpui::{AppContext as _, TestAppContext};

#[gpui::test]
fn the_preset_locks_and_the_bare_entry_lands_no_project(cx: &mut TestAppContext) {
    common::init_harness(cx);
    let (window, ws) = common::open_workspace(cx);
    let window: gpui::AnyWindowHandle = window.into();

    // A project entity for the preset; the hermetic settings file keeps the
    // registry off the developer's ~/.manox.
    let settings = tempfile::tempdir().expect("settings dir");
    let settings_file = settings.path().join("settings.toml");
    cx.update(|_cx| {
        agent_ui::projects::diagnostic_redirect_settings_file(Some(settings_file.clone()));
    });
    let project = cx.update(|_cx| {
        agent_ui::projects::create_project(Some("P".into()), vec![std::env::temp_dir()], None)
            .expect("create the project entity")
    });

    // Bare ⌘N from a non-launcher view: the unlocked no-project default.
    cx.update_window(window, |_, window, cx| {
        ws.update(cx, |ws, cx| ws.enter_launcher(None, false, window, cx));
    })
    .expect("window live");
    let (target, locked) = cx
        .update(|cx| ws.read(cx).diagnostic_launcher_target(cx))
        .expect("the launcher entity exists from boot");
    assert_eq!(
        target,
        LauncherTarget::None,
        "the bare default is no project"
    );
    assert!(!locked, "the bare entry never locks");

    // The project-item shape (the group header's ＋ / the group menu's 新建
    // 对话): the preset lands verbatim AND locks — the chip is display-only.
    cx.update_window(window, |_, window, cx| {
        ws.update(cx, |ws, cx| {
            ws.enter_launcher(
                Some(LauncherTarget::Project(project.id.clone())),
                true,
                window,
                cx,
            );
        });
    })
    .expect("window live");
    let (target, locked) = cx
        .update(|cx| ws.read(cx).diagnostic_launcher_target(cx))
        .expect("the launcher entity exists");
    assert_eq!(
        target,
        LauncherTarget::Project(project.id.clone()),
        "the preset target lands verbatim"
    );
    assert!(locked, "the project-item entry locks the page");

    // Leaving the launcher and re-entering bare resets both: no carried
    // target, no leftover lock.
    cx.update_window(window, |_, _, cx| {
        ws.update(cx, |ws, cx| ws.focus_conversation(cx));
    })
    .expect("window live");
    cx.update_window(window, |_, window, cx| {
        ws.update(cx, |ws, cx| ws.enter_launcher(None, false, window, cx));
    })
    .expect("window live");
    let (target, locked) = cx
        .update(|cx| ws.read(cx).diagnostic_launcher_target(cx))
        .expect("the launcher entity exists");
    assert_eq!(
        target,
        LauncherTarget::None,
        "the target never carries across conversations"
    );
    assert!(!locked, "the lock dies with its entry gesture");

    cx.update(|_cx| {
        agent_ui::projects::diagnostic_redirect_settings_file(None);
    });
}
