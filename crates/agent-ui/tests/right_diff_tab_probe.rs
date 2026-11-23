//! The edit cards' full-diff entry: the workspace parse lands in the right
//! dock's ONE diff sheet (a second open replaces the content instead of
//! stacking a sheet), the sheet paints the upstream `Diff` body, and the
//! label follows the opened file. Driven through the same workspace entry
//! the card chip routes to, against the production chrome assembly.
//!
//! Own test binary: shared harness singletons (see `tests/common/mod.rs`).
#![cfg(feature = "test-support")]

mod common;

use agent_ui::ahp_store::AhpStore;
use common::{full_wiring_boot, full_wiring_teardown};
use gpui::{AppContext as _, TestAppContext, VisualTestContext, px};

/// The app's real edit-output shape: an envelope line over a plain unified
/// diff with one file and two hunks.
const ONE_FILE_PATCH: &str = r#"[crates/agent-ui/src/workspace.rs#T4K2]
--- a/crates/agent-ui/src/workspace.rs
+++ b/crates/agent-ui/src/workspace.rs
@@ -3,6 +3,7 @@
 use std::collections::HashMap;
 use std::path::PathBuf;
+
 use crate::i18n;
 use gpui::ClickEvent;
-use gpui::DismissEvent;
+use gpui::DismissEvent as Dismiss;

@@ -41,4 +42,5 @@
 fn main() {
 let x = 1;
+let y = 2;
 println!("{x}");
 }
"#;

/// A git-style patch over two files — the replace leg's content.
const TWO_FILE_PATCH: &str = r#"diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,4 @@
 fn a() {}
+fn a2() {}

 fn b() {}
diff --git a/src/b.rs b/src/b.rs
--- a/src/b.rs
+++ b/src/b.rs
@@ -1,1 +1,2 @@
 struct B;
+impl B {}
"#;

fn open_diff(
    cx: &mut TestAppContext,
    ws: &gpui::Entity<agent_ui::Workspace>,
    window: gpui::AnyWindowHandle,
    path: Option<&str>,
    patch: &str,
) {
    let ws = ws.clone();
    let path = path.map(str::to_string);
    let patch = patch.to_string();
    cx.update_window(window, move |_, _, cx| {
        ws.update(cx, |ws, cx| ws.diagnostic_open_right_diff(path, patch, cx));
    })
    .expect("the full-wiring window is live");
}

fn pump(visual: &mut VisualTestContext, frames: usize) {
    for _ in 0..frames {
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }
}

#[gpui::test]
async fn an_edit_patch_opens_as_one_replacing_right_pane_diff_tab(cx: &mut TestAppContext) {
    let (ws, window) = full_wiring_boot(cx);
    let mut visual = VisualTestContext::from_window(window, cx);

    // A bound container: the diff sheet lands in THAT container's canvas
    // right dock (the edit card lives in the conversation). The boot lands
    // the launcher — focus the conversation first, or the canvas (and the
    // diff sheet with it) is not the main surface at all.
    let store = visual.cx.new(|_| AhpStore::detached());
    ws.update(&mut visual.cx, |ws, cx| {
        ws.focus_conversation(cx);
        ws.diagnostic_bind_store(store, "diff-demo", cx);
    });
    for _ in 0..3 {
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }

    open_diff(
        cx,
        &ws,
        window,
        Some("crates/agent-ui/src/workspace.rs"),
        ONE_FILE_PATCH,
    );
    pump(&mut visual, 3);

    let (title, files) = cx
        .update(|cx| ws.read(cx).diagnostic_right_diff(cx))
        .expect("the parsed diff state exists after the open");
    assert_eq!(files, 1, "the single-file patch parses to one file");
    assert_eq!(
        title, "workspace.rs",
        "the tab label is the target file name"
    );

    // The diff sheet rides the container's right dock, next to the session.
    let regions = cx.update(|cx| ws.read(cx).diagnostic_canvas_regions(cx));
    let right = regions
        .iter()
        .find(|(side, ..)| side == "right")
        .expect("the diff sheet's dock");
    let diff_sheets = right
        .2
        .iter()
        .filter(|sheet| sheet.starts_with("diff:"))
        .count();
    assert_eq!(
        diff_sheets, 1,
        "the diff kind stays a single sheet instance"
    );

    println!(
        "DEBUG: diff keys {:?} regions {:?} bounds {:?}",
        cx.update(|cx| ws.read(cx).diagnostic_diff_keys(cx)),
        cx.update(|cx| ws.read(cx).diagnostic_canvas_regions(cx)),
        visual.debug_bounds("canvas-diff-pane")
    );
    let bounds = visual
        .debug_bounds("canvas-diff-pane")
        .expect("the right dock paints the diff sheet body");
    assert!(
        bounds.size.width > px(100.) && bounds.size.height > px(100.),
        "the diff body fills a real pane region, not a collapsed box: {bounds:?}"
    );

    // The replace leg: a second open REPLACES the content (two files), same
    // sheet count.
    open_diff(cx, &ws, window, Some("a.rs"), TWO_FILE_PATCH);
    pump(&mut visual, 3);
    let (title2, files2) = cx
        .update(|cx| ws.read(cx).diagnostic_right_diff(cx))
        .expect("the replaced diff state exists");
    assert_eq!(files2, 2, "the second patch parses to two files");
    assert_eq!(
        title2, "a.rs",
        "the label falls back to the patch's own first file"
    );
    let regions = cx.update(|cx| ws.read(cx).diagnostic_canvas_regions(cx));
    let right = regions
        .iter()
        .find(|(side, ..)| side == "right")
        .expect("the dock");
    let diff_sheets = right
        .2
        .iter()
        .filter(|sheet| sheet.starts_with("diff:"))
        .count();
    assert_eq!(
        diff_sheets, 1,
        "the diff kind stays a single sheet instance"
    );

    // The sheet still paints after the replacement (the swapped DiffState
    // reaches the painted body, not just the workspace state).
    assert!(
        visual.debug_bounds("canvas-diff-pane").is_some(),
        "the replaced diff body still paints"
    );
    full_wiring_teardown(cx);
}
