//! The canvas surface: the container's layout tree paints its tab
//! strips and places the session sheet's body, and a tab close goes through the
//! canvas host's single write path.
//!
//! Own test binary: shared harness singletons (see `tests/common/mod.rs`).

#![cfg(feature = "test-support")]

mod common;

use agent_ui::ahp_store::AhpStore;
use common::{init_harness, open_workspace};
use gpui::{AppContext as _, TestAppContext, VisualTestContext};

/// Draws a frame (and drains the entities it notified).
fn draw(visual: &mut VisualTestContext) {
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    visual.cx.run_until_parked();
}

/// A tab close button's debug selector. `debug_bounds` wants `&'static str`;
/// the leak is bounded by the test's own tab count.
fn close_selector(sheet: &str) -> &'static str {
    format!("canvas-tab-close-{sheet}").leak()
}

/// The archive receipt's echo, as the fold carries it: the session channel
/// flips the container's session to archived. The detached harness has no host
/// to send one, so the test writes the same state the echo lands — this is
/// what lets the workspace's death predicate see the archive.
fn echo_archive(store: &mut AhpStore, container: &str) {
    let archived: ahp_types::state::SessionState = serde_json::from_value(serde_json::json!({
        "provider": "pi",
        "title": "",
        "status": ahp_types::state::SessionStatus::IsArchived.bits(),
        "lifecycle": "ready",
        "activeClients": [],
        "chats": [],
    }))
    .expect("the archived session state parses");
    store.book.sessions.insert(container.to_string(), archived);
}

#[gpui::test]
async fn the_canvas_paints_its_strip_and_the_session_body(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    for _ in 0..3 {
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }

    assert!(
        visual.debug_bounds("canvas-open-home").is_some()
            || visual.debug_bounds("workspace-message-list").is_some(),
        "the canvas chrome must paint"
    );
    let sheets = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx));
    assert!(
        !sheets.is_empty(),
        "opening the canvas seeds at least one sheet"
    );
    assert!(
        visual.debug_bounds("workspace-message-list").is_some()
            || visual.debug_bounds("canvas-open-home").is_some(),
        "the canvas paints its chrome"
    );
}

#[gpui::test]
async fn closing_a_tab_goes_through_the_one_writer(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    for _ in 0..2 {
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }
    let before = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx));
    assert!(!before.is_empty());

    // The close gesture is the host's; the renderer only fires it. Dropping the
    // last sheet must ask for the archive through the workspace (the canvas
    // never tears an entity down itself).
    workspace.update(&mut visual.cx, |ws, cx| {
        for sheet in before {
            ws.diagnostic_canvas_close(&sheet, cx);
        }
    });
    let after = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx));
    assert!(after.is_empty(), "the last close empties the layout");
}

/// The real close path: every tab's × is a click on `canvas-tab-close-{sheet}`,
/// and the click listener runs inside the canvas's own lease. Closing the last
/// sheet asks the workspace for the archive — that request must cross the
/// entity boundary on the event face, never a synchronous `workspace.update`
/// (the workspace's handler re-enters this canvas, which would double-lease
/// it). The container with no live dependents archives directly: its canvas
/// is forgotten (nothing of it stays in the layout or the parked set) and the
/// shell lands on the scratch start page.
#[gpui::test]
async fn closing_the_last_tab_by_click_archives_the_container(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let container = "close-e2e";
    // The offscreen harness has no host: a detached store bound as the
    // foreground gives the canvas a real container (its session id).
    let store = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store.clone(), container, cx);
    });
    draw(&mut visual);

    let sheets = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx));
    assert!(
        sheets.contains(&"home:close-e2e".to_string())
            && sheets.contains(&"session:close-e2e".to_string()),
        "the container opens with its start page and session, got {sheets:?}"
    );

    // A real click on every tab's close button. Each click redraws first (the
    // user sees the strip move before the next click), so every click lands on
    // the live strip instead of a stale frame.
    for sheet in &sheets {
        draw(&mut visual);
        let close = visual
            .debug_bounds(close_selector(sheet))
            .unwrap_or_else(|| panic!("the close button of {sheet} paints"));
        visual.simulate_click(close.center(), gpui::Modifiers::default());
    }
    visual.cx.run_until_parked();

    // The receipt echo lands (the detached harness has no host to send one)
    // and the next frame settles: the dead foreground owns no canvas, so the
    // archived container never pops back over the scratch surface.
    store.update(&mut visual.cx, |store, _| echo_archive(store, container));
    draw(&mut visual);
    visual.cx.run_until_parked();

    let after = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx));
    assert!(
        after.iter().all(|sheet| !sheet.contains(container)),
        "the archived container's canvas is forgotten, got {after:?}"
    );
    assert_eq!(
        after,
        vec!["home:scratch".to_string()],
        "the shell rests on the scratch start page"
    );
    // The pending save the last close scheduled (the start page reopen) is
    // retired with the archive: no layout file haunts the container's
    // revival (the harness writes under the real state root).
    assert_eq!(
        steer_canvas::persist::read_canvas(container),
        None,
        "the archive leaves no layout file behind"
    );
    if let Some(path) = steer_canvas::persist::layout_path(container) {
        let _ = std::fs::remove_file(path);
    }
}

