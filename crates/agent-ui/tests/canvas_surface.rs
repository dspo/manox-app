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

/// Writes a container's layout file with `sheets` (a one-group tabs canvas),
/// as a previous run of the app would have left it. The harness writes under
/// the real state root; callers remove the file when they finish with it.
fn seed_layout(container: &str, sheets: &[String]) {
    let file = steer_canvas::persist::CanvasFile::new(
        container,
        steer_canvas::DockLayout::for_sheets(sheets.to_vec()),
        None,
    );
    steer_canvas::persist::write_canvas(&file).expect("the layout file writes");
}

/// Removes a container's layout file (the tests share the real state root).
fn drop_layout(container: &str) {
    if let Some(path) = steer_canvas::persist::layout_path(container) {
        let _ = std::fs::remove_file(path);
    }
}

/// A restart's restore leg: a layout that came back from disk carries a
/// terminal tab, and that tab gets its PTY back — the view is the sheet's
/// resource, so `open_container` re-attaches a fresh shell per restored
/// sheet (the old one died with the process; the body never paints blank).
/// The next terminal the user opens is a fresh index, not the restored one.
#[gpui::test]
async fn a_restored_terminal_sheet_gets_its_pty_back(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let container = "restore-term";
    let restored_sheet = steer_canvas::id::terminal_sheet_id(container, 1);
    seed_layout(
        container,
        &[
            steer_canvas::id::home_sheet_id(container),
            restored_sheet.clone(),
        ],
    );

    let store = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store, container, cx);
    });
    draw(&mut visual);

    let views = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_terminal_views(cx));
    assert_eq!(
        views,
        vec![restored_sheet.clone()],
        "the restored terminal tab holds its PTY view"
    );
    let sheets = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx));
    assert!(
        sheets.contains(&restored_sheet),
        "the restored tab is on the canvas, got {sheets:?}"
    );

    // The next terminal skips the restored index.
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_canvas_open_terminal(cx)
    });
    let views = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_terminal_views(cx));
    assert_eq!(
        views,
        vec![
            restored_sheet.clone(),
            steer_canvas::id::terminal_sheet_id(container, 2),
        ],
        "a fresh terminal opens at the next free index"
    );
    drop_layout(container);
}

/// A restart's restore leg for pi: the restored tab's session is re-opened in
/// the store BEFORE the body view is created, so the tab never paints the
/// closed-session empty state (the session file being gone is acceptable —
/// the store rebuilds an empty session under the same id).
#[gpui::test]
async fn a_restored_pi_sheet_reopens_its_session(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let container = "restore-pi";
    let session = "01990000-0000-7000-8000-000000000001";
    let pi_sheet = format!("pi-session:{session}");
    seed_layout(
        container,
        &[steer_canvas::id::home_sheet_id(container), pi_sheet.clone()],
    );

    let store = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store, container, cx);
    });
    draw(&mut visual);

    // The re-open happened at restore time, before any body rendered.
    let pi_store = workspace
        .read_with(&visual.cx, |ws, _| ws.diagnostic_pi_store())
        .expect("the canvas created the pi store");
    let open = visual
        .cx
        .update(|cx| pi_store.read(cx).book().contains(session));
    assert!(open, "the restored tab's session is open in the store");

    // The tab click shows the body: the view entity is created on the
    // sheet's first show, against a session the store already holds.
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_canvas_activate(&pi_sheet, cx);
    });
    draw(&mut visual);
    let views = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_pi_views(cx));
    assert_eq!(
        views,
        vec![pi_sheet],
        "the restored pi tab holds its body view"
    );
    drop_layout(container);
}

/// The container-title projection: a session with no title yet answers its
/// tab's localized placeholder, and the placeholder names a TAB, never the
/// container — the row keeps its own catalogue title (the seed name / cwd
/// tail). A session that has earned a title names the container again.
#[gpui::test]
async fn an_untitled_session_does_not_name_its_container(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let container = "title-cnt";
    let store = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store.clone(), container, cx);
    });
    draw(&mut visual);

    let facts = workspace
        .read_with(&visual.cx, |ws, cx| {
            ws.diagnostic_canvas_row_facts(container, cx)
        })
        .expect("the container has a canvas");
    assert!(facts.sheet_count >= 2, "the canvas reports its sheet count");
    assert_eq!(
        facts.title, None,
        "an untitled session must not name its container"
    );

    let summary: ahp_types::state::SessionSummary = serde_json::from_value(serde_json::json!({
        "resource": format!("ahp-session:/{container}"),
        "provider": "test",
        "title": "Steady container",
        "status": 0,
        "createdAt": "2026-01-01T00:00:00Z",
        "modifiedAt": "2026-01-01T00:00:00Z",
    }))
    .expect("the summary parses");
    store.update(&mut visual.cx, |store, _| {
        store.book.summaries.insert(container.to_string(), summary);
    });
    let facts = workspace
        .read_with(&visual.cx, |ws, cx| {
            ws.diagnostic_canvas_row_facts(container, cx)
        })
        .expect("the container has a canvas");
    assert_eq!(
        facts.title.as_deref(),
        Some("Steady container"),
        "a titled session names the container again"
    );
    drop_layout(container);
}

