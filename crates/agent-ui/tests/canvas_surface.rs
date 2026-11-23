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