/// The archive gesture's canvas landing: archiving a container that holds a
/// terminal sheet (through THE archive path the sidebar lands on) takes the
/// sheet out of the shown layout AND the parked set, drops the PTY view, and
/// falls back to the scratch surface. Without the landing, the union judge
/// would keep the PTY alive behind a container that no longer exists.
#[gpui::test]
async fn archiving_a_container_reclaims_its_terminal_sheet(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let container = "term-arch";
    let store = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store.clone(), container, cx);
    });
    draw(&mut visual);
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_canvas_open_terminal(cx)
    });
    draw(&mut visual);

    let terminal_sheet = workspace
        .read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx))
        .into_iter()
        .find(|sheet| sheet.starts_with("terminal:"))
        .expect("the terminal sheet opens");
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_terminal_views(cx)),
        vec![terminal_sheet.clone()],
        "the PTY view is the sheet's resource"
    );

    // The sidebar path's landing: the ONE archive entry, direct arm (no live
    // dependents in this harness). Then the receipt echo lands and the next
    // frame settles — the dead foreground must not re-open the canvas.
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_archive_containers(vec![container.to_string()], cx);
    });
    store.update(&mut visual.cx, |store, _| echo_archive(store, container));
    draw(&mut visual);
    visual.cx.run_until_parked();

    let held = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_held_sheet_ids(cx));
    assert!(
        held.iter().all(|sheet| !sheet.contains(container)),
        "the archived container's sheets leave the layout and the parked set, got {held:?}"
    );
    let views = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_terminal_views(cx));
    assert!(
        views.is_empty(),
        "the terminal sheet's PTY view is dropped, got {views:?}"
    );
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws
            .diagnostic_canvas_active_container(cx)),
        None,
        "the archived container owned the shown canvas; the shell falls back to scratch"
    );
    if let Some(path) = steer_canvas::persist::layout_path(container) {
        let _ = std::fs::remove_file(path);
    }
}

/// The refused archive (live dependents gate the teardown behind a confirm
/// dialog): the container stays, and the canvas it was closed on shows its
/// start page — the state the archive request put it in — never an empty
/// canvas.
#[gpui::test]
async fn a_refused_archive_leaves_the_canvas_on_its_start_page(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let container = "refused-arch";
    let store = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store, container, cx);
    });
    draw(&mut visual);

    // One live dependent on the container's roster: the confirm gate refuses
    // the direct archive and opens the dialog instead.
    let mut runs = std::collections::BTreeMap::new();
    runs.insert(
        "run-live".to_string(),
        serde_json::json!({
            "runId": "run-live",
            "kind": "Explore",
            "label": "map the edges",
            "status": "running",
            "startedAt": "2026-10-30T00:00:00.000Z",
            "chat": "ahp-chat:/run-live",
        }),
    );
    let mux = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_mux_store(cx));
    mux.update(&mut visual.cx, |store, _| {
        store.book.ext.insert(
            steer_agent_chat_ui::ahp_store::work_uri(container),
            manox_ahp::ext::XManoxState {
                child_runs: Some(runs),
                ..Default::default()
            },
        );
    });

    // The real close path closes every tab; the last one asks for the archive.
    let sheets = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx));
    assert!(!sheets.is_empty());
    for sheet in &sheets {
        // Redraw before every click: the strip moves with each close.
        draw(&mut visual);
        let close = visual
            .debug_bounds(close_selector(sheet))
            .unwrap_or_else(|| panic!("the close button of {sheet} paints"));
        visual.simulate_click(close.center(), gpui::Modifiers::default());
    }
    visual.cx.run_until_parked();

    let after = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx));
    assert_eq!(
        after,
        vec![format!("home:{container}")],
        "the pending archive leaves the container on its start page"
    );
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws
            .diagnostic_canvas_active_container(cx)),
        Some(container.to_string()),
        "the container stays while its archive waits for confirmation"
    );
    if let Some(path) = steer_canvas::persist::layout_path(container) {
        let _ = std::fs::remove_file(path);
    }
}