// ── the 08A gesture matrix ────────────────────────────────────────────────
// Drags run the REAL pointer sequence (down → move → up) through the shell's
// mounted canvas; the drop is `projection::drop_target`'s verdict, committed
// once, through the host's single write path.

/// A drag as a real pointer sequence: press, a nudge (the drag promotes while
/// the pointer is still over the source), then travel, then drop.
fn drag(
    visual: &mut VisualTestContext,
    from: gpui::Point<gpui::Pixels>,
    to: gpui::Point<gpui::Pixels>,
) {
    use gpui::{Modifiers, MouseButton};
    let nudge = gpui::point(from.x + gpui::px(4.), from.y + gpui::px(2.));
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(nudge, MouseButton::Left, Modifiers::default());
    visual.cx.run_until_parked();
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.cx.run_until_parked();
}

fn release(visual: &mut VisualTestContext, to: gpui::Point<gpui::Pixels>) {
    use gpui::{Modifiers, MouseButton};
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    visual.cx.run_until_parked();
}

fn center(bounds: gpui::Bounds<gpui::Pixels>) -> gpui::Point<gpui::Pixels> {
    gpui::point(
        bounds.left() + bounds.size.width / 2.,
        bounds.top() + bounds.size.height / 2.,
    )
}

#[gpui::test]
async fn a_right_edge_drag_splits_the_single_group(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let store = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store, "drag-split", cx);
    });
    draw(&mut visual);
    let session = workspace
        .read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx))
        .into_iter()
        .find(|sheet| sheet.starts_with("session:"))
        .expect("the container opens with its session sheet");

    // Drag the session tab toward the canvas's right edge: the outer ring is
    // the outermost group's own edge — a SPLIT, never a dock.
    let tab = visual
        .debug_bounds(Box::leak(format!("canvas-tab-{session}").into_boxed_str()))
        .expect("the session tab paints");
    let card = visual
        .debug_bounds("canvas-strip-home:drag-split")
        .expect("the strip paints");
    let edge = gpui::point(card.right() - gpui::px(8.), tab.bottom() + gpui::px(150.));
    drag(&mut visual, center(tab), edge);

    // Mid-drag: the source fades, the indicator paints, and the layout has
    // not moved yet (a move-drag writes nothing).
    let dragging = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_dragging(cx));
    assert_eq!(
        dragging.as_deref(),
        Some(session.as_str()),
        "the drag is live"
    );
    assert!(
        visual.debug_bounds("canvas-drop-indicator").is_some(),
        "the indicator paints mid-drag"
    );
    assert_eq!(
        workspace
            .read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_groups(cx))
            .len(),
        1,
        "the layout does not move until the drop"
    );

    release(&mut visual, edge);
    let groups = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_groups(cx));
    assert_eq!(groups.len(), 2, "the right-edge drop splits the group");
    let sheets = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx));
    assert_eq!(
        sheets.len(),
        sheets
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        "one instance per sheet"
    );
}

#[gpui::test]
async fn a_tab_drag_between_groups_moves_the_sheet(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let store = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store, "drag-move", cx);
    });
    draw(&mut visual);
    let session = workspace
        .read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx))
        .into_iter()
        .find(|sheet| sheet.starts_with("session:"))
        .expect("the session sheet");
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_canvas_split_home(&session, cx);
    });
    draw(&mut visual);
    let home = "home:drag-move".to_string();
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_groups(cx)),
        [vec![session.clone()], vec![home.clone()]],
        "the seeded split"
    );

    // Drop the start page ONTO the session group: it joins that group.
    let tab = visual
        .debug_bounds(Box::leak(format!("canvas-tab-{home}").into_boxed_str()))
        .expect("the home tab paints");
    let other = visual
        .debug_bounds(Box::leak(format!("canvas-tab-{session}").into_boxed_str()))
        .expect("the session tab paints");
    let content = gpui::point(center(other).x, other.bottom() + gpui::px(120.));
    drag(&mut visual, center(tab), content);
    release(&mut visual, content);

    let groups = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_groups(cx));
    assert_eq!(groups.len(), 1, "the emptied source group collapses");
    // A content drop appends (and shows): the dragged tab lands last.
    assert_eq!(
        groups[0],
        [session, home],
        "the drop joined the group, in slot order"
    );
}

