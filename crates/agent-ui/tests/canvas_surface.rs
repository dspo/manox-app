//! The canvas surface (plan β): the container's layout tree paints its tab
//! strips and places the session sheet's body, and a tab close goes through the
//! canvas host's single write path.
//!
//! Own test binary: shared harness singletons (see `tests/common/mod.rs`).

#![cfg(feature = "test-support")]

mod common;

use common::{init_harness, open_workspace};
use gpui::{TestAppContext, VisualTestContext};

#[gpui::test]
async fn the_canvas_paints_its_strip_and_the_session_body(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    workspace.update(&mut visual.cx, |ws, cx| ws.open_canvas(cx));
    for _ in 0..3 {
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }

    assert!(
        visual.debug_bounds("canvas-main").is_some(),
        "the canvas main column must paint"
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

    workspace.update(&mut visual.cx, |ws, cx| ws.open_canvas(cx));
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

    workspace.update(&mut visual.cx, |ws, cx| ws.open_canvas(cx));
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