/// A pi sheet's close closes its session with the sheet: the layout commit
/// lands first, and only then the session (and its process) goes — a dropped
/// commit must never close a session whose sheet is still shown.
#[gpui::test]
async fn closing_a_pi_sheet_by_click_closes_its_session(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let container = "pi-close";
    let store = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store, container, cx);
    });
    draw(&mut visual);
    workspace.update(&mut visual.cx, |ws, cx| ws.diagnostic_canvas_open_pi(cx));
    draw(&mut visual);

    let pi_sheet = workspace
        .read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx))
        .into_iter()
        .find(|sheet| sheet.starts_with("pi-session:"))
        .expect("the pi sheet opens");
    let session = pi_sheet
        .strip_prefix("pi-session:")
        .expect("the pi sheet id names its session")
        .to_string();
    let pi_store = workspace
        .read_with(&visual.cx, |ws, _| ws.diagnostic_pi_store())
        .expect("the canvas created the pi store");
    let open_before = visual
        .cx
        .update(|cx| pi_store.read(cx).book().contains(&session));
    assert!(open_before, "the pi session is open before the close");
    assert!(
        workspace
            .read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_pi_views(cx))
            .contains(&pi_sheet),
        "the pi body view is the sheet's resource"
    );

    let close = visual
        .debug_bounds(close_selector(&pi_sheet))
        .expect("the pi tab's close button paints");
    visual.simulate_click(close.center(), gpui::Modifiers::default());
    visual.cx.run_until_parked();

    let after = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx));
    assert!(
        !after.contains(&pi_sheet),
        "the sheet is gone after the commit, got {after:?}"
    );
    assert!(
        !workspace
            .read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_pi_views(cx))
            .contains(&pi_sheet),
        "the pi body view is dropped with the sheet"
    );
    assert!(
        !visual
            .cx
            .update(|cx| pi_store.read(cx).book().contains(&session)),
        "the session closed with the committed layout, not before it"
    );
    if let Some(path) = steer_canvas::persist::layout_path(container) {
        let _ = std::fs::remove_file(path);
    }
}

#[gpui::test]
async fn a_terminal_sheet_opens_on_the_canvas(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    for _ in 0..2 {
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_canvas_open_terminal(cx)
    });

    let sheets = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx));
    assert!(
        sheets.iter().any(|sheet| sheet.starts_with("terminal:")),
        "the terminal gesture opens a terminal sheet, got {sheets:?}"
    );

    // The tab paints, and the sheet keeps its identity across a switch to the
    // start page and back (the PTY view is the resource, not the tab).
    for _ in 0..2 {
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }
    assert!(visual.debug_bounds("canvas-open-terminal").is_some());

    let terminal = sheets
        .into_iter()
        .find(|sheet| sheet.starts_with("terminal:"))
        .expect("a terminal sheet");
    workspace.update(&mut visual.cx, |ws, cx| {
        let home = ws
            .diagnostic_canvas_sheets(cx)
            .into_iter()
            .find(|sheet| sheet.starts_with("home:"));
        if let Some(home) = home {
            ws.diagnostic_canvas_activate(&home, cx);
        }
    });
    let still = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx));
    assert!(still.contains(&terminal), "the tab survives a switch");
}

