//! The canvas surface (plan β): the container's layout tree paints its tab
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
