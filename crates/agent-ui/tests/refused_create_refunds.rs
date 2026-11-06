//! FULL-WIRING regression for the refund leg (design §6): a create the host
//! refuses must not swallow the launcher send. The refusal here is forced
//! with a pick the hermetic registry cannot resolve — the in-proc host
//! answers `model/unresolvable` and creates nothing. The pins:
//!
//! - the launcher re-opens with the text back in the composer (never
//!   swallowed, never stranded in a hidden draft);
//! - the foreground never moves (the refused id attaches nothing);
//! - no membership key is left behind — the grouping fact writes at the
//!   attach tail, which a refused create never reaches.
//!
//! Own test binary: the harness initializes process-global singletons (see
//! `tests/common/mod.rs` — one `#[gpui::test]` per binary) and dials the
//! REAL in-proc host.
#![cfg(feature = "test-support")]

mod common;

use agent_ui::Workspace;
use agent_ui::launcher::LauncherTarget;
use gpui::{AppContext as _, TestAppContext};

#[gpui::test]
async fn a_refused_create_reopens_the_launcher_with_the_text_back(cx: &mut TestAppContext) {
    let (ws, window) = common::full_wiring_boot(cx);

    // A hermetic settings file: the project entity and its membership table
    // land here, and the file's contents are the refusal's witness.
    let settings = tempfile::tempdir().expect("settings dir");
    let settings_file = settings.path().join("settings.toml");
    cx.update(|_cx| {
        agent_ui::projects::diagnostic_redirect_settings_file(Some(settings_file.clone()));
    });
    let project = cx.update(|_cx| {
        agent_ui::projects::create_project(Some("P".into()), vec![std::env::temp_dir()], None)
            .expect("create the project entity")
    });

    let before = common::foreground_session(cx, &ws);

    // The launcher targets the project; the pick names a provider the
    // hermetic registry never carried — the create refuses on it.
    common::enter_launcher(
        cx,
        &ws,
        window,
        Some(LauncherTarget::Project(project.id.clone())),
    );
    cx.update_window(window, |_, _window, cx| {
        ws.update(cx, |ws, cx| {
            ws.diagnostic_launcher_picks(Some("no-such-provider/no-such-model"), false, None, cx);
        });
    })
    .expect("window live");

    common::launcher_send(cx, &ws, window, "hello again");

    // The refund round-trips through the host's refusal on the tokio
    // runtime: poll until the composer carries the text again.
    let mut refunded = false;
    for _ in 0..common::PARK_ROUNDS {
        cx.run_until_parked();
        if cx.update(|cx| ws.read(cx).diagnostic_launcher_text(cx)) == "hello again" {
            refunded = true;
            break;
        }
        std::thread::sleep(common::PARK_STEP);
    }
    assert!(
        refunded,
        "a refused create must refund its text to the launcher composer"
    );
    assert!(
        cx.update(|cx| ws.read(cx).diagnostic_view_mode_is_launcher()),
        "the launcher stays the main column after a refusal"
    );
    assert_eq!(
        common::foreground_session(cx, &ws),
        before,
        "a refused create attaches nothing"
    );

    // The durable witness: no membership key may point at the id that never
    // became a session. The remembered default target is allowed (the chip's
    // target was an explicit pick at the mint).
    let raw = std::fs::read_to_string(&settings_file).expect("settings file");
    let doc: toml::Value = toml::from_str(&raw).expect("parse");
    let membership = doc
        .get("project_membership")
        .and_then(|v| v.as_table())
        .map(|table| table.len())
        .unwrap_or(0);
    assert_eq!(
        membership, 0,
        "a refused create must leave no orphan membership key: {doc}"
    );

    cx.update(|_cx| {
        agent_ui::projects::diagnostic_redirect_settings_file(None);
    });
    common::full_wiring_teardown(cx);
}