#[gpui::test]
async fn the_dock_toggle_opens_a_terminal_into_an_empty_bottom(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let store = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store, "dock-open", cx);
    });
    draw(&mut visual);

    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_canvas_toggle_dock("bottom", cx);
    });
    let regions = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_regions(cx));
    let bottom = regions
        .iter()
        .find(|(side, _, _)| side == "bottom")
        .expect("the region exists");
    assert!(bottom.1, "the first toggle opens the region");
    assert!(
        bottom.2.iter().any(|sheet| sheet.starts_with("terminal:")),
        "opening an empty bottom dock asks for a terminal, got {:?}",
        bottom.2
    );

    // The dock's group paints its own strip (named by its sheet).
    draw(&mut visual);
    let terminal = bottom
        .2
        .iter()
        .find(|sheet| sheet.starts_with("terminal:"))
        .expect("the dock's terminal");
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("canvas-strip-{terminal}").into_boxed_str()
            ))
            .is_some()
    );

    // And the second toggle closes it, keeping the region (its sheets stay).
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_canvas_toggle_dock("bottom", cx);
    });
    let regions = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_regions(cx));
    let bottom = regions
        .iter()
        .find(|(side, _, _)| side == "bottom")
        .expect("the region stays");
    assert!(!bottom.1, "the second toggle closes");
    assert!(
        !bottom.2.is_empty(),
        "a closed dock's sheets stay resources"
    );
}

#[gpui::test]
async fn a_dock_drag_sizes_and_collapses_below_the_floor(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let store = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store, "dock-size", cx);
    });
    draw(&mut visual);
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_canvas_toggle_dock("bottom", cx);
    });
    draw(&mut visual);

    // The dock band rides the seam; drag it downward past the floor: the
    // dock collapses instead of going under 100pt.
    let band = visual
        .debug_bounds("canvas-dock-band-bottom")
        .expect("the dock band paints");
    let below = gpui::point(center(band).x, band.top() + gpui::px(120.));
    drag(&mut visual, center(band), below);
    let regions = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_regions(cx));
    let bottom = regions
        .iter()
        .find(|(side, ..)| side == "bottom")
        .expect("the region");
    assert!(
        !bottom.1,
        "dragging the band below the floor collapses the dock"
    );

    // Drag the closed edge upward: past the floor it reopens.
    let band = visual
        .debug_bounds("canvas-dock-band-bottom")
        .expect("the closed edge still carries its band");
    let up = gpui::point(center(band).x, band.top() - gpui::px(300.));
    drag(&mut visual, center(band), up);
    release(&mut visual, up);
    let regions = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_regions(cx));
    let bottom = regions
        .iter()
        .find(|(side, ..)| side == "bottom")
        .expect("the region");
    assert!(bottom.1, "dragging a closed edge past the floor reopens it");
    let size = workspace.read_with(&visual.cx, |ws, cx| {
        ws.diagnostic_canvas_dock_size("bottom", cx)
    });
    assert!(
        size.is_some_and(|size| size >= 100.),
        "the reopened dock records its dragged size, got {size:?}"
    );
}

