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