#[gpui::test]
async fn a_double_click_splits_the_start_page_beside_the_tab(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    // The offscreen harness has no host: a detached store bound as the
    // foreground gives the canvas a real container (its session id).
    let store = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store, "split-demo", cx);
    });
    draw(&mut visual);

    let session = workspace
        .read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx))
        .into_iter()
        .find(|sheet| sheet.starts_with("session:"))
        .expect("the container opens with its session sheet");

    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_canvas_split_home(&session, cx);
    });
    draw(&mut visual);

    // Two groups in tree order, and still exactly one instance of each
    // sheet: the split moved the start page instead of duplicating it.
    let groups = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_groups(cx));
    assert_eq!(
        groups,
        vec![vec![session.clone()], vec!["home:split-demo".to_string()]],
        "the session keeps its strip; the start page moved to its own group"
    );
    assert!(visual.debug_bounds("canvas-open-home").is_some());
    assert!(visual.debug_bounds("canvas-open-terminal").is_some());
    let sheets = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx));
    assert_eq!(sheets, [session, "home:split-demo".to_string()]);
}

#[gpui::test]
async fn switching_containers_parks_and_restores_the_layout(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let first = visual.cx.new(|_| AhpStore::detached());
    let second = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(first.clone(), "park-alpha", cx);
    });
    draw(&mut visual);
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_canvas_open_terminal(cx);
    });
    draw(&mut visual);
    let before = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx));
    assert!(
        before.iter().any(|sheet| sheet.starts_with("terminal:")),
        "the parked canvas keeps a terminal sheet, got {before:?}"
    );

    // Switching the foreground container parks alpha and opens beta's own
    // canvas (its start page + its session sheet, never alpha's).
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(second, "park-beta", cx);
    });
    draw(&mut visual);
    let parked = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx));
    assert!(
        parked.iter().all(|sheet| !sheet.contains("park-alpha")),
        "the parked canvas is not on screen, got {parked:?}"
    );

    // Coming back restores the parked layout verbatim — the terminal tab is
    // the same sheet, not a fresh one.
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(first, "park-alpha", cx);
    });
    draw(&mut visual);
    let restored = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx));
    assert_eq!(restored, before, "the parked layout comes back as it was");
}

/// The save pump's park leg: an edit inside the debounce window lands when
/// its container leaves the foreground. The switch's generation bump would
/// otherwise retire the debounced write and the just-opened tab would come
/// back after a restart.
#[gpui::test]
async fn an_edit_inside_the_debounce_window_lands_when_the_container_parks(
    cx: &mut TestAppContext,
) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let first = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(first, "save-alpha", cx);
    });
    draw(&mut visual);
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_canvas_open_terminal(cx)
    });

    // The switch happens well inside the 400ms debounce window.
    let second = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(second, "save-beta", cx);
    });
    draw(&mut visual);

    let saved = steer_canvas::persist::read_canvas("save-alpha")
        .expect("the parked container's newest edit landed on disk");
    let sheets: Vec<String> = steer_canvas::layout::panel_ids(&saved.layout)
        .into_iter()
        .map(str::to_string)
        .collect();
    assert!(
        sheets.iter().any(|sheet| sheet.starts_with("terminal:")),
        "the edit that opened a terminal is in the file, got {sheets:?}"
    );
    if let Some(path) = steer_canvas::persist::layout_path("save-alpha") {
        let _ = std::fs::remove_file(path);
    }
    if let Some(path) = steer_canvas::persist::layout_path("save-beta") {
        let _ = std::fs::remove_file(path);
    }
}

/// The save pump's release leg: the entity's last pending save lands when the
/// entity is released, even though its debounced task dies with it.
#[gpui::test]
async fn releasing_the_canvas_flushes_its_pending_save(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let container = "save-release";
    let store = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store, container, cx);
    });
    draw(&mut visual);
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_canvas_open_terminal(cx)
    });

    // Dropping the workspace's handle releases the canvas; the release
    // listener fires at the next effect flush.
    workspace.update(&mut visual.cx, |ws, _| ws.diagnostic_release_canvas());
    visual.cx.run_until_parked();

    let saved = steer_canvas::persist::read_canvas(container)
        .expect("the released canvas flushed its pending save");
    let sheets: Vec<String> = steer_canvas::layout::panel_ids(&saved.layout)
        .into_iter()
        .map(str::to_string)
        .collect();
    assert!(
        sheets.iter().any(|sheet| sheet.starts_with("terminal:")),
        "the edit that opened a terminal is in the file, got {sheets:?}"
    );
    if let Some(path) = steer_canvas::persist::layout_path(container) {
        let _ = std::fs::remove_file(path);
    }
}