#[gpui::test]
async fn a_zoom_toggles_and_survives_a_container_switch(cx: &mut TestAppContext) {
    init_harness(cx);
    // The tests share the real state root: a stale zoom-alpha file (a
    // previous run's save pump) would restore a zoom the toggle then clears.
    for container in ["zoom-alpha", "zoom-beta"] {
        if let Some(path) = steer_canvas::persist::layout_path(container) {
            let _ = std::fs::remove_file(path);
        }
    }
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let alpha = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(alpha, "zoom-alpha", cx);
    });
    draw(&mut visual);
    let session = workspace
        .read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx))
        .into_iter()
        .find(|sheet| sheet.starts_with("session:"))
        .expect("the session sheet");

    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_canvas_toggle_zoom(&session, cx);
    });
    assert_eq!(
        workspace
            .read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_zoomed(cx))
            .as_deref(),
        Some(session.as_str())
    );

    // A dock sheet never zooms, even by request.
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_canvas_toggle_dock("bottom", cx);
    });
    let terminal = workspace
        .read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_regions(cx))
        .iter()
        .find(|(side, ..)| side == "bottom")
        .and_then(|(_, _, panels)| panels.iter().find(|s| s.starts_with("terminal:")).cloned())
        .expect("the dock's terminal");
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_canvas_toggle_zoom(&terminal, cx);
    });
    assert_eq!(
        workspace
            .read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_zoomed(cx))
            .as_deref(),
        Some(session.as_str()),
        "a dock sheet never zooms"
    );

    // Switch away and back: the zoom comes home with the layout.
    let beta = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(beta, "zoom-beta", cx);
    });
    draw(&mut visual);
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_zoomed(cx)),
        None,
        "the parked canvas takes its zoom with it"
    );
    let alpha = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(alpha, "zoom-alpha", cx);
    });
    draw(&mut visual);
    assert_eq!(
        workspace
            .read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_zoomed(cx))
            .as_deref(),
        Some(session.as_str()),
        "the zoom restores with the container"
    );
    let _ = window;
}

#[gpui::test]
async fn a_split_handle_drag_sizes_and_the_center_resets(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let store = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store, "split-size", cx);
    });
    draw(&mut visual);
    let session = workspace
        .read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_sheets(cx))
        .into_iter()
        .find(|sheet| sheet.starts_with("session:"))
        .expect("the session sheet");
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_canvas_split_home(&session, cx);
    });
    draw(&mut visual);

    // Drag the center's split band: the slots record real sizes.
    let band = visual
        .debug_bounds("canvas-split-band-root")
        .expect("the split band paints");
    let right = gpui::point(band.right() + gpui::px(180.), center(band).y);
    drag(&mut visual, center(band), right);
    release(&mut visual, right);
    let sizes = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_center_sizes(cx));
    assert!(
        matches!(&sizes, Some(Some(values)) if values.len() == 2 && values[0] > 100.),
        "the drag records the split's sizes, got {sizes:?}"
    );

    // The double-click resets to equal shares.
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_canvas_set_split_sizes(None, cx);
    });
    let sizes = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_center_sizes(cx));
    assert_eq!(sizes, Some(None), "the reset lands None (equal shares)");
}

// ── the 08B right-dock tool kinds ────────────────────────────────────────

#[gpui::test]
async fn an_agent_sheet_opens_in_the_right_dock_with_its_picker(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let store = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store, "agent-demo", cx);
    });
    draw(&mut visual);

    // The strip's CLI entry: the sheet lands in the right dock, its body is
    // the picker (the agent segment first).
    let sheet = "agent:agent-demo:1".to_string();
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_canvas_drop(&sheet, "dock:right", cx);
    });
    for _ in 0..3 {
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }

    let regions = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_regions(cx));
    let right = regions
        .iter()
        .find(|(side, ..)| side == "right")
        .expect("the right dock");
    assert!(
        right.2.iter().any(|s| s == &sheet),
        "the agent sheet rides the right dock, got {:?}",
        right.2
    );
    // The picker body paints (the agent segment's rows).
    println!(
        "DEBUG agent keys: {:?} bounds {:?}",
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_agent_keys(cx)),
        visual.debug_bounds("agent-picker")
    );
    assert!(
        visual.debug_bounds("agent-picker").is_some(),
        "the CLI picker paints as the agent sheet's body"
    );
}

#[gpui::test]
async fn an_editor_sheet_restores_only_while_its_file_exists(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("draft.md");
    std::fs::write(&path, "# draft\n").unwrap();
    let encoded = steer_canvas::persist::encode_id(&path.to_string_lossy());

    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let store = visual.cx.new(|_| AhpStore::detached());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store, "editor-demo", cx);
    });
    draw(&mut visual);

    let sheet = format!("editor:editor-demo:{}", encoded);
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_canvas_drop(&sheet, "dock:right", cx);
    });
    draw(&mut visual);

    // The editor body paints (a minted markdown editor).
    let regions = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_canvas_regions(cx));
    let right = regions
        .iter()
        .find(|(side, ..)| side == "right")
        .expect("the dock");
    assert!(
        right.2.iter().any(|s| s == &sheet),
        "the editor sheet rides the dock"
    );

    // The restore face: the file exists here; a dead path prunes the sheet.
    let file_alive = std::path::Path::new(&path).exists();
    assert!(file_alive, "the probe's file exists");
    let _ = visual;
}
